//! In-process, host-local application boundary for D16.
//!
//! The [`LocalApplicationBackend`] trait supplies a passive bounded state
//! capture and owns explicit local effects. Snapshot construction itself never
//! runs Git, tmux, a provider executable/helper, a presentation mutation, or a
//! metadata refresh.

#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::unused_self,
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    reason = "The boundary names are intentionally explicit in public DTOs."
)]

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Component, Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use thiserror::Error;

use crate::domain::{
    AttentionState, HostId, LocationId, OperationId, OperationKind, OperationPhase, ProjectId,
    ProviderKind, Revision, RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
};
use crate::{
    actions,
    domain::RandomIdGenerator,
    provider,
    provider::codex::profile::{ObserverProfile, ProfileInspection},
    repository,
    state::{
        self, D16State, FreshRootRejection, HostRegistry, IntegrationLifecycle, StateError,
        StateRecoveryReason, StateRoot,
    },
};

/// Hard bounds for one application projection.
pub const MAX_SNAPSHOT_PROJECTS: usize = 128;
pub const MAX_SNAPSHOT_LOCATIONS: usize = 512;
pub const MAX_SNAPSHOT_WORKSTREAMS: usize = 128;
pub const MAX_SNAPSHOT_OPERATIONS: usize = 128;
pub const MAX_SNAPSHOT_CAPABILITIES: usize = 8;
const MAX_TEXT_SCALARS: usize = 256;
const MAX_BROWSER_RELATIVE_BYTES: usize = 1024;
const MAX_BROWSER_ROOT_BYTES: usize = 4096;
const MAX_BROWSER_ENTRIES: usize = 128;
const MAX_OBSERVER_PROFILE_BYTES: u64 = 128 * 1024;

/// Explicit bounds used by a production facade or a focused test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotLimits {
    pub projects: usize,
    pub locations: usize,
    pub workstreams: usize,
    pub operations: usize,
    pub capabilities: usize,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            projects: MAX_SNAPSHOT_PROJECTS,
            locations: MAX_SNAPSHOT_LOCATIONS,
            workstreams: MAX_SNAPSHOT_WORKSTREAMS,
            operations: MAX_SNAPSHOT_OPERATIONS,
            capabilities: MAX_SNAPSHOT_CAPABILITIES,
        }
    }
}

impl SnapshotLimits {
    /// Creates explicit bounds, primarily for disposable bound tests.
    #[must_use]
    pub const fn new(
        projects: usize,
        locations: usize,
        workstreams: usize,
        operations: usize,
        capabilities: usize,
    ) -> Self {
        Self {
            projects,
            locations,
            workstreams,
            operations,
            capabilities,
        }
    }
}

/// The projection collection that exceeded its bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotLimitKind {
    Projects,
    Locations,
    Workstreams,
    Operations,
    Capabilities,
}

/// Opaque local entity whose revision was supplied to an action authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisionSubject {
    Project(ProjectId),
    Location(LocationId),
    Workstream(WorkstreamId),
    Runtime(RuntimeId),
    Attention(WorkstreamId),
    Operation(OperationId),
    ProjectBrowserRoot,
}

/// Typed errors for the local application boundary.
///
/// No variant carries a host alias/target, remote state, protocol envelope,
/// cursor/page/replay state, raw path, provider payload, or free-form
/// diagnostic string.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ApplicationError {
    #[error("snapshot exceeds its {kind:?} bound of {limit}")]
    SnapshotOverLimit {
        kind: SnapshotLimitKind,
        limit: usize,
    },
    #[error("snapshot contains duplicate {entity:?}")]
    DuplicateSnapshotIdentity { entity: SnapshotEntity },
    #[error("snapshot contains invalid {entity:?} data")]
    InvalidSnapshotEntity { entity: SnapshotEntity },
    #[error("snapshot references an unknown Project")]
    UnknownSnapshotProject(ProjectId),
    #[error("snapshot references an unknown Location")]
    UnknownSnapshotLocation(LocationId),
    #[error("snapshot Location belongs to another Project")]
    LocationProjectMismatch(LocationId),
    #[error("snapshot Workstream belongs to another Project")]
    WorkstreamProjectMismatch(WorkstreamId),
    #[error("snapshot includes a terminal operation as unresolved")]
    TerminalOperation(OperationId),
    #[error("snapshot includes duplicate provider capability")]
    DuplicateProviderCapability(ProviderKind),
    #[error("snapshot is missing provider capability")]
    MissingProviderCapability(ProviderKind),
    #[error("provider capability evidence is malformed")]
    InvalidProviderCapability(ProviderKind),
    #[error("invalid host-local browser path")]
    InvalidBrowserPath,
    #[error("invalid browser listing")]
    InvalidBrowserListing,
    #[error("invalid complete Project refresh")]
    InvalidProjectRefresh,
    #[error("stale {subject:?} revision")]
    StaleRevision {
        subject: RevisionSubject,
        expected: Revision,
        current: Revision,
    },
    #[error("unknown local identity")]
    UnknownLocalIdentity,
    #[error("local provider identity does not match the requested action")]
    ProviderIdentityMismatch,
    #[error("unsupported local action")]
    UnsupportedAction,
    #[error("confirmed state cutover is required")]
    CutoverRequired,
    #[error("fresh state creation is required")]
    FreshStateRequired,
    #[error("host state schema {schema_version} requires an explicit reset")]
    HostStateResetRequired { schema_version: i64 },
    #[error("host schema evidence is malformed")]
    MalformedHostSchema,
    #[error("host state uses unsupported future schema {schema_version}")]
    UnsupportedFutureHostSchema { schema_version: i64 },
    #[error("host state recovery is required: {reason:?}")]
    StateRecoveryRequired { reason: StateRecoveryReason },
    #[error("fresh state root is not adoptable: {reason:?}")]
    FreshRootRejected { reason: FreshRootRejection },
    #[error("native attachment evidence does not match its requested target")]
    AttachmentEvidenceMismatch,
    #[error("snapshot authority failed")]
    SnapshotAuthorityFailed,
    #[error("action authority failed")]
    ActionAuthorityFailed,
    #[error("attachment authority failed")]
    AttachmentAuthorityFailed,
    #[error("local action failed: {reason}")]
    ActionFailed { reason: ActionFailureReason },
    #[error("observer readiness is unavailable for this action")]
    ObserverUnavailable { readiness: ObserverReadiness },
}

/// Bounded, content-free classifications for a failed local action.
///
/// The application boundary deliberately reduces internal provider, process,
/// filesystem, and state errors to this finite set before public application
/// output or presentation, so callers cannot expose provider payloads,
/// terminal output, credentials, raw paths, or unbounded process diagnostics.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActionFailureReason {
    #[error("provider did not become ready before the startup deadline")]
    ProviderReadinessTimeout,
    #[error("OpenCode observer failed during startup")]
    OpenCodeObserverStartupFailed,
    #[error("OpenCode observer did not become ready before the startup deadline")]
    OpenCodeObserverReadinessTimeout,
    #[error("OpenCode observer identity changed during startup")]
    OpenCodeObserverIdentityChanged,
    #[error("OpenCode observer exited before becoming ready")]
    OpenCodeObserverExitedBeforeReady,
    #[error("runtime evidence is ambiguous")]
    RuntimeEvidenceAmbiguous,
}

/// Identity categories used in typed projection validation errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotEntity {
    Project,
    Location,
    Workstream,
    Runtime,
    Operation,
    ProviderCapability,
    ObserverReadiness,
    ProjectBrowser,
    DisplayText,
    NativeName,
}

/// Read-only status of the Codex observer integration on this host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverReadiness {
    Ready,
    SetupRequired,
    TrustReviewRequired,
    UpdateRequired,
    Modified,
    Disabled,
    Foreign,
    Ambiguous,
    Unknown,
}

/// Exact read-only observer evidence captured with a pending action.
/// `None` is meaningful: it proves that no owned integration row existed at
/// capture time and must be revalidated as absence before preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObserverReadinessEvidence {
    pub readiness: ObserverReadiness,
    pub integration_revision: Option<Revision>,
}

impl ObserverReadinessEvidence {
    #[must_use]
    pub const fn needs_guide(self) -> bool {
        matches!(
            self.readiness,
            ObserverReadiness::SetupRequired
                | ObserverReadiness::TrustReviewRequired
                | ObserverReadiness::UpdateRequired
        )
    }
}

/// Availability status of one local provider's capability evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderCapabilityStatus {
    Available,
    Unavailable,
    Unknown,
}

/// Bounded reason accompanying unavailable or unknown provider evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderCapabilityReason {
    AdapterUnavailable,
    NotInstalled,
    UnsupportedVersion,
    ObserverNotReady,
    RuntimePrerequisiteMissing,
    ProbeFailed,
}

/// Dynamic capability evidence for one local provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderCapability {
    pub provider: ProviderKind,
    pub status: ProviderCapabilityStatus,
    pub reason: Option<ProviderCapabilityReason>,
    pub fresh_launch: bool,
    pub exact_resume: bool,
    pub observe: bool,
    pub metadata_read: bool,
    pub navigator_rename: bool,
    pub fork: bool,
}

impl ProviderCapability {
    /// Whether the provider can launch a fresh local Workstream.
    #[must_use]
    pub const fn eligible_for_new(self) -> bool {
        matches!(self.status, ProviderCapabilityStatus::Available)
            && self.fresh_launch
            && self.exact_resume
            && self.observe
    }

    /// Whether the provider can exact-resume a local Workstream.
    #[must_use]
    pub const fn eligible_for_resume(self) -> bool {
        matches!(self.status, ProviderCapabilityStatus::Available)
            && self.exact_resume
            && self.observe
    }

    /// Whether the provider can fork a local Workstream.
    #[must_use]
    pub const fn eligible_for_fork(self) -> bool {
        matches!(self.status, ProviderCapabilityStatus::Available) && self.fork && self.observe
    }
}

/// Bounded attention state shown by the navigator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttentionSnapshot {
    pub result_unseen: bool,
    pub recovery_unseen: bool,
    pub revision: Revision,
}

impl AttentionSnapshot {
    /// Projects durable attention without exposing provider turn contents.
    #[must_use]
    pub const fn from_state(state: &AttentionState) -> Self {
        Self {
            result_unseen: state.result_unseen_since_revision.is_some(),
            recovery_unseen: state.recovery_unseen_since_revision.is_some(),
            revision: state.revision,
        }
    }
}

/// Opaque local Runtime evidence needed by an attachment row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeSnapshot {
    pub runtime_id: RuntimeId,
    pub status: RuntimeStatus,
    pub revision: Revision,
    /// True only when the exact current Runtime generation has a private
    /// observer-degraded marker.  Live process status and attachment evidence
    /// remain unchanged; this flag gates observer-dependent mutations.
    pub observer_degraded: bool,
}

/// One host-local `ProjectLocation`, ordered by opaque `LocationId`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocationSnapshot {
    pub project_id: ProjectId,
    pub location_id: LocationId,
    pub display_name: String,
    pub revision: Revision,
    pub repository_fingerprint: Option<String>,
    pub origin_display: Option<String>,
    pub is_label_source: bool,
}

/// One active or archived Workstream in a Project group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamSnapshot {
    pub project_id: ProjectId,
    pub location_id: LocationId,
    pub workstream_id: WorkstreamId,
    pub provider: ProviderKind,
    pub lifecycle: WorkstreamLifecycle,
    pub archived: bool,
    pub last_activity_sequence: i64,
    /// Wall-clock time of the most recent observed native activity. `None`
    /// means no activity timestamp has been observed and no time is inferred.
    pub last_activity_at_millis: Option<i64>,
    pub revision: Revision,
    pub runtime: Option<RuntimeSnapshot>,
    pub attention: AttentionSnapshot,
    pub native_name: Option<String>,
}

/// One unresolved local operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub provider: ProviderKind,
    pub source_workstream_id: Option<WorkstreamId>,
    pub phase: OperationPhase,
    pub revision: Revision,
}

/// Safe display and revision evidence for the host-private browser root.
/// The configured absolute path never enters the application snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectBrowserSnapshot {
    pub root_label: String,
    pub revision: Revision,
}

/// One schema-13 Project input to the passive backend seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSnapshotInput {
    pub project_id: ProjectId,
    pub display_name: String,
    pub revision: Revision,
    pub label_location_id: LocationId,
    pub repository_fingerprint: Option<String>,
    pub origin_display: Option<String>,
    pub locations: Vec<LocationSnapshot>,
}

/// One Workstream input to the passive backend seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamSnapshotInput {
    pub project_id: ProjectId,
    pub location_id: LocationId,
    pub workstream_id: WorkstreamId,
    pub provider: ProviderKind,
    pub lifecycle: WorkstreamLifecycle,
    pub archived: bool,
    pub last_activity_sequence: i64,
    /// Wall-clock time of the most recent observed native activity. `None`
    /// means no activity timestamp has been observed and no time is inferred.
    pub last_activity_at_millis: Option<i64>,
    pub revision: Revision,
    pub runtime: Option<RuntimeSnapshot>,
    pub attention: AttentionSnapshot,
    pub native_name: Option<String>,
}

/// One complete passive input to [`LocalApplication::snapshot`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotInput {
    pub projects: Vec<ProjectSnapshotInput>,
    pub workstreams: Vec<WorkstreamSnapshotInput>,
    pub unresolved_operations: Vec<OperationSnapshot>,
    pub observer_readiness: ObserverReadinessEvidence,
    pub project_browser: ProjectBrowserSnapshot,
    pub provider_capabilities: Vec<ProviderCapability>,
}

/// One canonical host-local Project inventory row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSnapshot {
    pub project_id: ProjectId,
    pub display_name: String,
    pub revision: Revision,
    pub label_location_id: LocationId,
    pub repository_fingerprint: Option<String>,
    pub origin_display: Option<String>,
    pub locations: Vec<LocationSnapshot>,
}

/// One page-specific Project group.  Active and Archived groups are sorted
/// independently using only their included page members.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectWorkstreamGroup {
    pub project_id: ProjectId,
    pub max_activity_sequence: i64,
    pub workstreams: Vec<WorkstreamSnapshot>,
}

/// One deterministic hard-bounded current-host projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationSnapshot {
    /// Host identity appears once as registry identity/fallback evidence.
    pub host_id: HostId,
    /// Display-only label; it is never an action selector.
    pub host_display: String,
    pub projects: Vec<ProjectSnapshot>,
    pub active_project_groups: Vec<ProjectWorkstreamGroup>,
    pub archived_project_groups: Vec<ProjectWorkstreamGroup>,
    pub unresolved_operations: Vec<OperationSnapshot>,
    pub observer_readiness: ObserverReadinessEvidence,
    pub project_browser: ProjectBrowserSnapshot,
    pub provider_capabilities: Vec<ProviderCapability>,
}

impl ApplicationSnapshot {
    /// Iterates active Workstreams in Project-group and child order.
    pub fn active_workstreams(&self) -> impl Iterator<Item = &WorkstreamSnapshot> {
        self.active_project_groups
            .iter()
            .flat_map(|project| project.workstreams.iter())
    }

    /// Iterates archived Workstreams in Project-group and child order.
    pub fn archived_workstreams(&self) -> impl Iterator<Item = &WorkstreamSnapshot> {
        self.archived_project_groups
            .iter()
            .flat_map(|project| project.workstreams.iter())
    }
}

/// A bounded relative path resolved only by the host-local backend.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserPath(String);

impl BrowserPath {
    /// Creates the empty relative path at the configured browser root.
    #[must_use]
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Validates a slash-separated relative path without canonicalizing it.
    pub fn new(value: impl Into<String>) -> Result<Self, ApplicationError> {
        let value = value.into();
        let valid = value.is_empty()
            || (value.len() <= MAX_BROWSER_RELATIVE_BYTES
                && !value.contains('\0')
                && !value.contains('\\')
                && !value
                    .chars()
                    .any(|character| character.is_control() || is_unicode_format(character))
                && value
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != ".."));
        valid
            .then_some(Self(value))
            .ok_or(ApplicationError::InvalidBrowserPath)
    }

    /// Returns the bounded relative path for backend resolution.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A bounded host-local browser root path.  It is request-only and is never
/// returned in a public snapshot.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BrowserRootPath(String);

impl BrowserRootPath {
    /// Validates one bounded root path without canonicalizing it.
    pub fn new(value: impl Into<String>) -> Result<Self, ApplicationError> {
        let value = value.into();
        if value.is_empty()
            || !value.starts_with('/')
            || value.len() > MAX_BROWSER_ROOT_BYTES
            || value.contains('\0')
            || value
                .chars()
                .any(|character| character.is_control() || is_unicode_format(character))
            || (value != "/"
                && (value.ends_with('/')
                    || value.strip_prefix('/').is_none_or(|rest| {
                        rest.split('/')
                            .any(|part| part.is_empty() || part == "." || part == "..")
                    })))
        {
            return Err(ApplicationError::InvalidBrowserPath);
        }
        Ok(Self(value))
    }

    /// Returns the bounded host-local root path for the backend.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for BrowserPath {
    fn default() -> Self {
        Self::root()
    }
}

/// One safe direct-child browser entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserEntry {
    pub name: String,
    pub is_git_repository: bool,
}

/// Result of an explicit bounded browser-list action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserListing {
    pub root_label: String,
    pub relative_path: BrowserPath,
    pub include_hidden: bool,
    pub entries: Vec<BrowserEntry>,
    pub revision: Revision,
}

/// Complete revision-checked Project metadata refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRefreshRequest {
    pub project_id: ProjectId,
    pub expected_project_revision: Revision,
}

/// Which sticky attention bit to acknowledge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttentionKind {
    Result,
}

/// Exact observer-dependent request captured in a readiness guide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverIntent {
    Start {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        provider: ProviderKind,
    },
    Recover {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        provider: ProviderKind,
    },
    RecoverOperation {
        operation_id: OperationId,
        expected_revision: Revision,
        provider: ProviderKind,
    },
    RegisterLocation {
        /// The exact relative cursor remains in the captured
        /// `ApplicationAction`; this guide carries its root CAS evidence and
        /// selected provider without copying a path into readiness state.
        expected_browser_revision: Revision,
        provider: ProviderKind,
    },
    NewAtLocation {
        project_id: ProjectId,
        location_id: LocationId,
        expected_project_revision: Revision,
        expected_location_revision: Revision,
        provider: ProviderKind,
    },
    NewAtSameLocation {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    Fork {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
}

/// Typed contextual guidance; this facade never performs profile preparation
/// or native trust review on behalf of a caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObserverReadinessGuide {
    pub evidence: ObserverReadinessEvidence,
    pub intent: ObserverIntent,
    pub explicit_interactive_consent_required: bool,
    pub native_trust_review_required: bool,
}

/// Typed local actions.  Targets are only opaque current-host IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplicationAction {
    AcknowledgeAttention {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        kind: AttentionKind,
    },
    Park {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    },
    Archive {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    },
    Restore {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    },
    Rename {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        name: String,
    },
    Start {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        provider: ProviderKind,
    },
    Recover {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        provider: ProviderKind,
    },
    RecoverOperation {
        operation_id: OperationId,
        expected_revision: Revision,
        provider: ProviderKind,
    },
    NewAtLocation {
        project_id: ProjectId,
        location_id: LocationId,
        expected_project_revision: Revision,
        expected_location_revision: Revision,
        provider: ProviderKind,
    },
    NewAtSameLocation {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    Fork {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    SetProjectBrowserRoot {
        root_path: BrowserRootPath,
        expected_revision: Revision,
    },
    ListProjectBrowser {
        relative_path: BrowserPath,
        include_hidden: bool,
    },
    RegisterLocation {
        relative_path: BrowserPath,
        expected_browser_revision: Revision,
        provider: ProviderKind,
    },
    RefreshProject(ProjectRefreshRequest),
}

impl ApplicationAction {
    fn validate(&self) -> Result<(), ApplicationError> {
        match self {
            Self::Rename { name, .. } => validate_text(name, SnapshotEntity::NativeName),
            Self::SetProjectBrowserRoot { root_path, .. } => {
                BrowserRootPath::new(root_path.as_str().to_owned()).map(|_| ())
            }
            Self::ListProjectBrowser { relative_path, .. }
            | Self::RegisterLocation { relative_path, .. } => {
                BrowserPath::new(relative_path.as_str().to_owned()).map(|_| ())
            }
            Self::RefreshProject(request) => request.validate(),
            _ => Ok(()),
        }
    }

    fn observer_intent(&self) -> Option<ObserverIntent> {
        match *self {
            Self::Start {
                workstream_id,
                expected_revision,
                provider: ProviderKind::Codex,
            } => Some(ObserverIntent::Start {
                workstream_id,
                expected_revision,
                provider: ProviderKind::Codex,
            }),
            Self::Recover {
                workstream_id,
                expected_revision,
                provider: ProviderKind::Codex,
            } => Some(ObserverIntent::Recover {
                workstream_id,
                expected_revision,
                provider: ProviderKind::Codex,
            }),
            Self::RecoverOperation {
                operation_id,
                expected_revision,
                provider: ProviderKind::Codex,
            } => Some(ObserverIntent::RecoverOperation {
                operation_id,
                expected_revision,
                provider: ProviderKind::Codex,
            }),
            Self::RegisterLocation {
                expected_browser_revision,
                provider: ProviderKind::Codex,
                ..
            } => Some(ObserverIntent::RegisterLocation {
                expected_browser_revision,
                provider: ProviderKind::Codex,
            }),
            Self::NewAtLocation {
                project_id,
                location_id,
                expected_project_revision,
                expected_location_revision,
                provider: ProviderKind::Codex,
            } => Some(ObserverIntent::NewAtLocation {
                project_id,
                location_id,
                expected_project_revision,
                expected_location_revision,
                provider: ProviderKind::Codex,
            }),
            Self::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider: ProviderKind::Codex,
            } => Some(ObserverIntent::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider: ProviderKind::Codex,
            }),
            Self::Fork {
                source_workstream_id,
                expected_workstream_revision,
                provider: ProviderKind::Codex,
            } => Some(ObserverIntent::Fork {
                source_workstream_id,
                expected_workstream_revision,
                provider: ProviderKind::Codex,
            }),
            _ => None,
        }
    }
}

/// Result of an explicit application action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplicationOutcome {
    Applied {
        identity: RevisedIdentity,
    },
    Created {
        workstream_id: WorkstreamId,
        location_id: LocationId,
        revision: Revision,
    },
    BrowserListed(BrowserListing),
    ProjectRefreshed {
        project_id: ProjectId,
        revision: Revision,
    },
    ObserverReadinessRequired(ObserverReadinessGuide),
}

/// Identity and revision returned after a successful local mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisedIdentity {
    Project(ProjectId, Revision),
    Location(LocationId, Revision),
    Workstream(WorkstreamId, Revision),
    Runtime(RuntimeId, Revision),
    Attention(WorkstreamId, Revision),
    Operation(OperationId, Revision),
    ProjectBrowserRoot(Revision),
}

/// Exact opaque Workstream/Runtime evidence accepted for native attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachEvidence {
    pub workstream_id: WorkstreamId,
    pub runtime_id: RuntimeId,
    pub expected_workstream_revision: Revision,
    pub expected_runtime_revision: Revision,
}

/// Exact native-attachment result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachOutcome {
    pub workstream_id: WorkstreamId,
    pub runtime_id: RuntimeId,
}

/// Injected local state/action authority used by the host-local facade.
pub trait LocalApplicationBackend {
    /// Returns one passive, already-captured host-local projection.
    fn read_snapshot(&self) -> Result<SnapshotInput, ApplicationError>;

    /// Reads readiness only; it never prepares or reviews the observer.
    fn observer_readiness(&self) -> Result<ObserverReadinessEvidence, ApplicationError>;

    /// Applies one explicit typed local action.
    fn apply(&mut self, action: &ApplicationAction)
    -> Result<ApplicationOutcome, ApplicationError>;

    /// Delegates exact opaque local evidence to native attachment authority.
    fn attach(&mut self, evidence: &AttachEvidence) -> Result<AttachOutcome, ApplicationError>;
}

/// The production host-local backend for the D16 facade.
///
/// The backend owns a [`StateRoot`] but deliberately does not keep a mutable
/// registry connection across calls.  Every operation opens a fresh,
/// validated schema-13 handle, so a redraw observes current durable state and
/// a failed action cannot leave a stale connection as its authority.  The
/// constructor is read-only: schema migration, cutover, profile preparation,
/// and state-root creation remain outside this boundary.
pub struct HostRegistryApplicationBackend {
    root: StateRoot,
    host_id: HostId,
    installation_probe: provider::InstallationProbeCache,
    codex_home: Option<PathBuf>,
}

impl HostRegistryApplicationBackend {
    /// Opens one existing schema-13 host state root without running cutover or
    /// creating fresh state.
    pub fn open(root: StateRoot) -> Result<Self, ApplicationError> {
        let host_id = Self::validate_root(&root)?;
        Ok(Self {
            root,
            host_id,
            installation_probe: provider::InstallationProbeCache::probe(),
            codex_home: configured_codex_home(),
        })
    }

    /// Opens one existing schema-13 root with caller-supplied static
    /// installation evidence.  Production startup uses [`Self::open`]; this
    /// seam keeps focused tests from spawning provider probes.
    pub fn open_with_installation_cache(
        root: StateRoot,
        installation_probe: provider::InstallationProbeCache,
    ) -> Result<Self, ApplicationError> {
        let host_id = Self::validate_root(&root)?;
        Ok(Self {
            root,
            host_id,
            installation_probe,
            codex_home: configured_codex_home(),
        })
    }

    /// Opens one existing schema-13 root with deterministic static and
    /// observer-profile evidence.  The profile path is read only when no
    /// durable integration ownership row exists; this keeps disposable tests
    /// out of the operator's ordinary `CODEX_HOME`.
    pub fn open_with_installation_cache_and_codex_home(
        root: StateRoot,
        installation_probe: provider::InstallationProbeCache,
        codex_home: Option<PathBuf>,
    ) -> Result<Self, ApplicationError> {
        let host_id = Self::validate_root(&root)?;
        Ok(Self {
            root,
            host_id,
            installation_probe,
            codex_home,
        })
    }

    fn validate_root(root: &StateRoot) -> Result<HostId, ApplicationError> {
        let state = state::open_current_only(root)
            .map_err(|error| map_state_error(error, Authority::Snapshot))?;
        let registry = state
            .into_host_registry()
            .map_err(|error| map_state_error(error, Authority::Snapshot))?;
        let host_id = registry
            .identity()
            .map_err(|error| map_state_error(error, Authority::Snapshot))?
            .host_id;
        Ok(host_id)
    }

    /// Returns the registry identity captured at open time.
    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    /// Returns the owned state root after the backend is no longer needed.
    #[must_use]
    pub fn into_state_root(self) -> StateRoot {
        self.root
    }

    fn open_state(&self, authority: Authority) -> Result<D16State, ApplicationError> {
        state::open_current_only(&self.root).map_err(|error| map_state_error(error, authority))
    }

    fn open_registry(&self, authority: Authority) -> Result<HostRegistry, ApplicationError> {
        let state = self.open_state(authority)?;
        let registry = state
            .into_host_registry()
            .map_err(|error| map_state_error(error, authority))?;
        let identity = registry
            .identity()
            .map_err(|error| map_state_error(error, authority))?;
        if identity.host_id != self.host_id {
            return Err(ApplicationError::UnknownLocalIdentity);
        }
        Ok(registry)
    }

    fn read_browser_metadata(
        &self,
        authority: Authority,
    ) -> Result<(PathBuf, String, Revision), ApplicationError> {
        let path = self.root.host_database_path();
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| authority.failure())?;
        let configured = connection
            .query_row(
                "SELECT root_path, revision FROM project_browser_settings
                 WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|_| authority.failure())?;
        let (root, revision) = if let Some((root, revision)) = configured {
            if !valid_browser_root_text(&root) {
                return Err(authority.failure());
            }
            (
                PathBuf::from(root),
                Revision::try_from(revision).map_err(|_| authority.failure())?,
            )
        } else {
            let root = env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| authority.failure())?;
            let root_text = root.to_str().ok_or_else(|| authority.failure())?;
            if !valid_browser_root_text(root_text) {
                return Err(authority.failure());
            }
            (root, Revision::INITIAL)
        };
        let label = browser_root_label(&root);
        validate_text(&label, SnapshotEntity::DisplayText).map_err(|_| authority.failure())?;
        Ok((root, label, revision))
    }

    fn bounded_workstreams(
        &self,
        registry: &HostRegistry,
        authority: Authority,
    ) -> Result<Vec<state::WorkstreamOverview>, ApplicationError> {
        registry
            .workstream_overviews()
            .map_err(|error| match error {
                StateError::NavigatorSnapshotTooLarge => ApplicationError::SnapshotOverLimit {
                    kind: SnapshotLimitKind::Workstreams,
                    limit: MAX_SNAPSHOT_WORKSTREAMS,
                },
                error => map_state_error(error, authority),
            })
    }

    /// Reads only each current Runtime's exact generation-derived degraded
    /// marker.  No run-tree scan or marker payload crosses the application
    /// boundary.
    fn degraded_runtime_ids(
        &self,
        workstreams: &[state::WorkstreamOverview],
        authority: Authority,
    ) -> Result<BTreeSet<RuntimeId>, ApplicationError> {
        let mut degraded_runtimes = BTreeSet::new();
        for workstream in workstreams {
            let Some(runtime) = workstream.runtime.as_ref() else {
                continue;
            };
            if state::read_observer_degraded_marker(
                self.root.base(),
                runtime.runtime_id,
                &runtime.tmux_generation,
            )
            .map_err(|error| map_state_error(error, authority))?
            .is_some()
            {
                degraded_runtimes.insert(runtime.runtime_id);
            }
        }
        Ok(degraded_runtimes)
    }

    /// Fails closed for an observer-dependent action on the one exact current
    /// Runtime generation carrying degraded evidence. Unrelated Runtimes and
    /// fresh provider launches retain their own independent authority.
    fn ensure_workstream_observer_available(
        &self,
        workstream: &state::WorkstreamOverview,
        authority: Authority,
    ) -> Result<(), ApplicationError> {
        let Some(runtime) = workstream.runtime.as_ref() else {
            return Ok(());
        };
        if state::read_observer_degraded_marker(
            self.root.base(),
            runtime.runtime_id,
            &runtime.tmux_generation,
        )
        .map_err(|error| map_state_error(error, authority))?
        .is_some()
        {
            return Err(ApplicationError::ObserverUnavailable {
                readiness: ObserverReadiness::Unknown,
            });
        }
        Ok(())
    }

    fn bounded_operations(
        &self,
        registry: &HostRegistry,
        authority: Authority,
    ) -> Result<Vec<state::OperationOverview>, ApplicationError> {
        let operations =
            registry
                .unresolved_operation_overviews()
                .map_err(|error| match error {
                    StateError::NavigatorSnapshotTooLarge => ApplicationError::SnapshotOverLimit {
                        kind: SnapshotLimitKind::Operations,
                        limit: MAX_SNAPSHOT_OPERATIONS,
                    },
                    error => map_state_error(error, authority),
                })?;
        if operations.len() > MAX_SNAPSHOT_OPERATIONS {
            return Err(ApplicationError::SnapshotOverLimit {
                kind: SnapshotLimitKind::Operations,
                limit: MAX_SNAPSHOT_OPERATIONS,
            });
        }
        Ok(operations)
    }

    fn bounded_projects(
        &self,
        state: &D16State,
        authority: Authority,
    ) -> Result<Vec<state::ProjectProjection>, ApplicationError> {
        let projects = state
            .project_projections()
            .map_err(|error| map_state_error(error, authority))?;
        if projects.len() > MAX_SNAPSHOT_PROJECTS {
            return Err(ApplicationError::SnapshotOverLimit {
                kind: SnapshotLimitKind::Projects,
                limit: MAX_SNAPSHOT_PROJECTS,
            });
        }
        let locations = projects
            .iter()
            .map(|project| project.locations.len())
            .try_fold(0_usize, usize::checked_add)
            .ok_or(ApplicationError::SnapshotOverLimit {
                kind: SnapshotLimitKind::Locations,
                limit: MAX_SNAPSHOT_LOCATIONS,
            })?;
        if locations > MAX_SNAPSHOT_LOCATIONS {
            return Err(ApplicationError::SnapshotOverLimit {
                kind: SnapshotLimitKind::Locations,
                limit: MAX_SNAPSHOT_LOCATIONS,
            });
        }
        Ok(projects)
    }

    fn workstream(
        &self,
        registry: &HostRegistry,
        workstream_id: WorkstreamId,
        expected_revision: Option<Revision>,
        provider: Option<ProviderKind>,
    ) -> Result<state::WorkstreamOverview, ApplicationError> {
        self.workstream_with_authority(
            registry,
            workstream_id,
            expected_revision,
            provider,
            Authority::Action,
        )
    }

    fn workstream_with_authority(
        &self,
        registry: &HostRegistry,
        workstream_id: WorkstreamId,
        expected_revision: Option<Revision>,
        provider: Option<ProviderKind>,
        authority: Authority,
    ) -> Result<state::WorkstreamOverview, ApplicationError> {
        let workstreams = self.bounded_workstreams(registry, authority)?;
        let workstream = workstreams
            .into_iter()
            .find(|workstream| workstream.workstream_id == workstream_id)
            .ok_or(ApplicationError::UnknownLocalIdentity)?;
        if let Some(expected_revision) = expected_revision
            && workstream.revision != expected_revision
        {
            return Err(ApplicationError::StaleRevision {
                subject: RevisionSubject::Workstream(workstream_id),
                expected: expected_revision,
                current: workstream.revision,
            });
        }
        if let Some(provider) = provider
            && workstream.provider != provider
        {
            return Err(ApplicationError::ProviderIdentityMismatch);
        }
        Ok(workstream)
    }

    fn operation_provider(
        &self,
        _registry: &HostRegistry,
        operation: &state::OperationOverview,
        _authority: Authority,
    ) -> Result<ProviderKind, ApplicationError> {
        // `OperationOverview::provider` is decoded and bounded by the state
        // projection under this same validated registry connection.  The
        // application boundary must not reinterpret private effect evidence
        // or guess a provider from an operation kind.
        Ok(operation.provider)
    }

    fn refresh_project(
        &self,
        request: &ProjectRefreshRequest,
    ) -> Result<ApplicationOutcome, ApplicationError> {
        let _registry = self.open_registry(Authority::Action)?;
        let state = self.open_state(Authority::Action)?;
        let projections = self.bounded_projects(&state, Authority::Action)?;
        let selected = projections
            .iter()
            .find(|project| project.project_id == request.project_id)
            .ok_or(ApplicationError::UnknownLocalIdentity)?;
        if selected.revision != request.expected_project_revision {
            return Err(ApplicationError::StaleRevision {
                subject: RevisionSubject::Project(request.project_id),
                expected: request.expected_project_revision,
                current: selected.revision,
            });
        }
        let capture = state
            .capture_project_refresh(request.project_id)
            .map_err(|error| map_state_error(error, Authority::Action))?;
        if capture.project_revision != request.expected_project_revision {
            return Err(ApplicationError::StaleRevision {
                subject: RevisionSubject::Project(request.project_id),
                expected: request.expected_project_revision,
                current: capture.project_revision,
            });
        }
        let mut members = Vec::with_capacity(capture.members.len());
        for member in capture.members {
            let observation = repository::inspect(&member.repository_path)
                .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
            members.push(state::ProjectRefreshMember {
                location_id: member.location_id,
                expected_revision: member.expected_revision,
                observation: state::ProjectRefreshObservation {
                    display_name: observation.display_name,
                    repository_fingerprint: observation.remote_identity_fingerprint,
                    remote_identity_display: observation.remote_identity_display,
                },
            });
        }

        let input = state::ProjectRefreshInput {
            selected_project_id: capture.project_id,
            selected_project_revision: capture.project_revision,
            members,
        };
        let mut state = self.open_state(Authority::Action)?;
        let outcome = state
            .refresh_project(&input, &RandomIdGenerator)
            .map_err(|error| map_state_error(error, Authority::Action))?;
        let selected = outcome
            .selected_project
            .ok_or(ApplicationError::ActionAuthorityFailed)?;
        Ok(ApplicationOutcome::ProjectRefreshed {
            project_id: selected.project_id,
            revision: selected.revision,
        })
    }

    fn apply_action(
        &mut self,
        action: &ApplicationAction,
    ) -> Result<ApplicationOutcome, ApplicationError> {
        action.validate()?;
        if let ApplicationAction::RefreshProject(request) = action {
            return self.refresh_project(request);
        }

        let mut registry = self.open_registry(Authority::Action)?;
        match action {
            ApplicationAction::AcknowledgeAttention {
                workstream_id,
                expected_revision,
                kind: AttentionKind::Result,
            } => {
                let attention = registry
                    .attention(*workstream_id)
                    .map_err(|error| map_state_error(error, Authority::Action))?
                    .ok_or(ApplicationError::UnknownLocalIdentity)?;
                if attention.revision != *expected_revision {
                    return Err(ApplicationError::StaleRevision {
                        subject: RevisionSubject::Attention(*workstream_id),
                        expected: *expected_revision,
                        current: attention.revision,
                    });
                }
                let next = registry
                    .acknowledge_result_attention(*workstream_id, *expected_revision)
                    .map_err(|error| map_state_error(error, Authority::Action))?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Attention(*workstream_id, next.revision),
                })
            }
            ApplicationAction::Park {
                workstream_id,
                expected_revision,
            } => {
                self.workstream(&registry, *workstream_id, Some(*expected_revision), None)?;
                let revision = actions::park(
                    &self.root,
                    &mut registry,
                    *workstream_id,
                    Some(*expected_revision),
                )
                .map_err(map_action_error)?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Workstream(*workstream_id, revision),
                })
            }
            ApplicationAction::Archive {
                workstream_id,
                expected_revision,
            } => {
                self.workstream(&registry, *workstream_id, Some(*expected_revision), None)?;
                let revision = actions::archive(
                    &self.root,
                    &mut registry,
                    *workstream_id,
                    *expected_revision,
                )
                .map_err(map_action_error)?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Workstream(*workstream_id, revision),
                })
            }
            ApplicationAction::Restore {
                workstream_id,
                expected_revision,
            } => {
                self.workstream(&registry, *workstream_id, Some(*expected_revision), None)?;
                let revision = actions::restore(&mut registry, *workstream_id, *expected_revision)
                    .map_err(map_action_error)?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Workstream(*workstream_id, revision),
                })
            }
            ApplicationAction::Rename {
                workstream_id,
                expected_revision,
                name,
            } => {
                self.workstream(
                    &registry,
                    *workstream_id,
                    Some(*expected_revision),
                    Some(ProviderKind::Codex),
                )?;
                actions::rename(&mut registry, *workstream_id, *expected_revision, name)
                    .map_err(map_action_error)?;
                let current = self.workstream(&registry, *workstream_id, None, None)?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Workstream(*workstream_id, current.revision),
                })
            }
            ApplicationAction::Start {
                workstream_id,
                expected_revision,
                provider,
            } => {
                let workstream = self.workstream(
                    &registry,
                    *workstream_id,
                    Some(*expected_revision),
                    Some(*provider),
                )?;
                self.ensure_workstream_observer_available(&workstream, Authority::Action)?;
                actions::start(
                    &self.root,
                    &mut registry,
                    *workstream_id,
                    Some(*expected_revision),
                )
                .map_err(map_action_error)?;
                let current = self.workstream(&registry, *workstream_id, None, None)?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Workstream(*workstream_id, current.revision),
                })
            }
            ApplicationAction::Recover {
                workstream_id,
                expected_revision,
                provider,
            } => {
                let workstream = self.workstream(
                    &registry,
                    *workstream_id,
                    Some(*expected_revision),
                    Some(*provider),
                )?;
                self.ensure_workstream_observer_available(&workstream, Authority::Action)?;
                actions::recover(
                    &self.root,
                    &mut registry,
                    *workstream_id,
                    Some(*expected_revision),
                )
                .map_err(map_action_error)?;
                let current = self.workstream(&registry, *workstream_id, None, None)?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::Workstream(*workstream_id, current.revision),
                })
            }
            ApplicationAction::RecoverOperation {
                operation_id,
                expected_revision,
                provider,
            } => {
                let operation = self
                    .bounded_operations(&registry, Authority::Action)?
                    .into_iter()
                    .find(|operation| operation.operation_id == *operation_id)
                    .ok_or(ApplicationError::UnknownLocalIdentity)?;
                if operation.revision != *expected_revision {
                    return Err(ApplicationError::StaleRevision {
                        subject: RevisionSubject::Operation(*operation_id),
                        expected: *expected_revision,
                        current: operation.revision,
                    });
                }
                let operation_provider =
                    self.operation_provider(&registry, &operation, Authority::Action)?;
                if operation_provider != *provider {
                    return Err(ApplicationError::ProviderIdentityMismatch);
                }
                if operation.kind != OperationKind::Fork {
                    return Err(ApplicationError::UnsupportedAction);
                }
                let source_workstream_id = operation
                    .source_workstream_id
                    .ok_or(ApplicationError::UnknownLocalIdentity)?;
                let source =
                    self.workstream(&registry, source_workstream_id, None, Some(*provider))?;
                self.ensure_workstream_observer_available(&source, Authority::Action)?;
                let workstream_id =
                    actions::recover_managed_operation(&self.root, &mut registry, *operation_id)
                        .map_err(map_action_error)?;
                let current = self.workstream(&registry, workstream_id, None, Some(*provider))?;
                Ok(ApplicationOutcome::Created {
                    workstream_id,
                    location_id: current.location_id,
                    revision: current.revision,
                })
            }
            ApplicationAction::NewAtLocation {
                project_id,
                location_id,
                expected_project_revision,
                expected_location_revision,
                provider,
            } => {
                provider::require_new_eligible(&registry, *provider)
                    .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
                let request_key = format!(
                    "d16-app:new-location:{}:{}:{}:{}:{}",
                    project_id,
                    location_id,
                    expected_project_revision.value(),
                    expected_location_revision.value(),
                    provider
                );
                let created = registry
                    .create_independent_workstream_at_location(
                        *project_id,
                        *location_id,
                        *expected_project_revision,
                        *expected_location_revision,
                        &request_key,
                        *provider,
                    )
                    .map_err(|error| map_state_error(error, Authority::Action))?;
                let _ = actions::start(
                    &self.root,
                    &mut registry,
                    created.workstream_id,
                    Some(created.revision),
                )
                .map_err(map_action_error)?;
                let current =
                    self.workstream(&registry, created.workstream_id, None, Some(*provider))?;
                Ok(ApplicationOutcome::Created {
                    workstream_id: current.workstream_id,
                    location_id: current.location_id,
                    revision: current.revision,
                })
            }
            ApplicationAction::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            } => {
                self.workstream(
                    &registry,
                    *source_workstream_id,
                    Some(*expected_workstream_revision),
                    None,
                )?;
                let request_key = format!(
                    "d16-app:new-same:{}:{}:{}",
                    source_workstream_id,
                    expected_workstream_revision.value(),
                    provider
                );
                let workstream_id = actions::start_independent_workstream(
                    &self.root,
                    &mut registry,
                    *source_workstream_id,
                    Some(*expected_workstream_revision),
                    &request_key,
                    *provider,
                )
                .map_err(map_action_error)?;
                let current = self.workstream(&registry, workstream_id, None, Some(*provider))?;
                Ok(ApplicationOutcome::Created {
                    workstream_id,
                    location_id: current.location_id,
                    revision: current.revision,
                })
            }
            ApplicationAction::Fork {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            } => {
                let source = self.workstream(
                    &registry,
                    *source_workstream_id,
                    Some(*expected_workstream_revision),
                    Some(*provider),
                )?;
                self.ensure_workstream_observer_available(&source, Authority::Action)?;
                let request_key = format!(
                    "d16-app:fork:{}:{}",
                    source_workstream_id,
                    expected_workstream_revision.value()
                );
                let workstream_id = actions::fork_workstream(
                    &self.root,
                    &mut registry,
                    *source_workstream_id,
                    Some(*expected_workstream_revision),
                    request_key,
                )
                .map_err(map_action_error)?;
                let current = self.workstream(&registry, workstream_id, None, Some(*provider))?;
                let _ = source;
                Ok(ApplicationOutcome::Created {
                    workstream_id,
                    location_id: current.location_id,
                    revision: current.revision,
                })
            }
            ApplicationAction::SetProjectBrowserRoot {
                root_path,
                expected_revision,
            } => {
                let mut state = self.open_state(Authority::Action)?;
                let current = state
                    .project_browser_root_revision()
                    .map_err(|error| map_state_error(error, Authority::Action))?;
                if current != *expected_revision {
                    return Err(ApplicationError::StaleRevision {
                        subject: RevisionSubject::ProjectBrowserRoot,
                        expected: *expected_revision,
                        current,
                    });
                }
                let next = state
                    .set_project_browser_root(*expected_revision, root_path.as_str())
                    .map_err(|error| map_state_error(error, Authority::Action))?;
                Ok(ApplicationOutcome::Applied {
                    identity: RevisedIdentity::ProjectBrowserRoot(next.revision),
                })
            }
            ApplicationAction::ListProjectBrowser {
                relative_path,
                include_hidden,
            } => {
                let (expected_root, expected_label, expected_revision) =
                    self.read_browser_metadata(Authority::Action)?;
                let result = registry
                    .project_directories(relative_path.as_str(), *include_hidden)
                    .map_err(|error| map_state_error(error, Authority::Action))?;
                let (current_root, _, revision) = self.read_browser_metadata(Authority::Action)?;
                if revision != expected_revision {
                    return Err(ApplicationError::StaleRevision {
                        subject: RevisionSubject::ProjectBrowserRoot,
                        expected: expected_revision,
                        current: revision,
                    });
                }
                if current_root != expected_root {
                    return Err(ApplicationError::ActionAuthorityFailed);
                }
                if result.root_label != expected_label
                    || result.relative_path != relative_path.as_str()
                    || result.include_hidden != *include_hidden
                {
                    return Err(ApplicationError::InvalidBrowserListing);
                }
                let entries = result
                    .entries
                    .into_iter()
                    .map(|entry| BrowserEntry {
                        name: entry.name,
                        is_git_repository: entry.is_git_repository,
                    })
                    .collect();
                Ok(ApplicationOutcome::BrowserListed(BrowserListing {
                    root_label: result.root_label,
                    relative_path: BrowserPath::new(result.relative_path)
                        .map_err(|_| ApplicationError::InvalidBrowserListing)?,
                    include_hidden: result.include_hidden,
                    entries,
                    revision,
                }))
            }
            ApplicationAction::RegisterLocation {
                relative_path,
                expected_browser_revision,
                provider,
            } => {
                provider::require_new_eligible(&registry, *provider)
                    .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
                let (browser_root, _, current_revision) =
                    self.read_browser_metadata(Authority::Action)?;
                if current_revision != *expected_browser_revision {
                    return Err(ApplicationError::StaleRevision {
                        subject: RevisionSubject::ProjectBrowserRoot,
                        expected: *expected_browser_revision,
                        current: current_revision,
                    });
                }
                if relative_path.as_str().is_empty() {
                    return Err(ApplicationError::InvalidBrowserPath);
                }
                let canonical_root = fs::canonicalize(&browser_root)
                    .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
                let candidate = fs::canonicalize(browser_root.join(relative_path.as_str()))
                    .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
                if candidate == canonical_root || !candidate.starts_with(&canonical_root) {
                    return Err(ApplicationError::InvalidBrowserPath);
                }
                let inspected = repository::inspect(&candidate)
                    .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
                if inspected.project_root != candidate {
                    return Err(ApplicationError::ActionAuthorityFailed);
                }
                let (current_root, _, revalidated_revision) =
                    self.read_browser_metadata(Authority::Action)?;
                if revalidated_revision != *expected_browser_revision {
                    return Err(ApplicationError::StaleRevision {
                        subject: RevisionSubject::ProjectBrowserRoot,
                        expected: *expected_browser_revision,
                        current: revalidated_revision,
                    });
                }
                let revalidated_root = fs::canonicalize(current_root)
                    .map_err(|_| ApplicationError::ActionAuthorityFailed)?;
                if revalidated_root != canonical_root {
                    return Err(ApplicationError::ActionAuthorityFailed);
                }
                let mut state = self.open_state(Authority::Action)?;
                let registered = state
                    .register_project_location_with_initial_workstream(
                        &candidate,
                        &inspected.display_name,
                        inspected.remote_identity_fingerprint.as_deref(),
                        inspected.remote_identity_display.as_deref(),
                        *provider,
                        &RandomIdGenerator,
                    )
                    .map_err(|error| map_state_error(error, Authority::Action))?;
                Ok(ApplicationOutcome::Created {
                    workstream_id: registered.workstream.workstream_id,
                    location_id: registered.location_id,
                    revision: Revision::INITIAL,
                })
            }
            ApplicationAction::RefreshProject(_) => unreachable!("handled before opening registry"),
        }
    }
}

impl LocalApplicationBackend for HostRegistryApplicationBackend {
    fn read_snapshot(&self) -> Result<SnapshotInput, ApplicationError> {
        let state = self.open_state(Authority::Snapshot)?;
        let projections = self.bounded_projects(&state, Authority::Snapshot)?;
        let registry = state
            .into_host_registry()
            .map_err(|error| map_state_error(error, Authority::Snapshot))?;
        let identity = registry
            .identity()
            .map_err(|error| map_state_error(error, Authority::Snapshot))?;
        if identity.host_id != self.host_id {
            return Err(ApplicationError::UnknownLocalIdentity);
        }
        let workstreams = self.bounded_workstreams(&registry, Authority::Snapshot)?;
        let degraded_runtimes = self.degraded_runtime_ids(&workstreams, Authority::Snapshot)?;
        let operations = self.bounded_operations(&registry, Authority::Snapshot)?;
        let (_, root_label, browser_revision) = self.read_browser_metadata(Authority::Snapshot)?;
        let observer_readiness = observer_readiness(
            &registry,
            &self.root,
            self.codex_home.as_deref(),
            Authority::Snapshot,
        )?;
        let provider_capabilities = provider::discover_capabilities_with_installation_cache(
            &registry,
            self.installation_probe,
        )
        .map_err(|error| map_state_error(error, Authority::Snapshot))?
        .into_iter()
        .map(map_provider_capability)
        .collect::<Vec<_>>();
        let provider_capabilities =
            constrain_codex_capability(provider_capabilities, observer_readiness.readiness);

        let mut project_by_location = BTreeMap::new();
        let workstream_provider_by_id = workstreams
            .iter()
            .map(|workstream| (workstream.workstream_id, workstream.provider))
            .collect::<BTreeMap<_, _>>();
        let projects = projections
            .into_iter()
            .map(|project| {
                let locations = project
                    .locations
                    .into_iter()
                    .map(|location| {
                        project_by_location.insert(location.location_id, project.project_id);
                        LocationSnapshot {
                            project_id: project.project_id,
                            location_id: location.location_id,
                            display_name: location.display_name,
                            revision: location.revision,
                            repository_fingerprint: location.repository_fingerprint,
                            origin_display: location.origin_display,
                            is_label_source: location.is_label_source,
                        }
                    })
                    .collect();
                ProjectSnapshotInput {
                    project_id: project.project_id,
                    display_name: project.display_name,
                    revision: project.revision,
                    label_location_id: project.label_location_id,
                    repository_fingerprint: project.repository_fingerprint,
                    origin_display: None,
                    locations,
                }
            })
            .collect::<Vec<_>>();

        let workstreams = workstreams
            .into_iter()
            .map(|workstream| {
                let project_id = project_by_location
                    .get(&workstream.location_id)
                    .copied()
                    .ok_or(ApplicationError::SnapshotAuthorityFailed)?;
                if workstream
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.provider != workstream.provider)
                {
                    return Err(ApplicationError::ProviderIdentityMismatch);
                }
                let native_name = workstream
                    .binding
                    .as_ref()
                    .and_then(|binding| binding.observed_thread_name.clone())
                    .filter(|name| !name.is_empty());
                Ok(WorkstreamSnapshotInput {
                    project_id,
                    location_id: workstream.location_id,
                    workstream_id: workstream.workstream_id,
                    provider: workstream.provider,
                    lifecycle: workstream.lifecycle,
                    archived: workstream.archived_at_millis.is_some(),
                    last_activity_sequence: workstream.last_activity_sequence,
                    last_activity_at_millis: workstream.last_activity_at_millis,
                    revision: workstream.revision,
                    runtime: workstream.runtime.map(|runtime| RuntimeSnapshot {
                        observer_degraded: degraded_runtimes.contains(&runtime.runtime_id),
                        runtime_id: runtime.runtime_id,
                        status: runtime.status,
                        revision: runtime.revision,
                    }),
                    attention: workstream.attention.as_ref().map_or(
                        AttentionSnapshot {
                            result_unseen: false,
                            recovery_unseen: false,
                            revision: Revision::INITIAL,
                        },
                        AttentionSnapshot::from_state,
                    ),
                    native_name,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let unresolved_operations = operations
            .into_iter()
            .map(|operation| {
                let provider =
                    self.operation_provider(&registry, &operation, Authority::Snapshot)?;
                if operation.kind == OperationKind::Fork {
                    let source = operation.source_workstream_id.ok_or(
                        ApplicationError::InvalidSnapshotEntity {
                            entity: SnapshotEntity::Operation,
                        },
                    )?;
                    let source_provider = workstream_provider_by_id
                        .get(&source)
                        .copied()
                        .ok_or(ApplicationError::UnknownLocalIdentity)?;
                    if source_provider != provider {
                        return Err(ApplicationError::ProviderIdentityMismatch);
                    }
                }
                Ok(OperationSnapshot {
                    operation_id: operation.operation_id,
                    kind: operation.kind,
                    provider,
                    source_workstream_id: operation.source_workstream_id,
                    phase: operation.phase,
                    revision: operation.revision,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;

        Ok(SnapshotInput {
            projects,
            workstreams,
            unresolved_operations,
            observer_readiness,
            project_browser: ProjectBrowserSnapshot {
                root_label,
                revision: browser_revision,
            },
            provider_capabilities,
        })
    }

    fn observer_readiness(&self) -> Result<ObserverReadinessEvidence, ApplicationError> {
        let registry = self.open_registry(Authority::Action)?;
        observer_readiness(
            &registry,
            &self.root,
            self.codex_home.as_deref(),
            Authority::Action,
        )
    }

    fn apply(
        &mut self,
        action: &ApplicationAction,
    ) -> Result<ApplicationOutcome, ApplicationError> {
        self.apply_action(action)
    }

    fn attach(&mut self, evidence: &AttachEvidence) -> Result<AttachOutcome, ApplicationError> {
        let mut registry = self.open_registry(Authority::Attachment)?;
        let runtime = registry
            .runtime_by_id(evidence.runtime_id)
            .map_err(|error| map_state_error(error, Authority::Attachment))?
            .ok_or(ApplicationError::UnknownLocalIdentity)?;
        if runtime.workstream_id != evidence.workstream_id {
            return Err(ApplicationError::UnknownLocalIdentity);
        }
        if runtime.revision != evidence.expected_runtime_revision {
            return Err(ApplicationError::StaleRevision {
                subject: RevisionSubject::Runtime(evidence.runtime_id),
                expected: evidence.expected_runtime_revision,
                current: runtime.revision,
            });
        }
        self.workstream_with_authority(
            &registry,
            evidence.workstream_id,
            Some(evidence.expected_workstream_revision),
            Some(runtime.provider),
            Authority::Attachment,
        )?;
        let current =
            actions::preflight_attachment(&self.root, &mut registry, evidence.workstream_id)
                .map_err(map_action_error)?;
        if current.runtime_id != evidence.runtime_id {
            return Err(ApplicationError::UnknownLocalIdentity);
        }
        if current.revision != evidence.expected_runtime_revision {
            return Err(ApplicationError::StaleRevision {
                subject: RevisionSubject::Runtime(evidence.runtime_id),
                expected: evidence.expected_runtime_revision,
                current: current.revision,
            });
        }
        self.workstream_with_authority(
            &registry,
            evidence.workstream_id,
            Some(evidence.expected_workstream_revision),
            Some(current.provider),
            Authority::Attachment,
        )?;
        Ok(AttachOutcome {
            workstream_id: evidence.workstream_id,
            runtime_id: evidence.runtime_id,
        })
    }
}

/// Opens a host-local D16 application using the registry's authoritative `HostId`.
impl LocalApplication<HostRegistryApplicationBackend> {
    pub fn open_host_local(
        root: StateRoot,
        hostname: Option<String>,
    ) -> Result<Self, ApplicationError> {
        let backend = HostRegistryApplicationBackend::open(root)?;
        let host_id = backend.host_id();
        Ok(Self::new(backend, host_id, hostname))
    }
}

/// Compatibility-free descriptive aliases for callers that name the backend
/// by its state-root responsibility.
pub type StateRootApplicationBackend = HostRegistryApplicationBackend;
pub type LocalStateBackend = HostRegistryApplicationBackend;

#[derive(Clone, Copy)]
enum Authority {
    Snapshot,
    Action,
    Attachment,
}

impl Authority {
    const fn failure(self) -> ApplicationError {
        match self {
            Self::Snapshot => ApplicationError::SnapshotAuthorityFailed,
            Self::Action => ApplicationError::ActionAuthorityFailed,
            Self::Attachment => ApplicationError::AttachmentAuthorityFailed,
        }
    }
}

fn map_state_error(error: StateError, authority: Authority) -> ApplicationError {
    match error {
        StateError::ProviderIdentityMismatch => ApplicationError::ProviderIdentityMismatch,
        StateError::CutoverRequired => ApplicationError::CutoverRequired,
        StateError::FreshStateRequired => ApplicationError::FreshStateRequired,
        StateError::HostStateResetRequired(schema_version) => {
            ApplicationError::HostStateResetRequired { schema_version }
        }
        StateError::MalformedHostSchema => ApplicationError::MalformedHostSchema,
        StateError::UnsupportedFutureHostSchema(schema_version) => {
            ApplicationError::UnsupportedFutureHostSchema { schema_version }
        }
        StateError::StateRecoveryRequired(reason) => {
            ApplicationError::StateRecoveryRequired { reason }
        }
        StateError::FreshRootRejected(reason) => ApplicationError::FreshRootRejected { reason },
        _ => authority.failure(),
    }
}

fn map_action_error(error: actions::ActionError) -> ApplicationError {
    match error {
        actions::ActionError::OpenCodeProviderReadinessTimeout => ApplicationError::ActionFailed {
            reason: ActionFailureReason::ProviderReadinessTimeout,
        },
        actions::ActionError::OpenCodeObserverReadinessTimeout => ApplicationError::ActionFailed {
            reason: ActionFailureReason::OpenCodeObserverReadinessTimeout,
        },
        actions::ActionError::OpenCodeObserverStartupFailed => ApplicationError::ActionFailed {
            reason: ActionFailureReason::OpenCodeObserverStartupFailed,
        },
        actions::ActionError::OpenCodeObserverIdentityChanged => ApplicationError::ActionFailed {
            reason: ActionFailureReason::OpenCodeObserverIdentityChanged,
        },
        actions::ActionError::OpenCodeObserverExitedBeforeReady => ApplicationError::ActionFailed {
            reason: ActionFailureReason::OpenCodeObserverExitedBeforeReady,
        },
        actions::ActionError::RuntimeProbeAmbiguous => ApplicationError::ActionFailed {
            reason: ActionFailureReason::RuntimeEvidenceAmbiguous,
        },
        actions::ActionError::State(StateError::ProviderIdentityMismatch) => {
            ApplicationError::ProviderIdentityMismatch
        }
        actions::ActionError::State(error) => map_state_error(error, Authority::Action),
        _ => ApplicationError::ActionAuthorityFailed,
    }
}

fn observer_readiness(
    registry: &HostRegistry,
    state_root: &StateRoot,
    codex_home: Option<&Path>,
    authority: Authority,
) -> Result<ObserverReadinessEvidence, ApplicationError> {
    let integration = registry
        .codex_integration()
        .map_err(|error| map_state_error(error, authority))?;
    let profile = inspect_observer_profile(state_root, codex_home, integration.as_ref());
    let readiness = match profile {
        Ok(profile) => map_profile_readiness(integration.as_ref(), profile),
        Err(()) => ObserverReadiness::Unknown,
    };
    Ok(ObserverReadinessEvidence {
        readiness,
        integration_revision: integration.map(|integration| integration.revision),
    })
}

fn inspect_observer_profile(
    state_root: &StateRoot,
    configured_codex_home: Option<&Path>,
    integration: Option<&state::CodexIntegration>,
) -> Result<ProfileInspection, ()> {
    let (codex_home, hook_executable) = if let Some(integration) = integration {
        let codex_home = integration
            .ownership
            .canonical_path
            .parent()
            .ok_or(())?
            .to_path_buf();
        if !valid_configured_codex_home(&codex_home) {
            return Err(());
        }
        (codex_home, integration.ownership.hook_executable.clone())
    } else {
        let codex_home = configured_codex_home
            .map(Path::to_path_buf)
            .filter(|path| valid_configured_codex_home(path))
            .ok_or(())?;
        (codex_home, PathBuf::new())
    };
    let profile = ObserverProfile::new(codex_home, hook_executable, state_root.base());
    let metadata = match fs::symlink_metadata(profile.path()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return profile
                .inspect(integration.map(|integration| &integration.ownership))
                .map_err(|_| ());
        }
        Err(_) => return Err(()),
    };
    if metadata.len() > MAX_OBSERVER_PROFILE_BYTES {
        return Err(());
    }
    profile
        .inspect(integration.map(|integration| &integration.ownership))
        .map_err(|_| ())
}

fn map_profile_readiness(
    integration: Option<&state::CodexIntegration>,
    profile: ProfileInspection,
) -> ObserverReadiness {
    if matches!(
        integration.map(|integration| integration.lifecycle),
        Some(IntegrationLifecycle::Disabled)
    ) {
        return ObserverReadiness::Disabled;
    }
    match profile {
        ProfileInspection::Missing => ObserverReadiness::SetupRequired,
        ProfileInspection::Foreign => ObserverReadiness::Foreign,
        ProfileInspection::Modified => ObserverReadiness::Modified,
        ProfileInspection::UpdateRequired => ObserverReadiness::UpdateRequired,
        ProfileInspection::TrustPending => ObserverReadiness::TrustReviewRequired,
        ProfileInspection::Ready => match integration.map(|integration| integration.lifecycle) {
            Some(IntegrationLifecycle::Ready) | None => ObserverReadiness::Ready,
            Some(IntegrationLifecycle::Modified) => ObserverReadiness::Modified,
            Some(IntegrationLifecycle::TrustPending) => ObserverReadiness::TrustReviewRequired,
            Some(IntegrationLifecycle::Disabled) => ObserverReadiness::Disabled,
        },
    }
}

fn map_provider_capability(capability: crate::provider::ProviderCapability) -> ProviderCapability {
    let reason = match capability.reason {
        crate::provider::ProviderCapabilityReason::None => None,
        crate::provider::ProviderCapabilityReason::AdapterUnavailable => {
            Some(ProviderCapabilityReason::AdapterUnavailable)
        }
        crate::provider::ProviderCapabilityReason::NotInstalled => {
            Some(ProviderCapabilityReason::NotInstalled)
        }
        crate::provider::ProviderCapabilityReason::UnsupportedVersion => {
            Some(ProviderCapabilityReason::UnsupportedVersion)
        }
        crate::provider::ProviderCapabilityReason::ObserverNotReady => {
            Some(ProviderCapabilityReason::ObserverNotReady)
        }
        crate::provider::ProviderCapabilityReason::RuntimePrerequisiteMissing => {
            Some(ProviderCapabilityReason::RuntimePrerequisiteMissing)
        }
        crate::provider::ProviderCapabilityReason::ProbeFailed => {
            Some(ProviderCapabilityReason::ProbeFailed)
        }
    };
    ProviderCapability {
        provider: capability.kind,
        status: match capability.status {
            crate::provider::ProviderCapabilityStatus::Available => {
                ProviderCapabilityStatus::Available
            }
            crate::provider::ProviderCapabilityStatus::Unavailable => {
                ProviderCapabilityStatus::Unavailable
            }
            crate::provider::ProviderCapabilityStatus::Unknown => ProviderCapabilityStatus::Unknown,
        },
        reason,
        fresh_launch: capability.fresh_launch,
        exact_resume: capability.exact_resume,
        observe: capability.observe,
        metadata_read: capability.metadata_read,
        navigator_rename: capability.rename,
        fork: capability.fork,
    }
}

fn configured_codex_home() -> Option<PathBuf> {
    configured_codex_home_from_environment()
}

fn configured_codex_home_from_environment() -> Option<PathBuf> {
    let value = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
    valid_configured_codex_home(&value).then_some(value)
}

fn valid_configured_codex_home(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    path.is_absolute()
        && text.len() <= MAX_BROWSER_ROOT_BYTES
        && !text.contains('\0')
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        && !text
            .chars()
            .any(|character| character.is_control() || is_unicode_format(character))
}

fn constrain_codex_capability(
    mut capabilities: Vec<ProviderCapability>,
    readiness: ObserverReadiness,
) -> Vec<ProviderCapability> {
    if !matches!(readiness, ObserverReadiness::Ready) {
        for capability in &mut capabilities {
            if capability.provider == ProviderKind::Codex
                && capability.status == ProviderCapabilityStatus::Available
            {
                *capability = ProviderCapability {
                    provider: ProviderKind::Codex,
                    status: ProviderCapabilityStatus::Unavailable,
                    reason: Some(ProviderCapabilityReason::ObserverNotReady),
                    fresh_launch: false,
                    exact_resume: false,
                    observe: false,
                    metadata_read: false,
                    navigator_rename: false,
                    fork: false,
                };
            }
        }
    }
    capabilities
}

fn valid_browser_root_text(root: &str) -> bool {
    !root.is_empty()
        && root.len() <= MAX_BROWSER_ROOT_BYTES
        && Path::new(root).is_absolute()
        && !root.contains('\0')
        && !root
            .chars()
            .any(|character| character.is_control() || is_unicode_format(character))
        && (root == "/"
            || (!root.ends_with('/')
                && root.strip_prefix('/').is_some_and(|rest| {
                    !rest
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                })))
}

fn browser_root_label(root: &Path) -> String {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project root")
            .to_owned();
    };
    if let Ok(relative) = root.strip_prefix(home) {
        if relative.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", relative.to_string_lossy())
        }
    } else {
        root.file_name().and_then(|name| name.to_str()).map_or_else(
            || "custom project root".to_owned(),
            |name| format!("custom root · {name}"),
        )
    }
}

/// Typed host-local application facade.
pub struct LocalApplication<B> {
    backend: B,
    host_id: HostId,
    hostname: Option<String>,
    limits: SnapshotLimits,
}

impl<B> LocalApplication<B> {
    /// Builds a facade with production hard bounds.
    #[must_use]
    pub fn new(backend: B, host_id: HostId, hostname: Option<String>) -> Self {
        Self {
            backend,
            host_id,
            hostname,
            limits: SnapshotLimits::default(),
        }
    }

    /// Convenience constructor for borrowed hostname samples.
    #[must_use]
    pub fn with_hostname(backend: B, host_id: HostId, hostname: Option<&str>) -> Self {
        Self::new(backend, host_id, hostname.map(str::to_owned))
    }

    /// Uses explicit projection bounds for a focused test.
    #[must_use]
    pub fn with_limits(mut self, limits: SnapshotLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Returns the injected backend.
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Returns mutable access to the injected backend.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Returns the display-only current-host label.
    #[must_use]
    pub fn host_display(&self) -> String {
        derived_host_label(self.host_id, self.hostname.as_deref())
    }
}

impl<B: LocalApplicationBackend> LocalApplication<B> {
    /// Builds one deterministic, bounded in-memory projection.
    pub fn snapshot(&self) -> Result<ApplicationSnapshot, ApplicationError> {
        let input = self.backend.read_snapshot()?;
        build_snapshot(self.host_id, self.hostname.as_deref(), input, self.limits)
    }

    /// Applies one typed action, returning readiness guidance before any
    /// observer-dependent backend mutation.
    pub fn apply(
        &mut self,
        action: ApplicationAction,
    ) -> Result<ApplicationOutcome, ApplicationError> {
        action.validate()?;
        if let Some(intent) = action.observer_intent() {
            let evidence = self.backend.observer_readiness()?;
            if evidence.needs_guide() {
                return Ok(ApplicationOutcome::ObserverReadinessRequired(
                    ObserverReadinessGuide {
                        evidence,
                        intent,
                        explicit_interactive_consent_required: true,
                        native_trust_review_required: true,
                    },
                ));
            }
            if !matches!(evidence.readiness, ObserverReadiness::Ready) {
                return Err(ApplicationError::ObserverUnavailable {
                    readiness: evidence.readiness,
                });
            }
        }
        let mut outcome = self.backend.apply(&action)?;
        if let ApplicationOutcome::BrowserListed(listing) = &mut outcome {
            validate_and_sort_browser_listing(listing)?;
        }
        Ok(outcome)
    }

    /// Delegates exact opaque Workstream/Runtime evidence to native attach.
    pub fn attach(&mut self, evidence: AttachEvidence) -> Result<AttachOutcome, ApplicationError> {
        let outcome = self.backend.attach(&evidence)?;
        if outcome.workstream_id != evidence.workstream_id
            || outcome.runtime_id != evidence.runtime_id
        {
            return Err(ApplicationError::AttachmentEvidenceMismatch);
        }
        Ok(outcome)
    }
}

/// Reads the current machine name from the operating-system hostname API.
/// Non-Unicode or unavailable values deliberately fall back through
/// [`derived_host_label`] rather than consulting mutable environment state.
#[must_use]
pub fn operating_system_hostname() -> Option<String> {
    #[cfg(unix)]
    {
        nix::unistd::gethostname().ok()?.into_string().ok()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Derives the display-only current-host label.
///
/// A trimmed hostname is accepted only when non-empty, single-line, no longer
/// than 64 Unicode scalar values, and free of Unicode control/format chars.
/// Otherwise the first eight lowercase UUID hex digits form `host-<HostId8>`.
#[must_use]
pub fn derived_host_label(host_id: HostId, os_hostname: Option<&str>) -> String {
    let candidate = os_hostname.map(str::trim).unwrap_or_default();
    let valid = !candidate.is_empty()
        && candidate.chars().count() <= 64
        && !candidate.chars().any(|character| {
            character.is_control()
                || is_unicode_format(character)
                || matches!(
                    character,
                    '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}'
                )
        });
    if valid {
        candidate.to_owned()
    } else {
        let hex = host_id.as_uuid().simple().to_string();
        format!("host-{}", &hex[..8])
    }
}

fn is_unicode_format(character: char) -> bool {
    matches!(
        character as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

fn build_snapshot(
    host_id: HostId,
    hostname: Option<&str>,
    mut input: SnapshotInput,
    limits: SnapshotLimits,
) -> Result<ApplicationSnapshot, ApplicationError> {
    validate_text(
        &input.project_browser.root_label,
        SnapshotEntity::DisplayText,
    )?;
    validate_observer_readiness(input.observer_readiness)?;
    check_limit(
        input.projects.len(),
        limits.projects,
        SnapshotLimitKind::Projects,
    )?;
    check_limit(
        input.workstreams.len(),
        limits.workstreams,
        SnapshotLimitKind::Workstreams,
    )?;
    check_limit(
        input.unresolved_operations.len(),
        limits.operations,
        SnapshotLimitKind::Operations,
    )?;
    check_limit(
        input.provider_capabilities.len(),
        limits.capabilities,
        SnapshotLimitKind::Capabilities,
    )?;
    let location_count = input
        .projects
        .iter()
        .map(|project| project.locations.len())
        .sum::<usize>();
    check_limit(
        location_count,
        limits.locations,
        SnapshotLimitKind::Locations,
    )?;

    let mut project_ids = BTreeSet::new();
    let mut location_ids = BTreeSet::new();
    for project in &mut input.projects {
        if !project_ids.insert(project.project_id) {
            return Err(ApplicationError::DuplicateSnapshotIdentity {
                entity: SnapshotEntity::Project,
            });
        }
        validate_text(&project.display_name, SnapshotEntity::DisplayText)?;
        if let Some(origin) = &project.origin_display {
            validate_text(origin, SnapshotEntity::DisplayText)?;
        }
        if let Some(fingerprint) = &project.repository_fingerprint {
            validate_opaque_metadata(fingerprint)?;
        }
        project
            .locations
            .sort_by_key(|location| location.location_id);
        let mut label_source_count = 0;
        for location in &project.locations {
            if location.project_id != project.project_id {
                return Err(ApplicationError::LocationProjectMismatch(
                    location.location_id,
                ));
            }
            if !location_ids.insert(location.location_id) {
                return Err(ApplicationError::DuplicateSnapshotIdentity {
                    entity: SnapshotEntity::Location,
                });
            }
            validate_text(&location.display_name, SnapshotEntity::DisplayText)?;
            if let Some(origin) = &location.origin_display {
                validate_text(origin, SnapshotEntity::DisplayText)?;
            }
            if let Some(fingerprint) = &location.repository_fingerprint {
                validate_opaque_metadata(fingerprint)?;
            }
            if location.is_label_source {
                label_source_count += 1;
                if location.location_id != project.label_location_id {
                    return Err(ApplicationError::InvalidSnapshotEntity {
                        entity: SnapshotEntity::Location,
                    });
                }
            }
        }
        if label_source_count != 1
            || !project.locations.iter().any(|location| {
                location.location_id == project.label_location_id && location.is_label_source
            })
        {
            return Err(ApplicationError::InvalidSnapshotEntity {
                entity: SnapshotEntity::Location,
            });
        }
    }

    let mut workstream_ids = BTreeSet::new();
    for workstream in &input.workstreams {
        if !project_ids.contains(&workstream.project_id) {
            return Err(ApplicationError::UnknownSnapshotProject(
                workstream.project_id,
            ));
        }
        if !location_ids.contains(&workstream.location_id) {
            return Err(ApplicationError::UnknownSnapshotLocation(
                workstream.location_id,
            ));
        }
        let location_project = input
            .projects
            .iter()
            .flat_map(|project| project.locations.iter())
            .find(|location| location.location_id == workstream.location_id)
            .map(|location| location.project_id);
        if location_project != Some(workstream.project_id) {
            return Err(ApplicationError::WorkstreamProjectMismatch(
                workstream.workstream_id,
            ));
        }
        if !workstream_ids.insert(workstream.workstream_id) {
            return Err(ApplicationError::DuplicateSnapshotIdentity {
                entity: SnapshotEntity::Workstream,
            });
        }
        if workstream.last_activity_sequence < 0 {
            return Err(ApplicationError::InvalidSnapshotEntity {
                entity: SnapshotEntity::Workstream,
            });
        }
        if workstream
            .last_activity_at_millis
            .is_some_and(|timestamp| timestamp < 0)
        {
            return Err(ApplicationError::InvalidSnapshotEntity {
                entity: SnapshotEntity::Workstream,
            });
        }
        if let Some(name) = &workstream.native_name {
            validate_text(name, SnapshotEntity::NativeName)?;
        }
    }

    let mut operation_ids = BTreeSet::new();
    for operation in &input.unresolved_operations {
        if !operation_ids.insert(operation.operation_id) {
            return Err(ApplicationError::DuplicateSnapshotIdentity {
                entity: SnapshotEntity::Operation,
            });
        }
        if operation.phase.is_terminal() {
            return Err(ApplicationError::TerminalOperation(operation.operation_id));
        }
        if operation.kind == OperationKind::Fork
            && operation
                .source_workstream_id
                .is_none_or(|source| !workstream_ids.contains(&source))
        {
            return Err(ApplicationError::InvalidSnapshotEntity {
                entity: SnapshotEntity::Operation,
            });
        }
    }

    input
        .provider_capabilities
        .sort_by_key(|capability| capability.provider);
    for pair in input.provider_capabilities.windows(2) {
        if pair[0].provider == pair[1].provider {
            return Err(ApplicationError::DuplicateProviderCapability(
                pair[0].provider,
            ));
        }
    }
    for provider in [ProviderKind::Codex, ProviderKind::OpenCode] {
        let Some(capability) = input
            .provider_capabilities
            .iter()
            .find(|capability| capability.provider == provider)
        else {
            return Err(ApplicationError::MissingProviderCapability(provider));
        };
        validate_provider_capability(capability)?;
    }
    input
        .unresolved_operations
        .sort_by_key(|operation| operation.operation_id);

    let mut projects = input
        .projects
        .iter()
        .map(|project| ProjectSnapshot {
            project_id: project.project_id,
            display_name: project.display_name.clone(),
            revision: project.revision,
            label_location_id: project.label_location_id,
            repository_fingerprint: project.repository_fingerprint.clone(),
            origin_display: project.origin_display.clone(),
            locations: project.locations.clone(),
        })
        .collect::<Vec<_>>();
    projects.sort_by_key(|project| project.project_id);

    let mut active_project_groups = input
        .projects
        .iter()
        .filter_map(|project| workstream_group(&input, project.project_id, false))
        .collect::<Vec<_>>();
    let mut archived_project_groups = input
        .projects
        .iter()
        .filter_map(|project| workstream_group(&input, project.project_id, true))
        .collect::<Vec<_>>();
    active_project_groups
        .sort_by_key(|group| (Reverse(group.max_activity_sequence), group.project_id));
    archived_project_groups
        .sort_by_key(|group| (Reverse(group.max_activity_sequence), group.project_id));

    Ok(ApplicationSnapshot {
        host_id,
        host_display: derived_host_label(host_id, hostname),
        projects,
        active_project_groups,
        archived_project_groups,
        unresolved_operations: input.unresolved_operations,
        observer_readiness: input.observer_readiness,
        project_browser: input.project_browser,
        provider_capabilities: input.provider_capabilities,
    })
}

fn workstream_group(
    input: &SnapshotInput,
    project_id: ProjectId,
    archived: bool,
) -> Option<ProjectWorkstreamGroup> {
    let mut workstreams = input
        .workstreams
        .iter()
        .filter(|workstream| workstream.project_id == project_id && workstream.archived == archived)
        .map(|workstream| WorkstreamSnapshot {
            project_id: workstream.project_id,
            location_id: workstream.location_id,
            workstream_id: workstream.workstream_id,
            provider: workstream.provider,
            lifecycle: workstream.lifecycle,
            archived: workstream.archived,
            last_activity_sequence: workstream.last_activity_sequence,
            last_activity_at_millis: workstream.last_activity_at_millis,
            revision: workstream.revision,
            runtime: workstream.runtime,
            attention: workstream.attention,
            native_name: workstream.native_name.clone(),
        })
        .collect::<Vec<_>>();
    if workstreams.is_empty() {
        return None;
    }
    workstreams.sort_by_key(|workstream| {
        (
            Reverse(workstream.last_activity_sequence),
            workstream.workstream_id,
        )
    });
    let max_activity_sequence = workstreams
        .first()
        .map_or(0, |workstream| workstream.last_activity_sequence);
    Some(ProjectWorkstreamGroup {
        project_id,
        max_activity_sequence,
        workstreams,
    })
}

fn check_limit(
    actual: usize,
    limit: usize,
    kind: SnapshotLimitKind,
) -> Result<(), ApplicationError> {
    (actual <= limit)
        .then_some(())
        .ok_or(ApplicationError::SnapshotOverLimit { kind, limit })
}

fn validate_text(value: &str, entity: SnapshotEntity) -> Result<(), ApplicationError> {
    if value.is_empty()
        || value.chars().count() > MAX_TEXT_SCALARS
        || value
            .chars()
            .any(|character| character.is_control() || is_unicode_format(character))
    {
        return Err(ApplicationError::InvalidSnapshotEntity { entity });
    }
    Ok(())
}

fn validate_opaque_metadata(value: &str) -> Result<(), ApplicationError> {
    if value.is_empty()
        || value.len() > MAX_BROWSER_ROOT_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || is_unicode_format(character))
    {
        return Err(ApplicationError::InvalidSnapshotEntity {
            entity: SnapshotEntity::DisplayText,
        });
    }
    Ok(())
}

fn validate_provider_capability(capability: &ProviderCapability) -> Result<(), ApplicationError> {
    match capability.status {
        ProviderCapabilityStatus::Available => {
            if capability.reason.is_some() {
                return Err(ApplicationError::InvalidProviderCapability(
                    capability.provider,
                ));
            }
        }
        ProviderCapabilityStatus::Unavailable | ProviderCapabilityStatus::Unknown => {
            if capability.reason.is_none()
                || capability.fresh_launch
                || capability.exact_resume
                || capability.observe
                || capability.metadata_read
                || capability.navigator_rename
                || capability.fork
            {
                return Err(ApplicationError::InvalidProviderCapability(
                    capability.provider,
                ));
            }
        }
    }
    Ok(())
}

fn validate_observer_readiness(
    evidence: ObserverReadinessEvidence,
) -> Result<(), ApplicationError> {
    let valid = match evidence.readiness {
        ObserverReadiness::Ready
        | ObserverReadiness::TrustReviewRequired
        | ObserverReadiness::UpdateRequired
        | ObserverReadiness::Modified
        | ObserverReadiness::Disabled => evidence.integration_revision.is_some(),
        ObserverReadiness::SetupRequired => evidence.integration_revision.is_none(),
        ObserverReadiness::Foreign | ObserverReadiness::Ambiguous | ObserverReadiness::Unknown => {
            true
        }
    };
    valid
        .then_some(())
        .ok_or(ApplicationError::InvalidSnapshotEntity {
            entity: SnapshotEntity::ObserverReadiness,
        })
}

impl ProjectRefreshRequest {
    fn validate(&self) -> Result<(), ApplicationError> {
        Ok(())
    }
}

fn validate_and_sort_browser_listing(listing: &mut BrowserListing) -> Result<(), ApplicationError> {
    validate_text(&listing.root_label, SnapshotEntity::DisplayText)?;
    if listing.entries.len() > MAX_BROWSER_ENTRIES {
        return Err(ApplicationError::InvalidBrowserListing);
    }
    for entry in &listing.entries {
        if entry.name.is_empty()
            || entry.name.len() > MAX_BROWSER_RELATIVE_BYTES
            || entry.name.contains('/')
            || entry.name.contains('\\')
            || entry.name == "."
            || entry.name == ".."
            || entry
                .name
                .chars()
                .any(|character| character.is_control() || is_unicode_format(character))
        {
            return Err(ApplicationError::InvalidBrowserListing);
        }
    }
    listing.entries.sort_by_key(|entry| {
        (
            u8::from(!entry.is_git_repository),
            u8::from(!(listing.include_hidden && entry.name.starts_with('.'))),
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
    Ok(())
}

#[cfg(test)]
mod backend_tests {
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn state_readiness_failures_remain_typed_at_the_application_boundary() {
        let authority = Authority::Snapshot;
        assert_eq!(
            map_state_error(StateError::CutoverRequired, authority),
            ApplicationError::CutoverRequired
        );
        assert_eq!(
            map_state_error(StateError::FreshStateRequired, authority),
            ApplicationError::FreshStateRequired
        );
        assert_eq!(
            map_state_error(StateError::HostStateResetRequired(11), authority),
            ApplicationError::HostStateResetRequired { schema_version: 11 }
        );
        assert_eq!(
            map_state_error(
                StateError::StateRecoveryRequired(StateRecoveryReason::MissingHostDatabase),
                authority,
            ),
            ApplicationError::StateRecoveryRequired {
                reason: StateRecoveryReason::MissingHostDatabase,
            }
        );
        assert_eq!(
            map_state_error(StateError::UnsupportedFutureHostSchema(14), authority),
            ApplicationError::UnsupportedFutureHostSchema { schema_version: 14 }
        );
    }

    #[test]
    fn action_failures_preserve_bounded_provider_runtime_and_observer_reasons() {
        assert_eq!(
            map_action_error(actions::ActionError::OpenCodeProviderReadinessTimeout),
            ApplicationError::ActionFailed {
                reason: ActionFailureReason::ProviderReadinessTimeout,
            }
        );
        assert_eq!(
            map_action_error(actions::ActionError::OpenCodeObserverStartupFailed),
            ApplicationError::ActionFailed {
                reason: ActionFailureReason::OpenCodeObserverStartupFailed,
            }
        );
        assert_eq!(
            map_action_error(actions::ActionError::OpenCodeObserverReadinessTimeout),
            ApplicationError::ActionFailed {
                reason: ActionFailureReason::OpenCodeObserverReadinessTimeout,
            }
        );
        assert_eq!(
            map_action_error(actions::ActionError::OpenCodeObserverIdentityChanged),
            ApplicationError::ActionFailed {
                reason: ActionFailureReason::OpenCodeObserverIdentityChanged,
            }
        );
        assert_eq!(
            map_action_error(actions::ActionError::OpenCodeObserverExitedBeforeReady),
            ApplicationError::ActionFailed {
                reason: ActionFailureReason::OpenCodeObserverExitedBeforeReady,
            }
        );
        assert_eq!(
            map_action_error(actions::ActionError::RuntimeProbeAmbiguous),
            ApplicationError::ActionFailed {
                reason: ActionFailureReason::RuntimeEvidenceAmbiguous,
            }
        );
        assert_eq!(
            map_action_error(actions::ActionError::NativeRecoveryRequired),
            ApplicationError::ActionAuthorityFailed
        );
        assert_eq!(
            map_action_error(actions::ActionError::State(
                StateError::ProviderIdentityMismatch,
            )),
            ApplicationError::ProviderIdentityMismatch
        );
    }

    #[cfg(unix)]
    #[test]
    fn hostname_sample_comes_from_the_operating_system_api() {
        let expected = nix::unistd::gethostname()
            .expect("operating-system hostname")
            .into_string()
            .expect("UTF-8 hostname");
        assert_eq!(operating_system_hostname(), Some(expected));
    }

    #[test]
    fn fresh_schema13_backend_snapshot_is_passive_and_bounded() {
        let temporary = tempdir().expect("temporary state root");
        let root_path = temporary.path().join("state");
        let root = StateRoot::create(&root_path).expect("private state root");
        let state =
            state::fresh_create(&root_path, &RandomIdGenerator).expect("fresh schema 13 state");
        drop(state);
        let codex_home = temporary.path().join("codex-home");

        let backend = HostRegistryApplicationBackend::open_with_installation_cache_and_codex_home(
            root,
            provider::InstallationProbeCache::probe_with(
                |_| false,
                provider::opencode::InstallationProbe::NotInstalled,
            ),
            Some(codex_home),
        )
        .expect("open backend");
        let host_id = backend.host_id();
        let database_before = fs::read(root_path.join("host.sqlite")).expect("database bytes");
        let root_entries_before = fs::read_dir(&root_path)
            .expect("state root entries")
            .map(|entry| entry.expect("state entry").file_name())
            .collect::<BTreeSet<_>>();
        let input = backend.read_snapshot().expect("passive snapshot");
        assert_eq!(
            fs::read(root_path.join("host.sqlite")).expect("database after snapshot"),
            database_before
        );
        let root_entries_after = fs::read_dir(&root_path)
            .expect("state root entries after snapshot")
            .map(|entry| entry.expect("state entry").file_name())
            .collect::<BTreeSet<_>>();
        assert_eq!(root_entries_after, root_entries_before);
        assert!(input.projects.is_empty());
        assert!(input.workstreams.is_empty());
        assert!(input.unresolved_operations.is_empty());
        assert_eq!(input.provider_capabilities.len(), 2);
        assert_eq!(input.project_browser.revision, Revision::INITIAL);

        let application = LocalApplication::new(backend, host_id, None);
        let snapshot = application
            .snapshot()
            .expect("bounded application snapshot");
        assert_eq!(snapshot.host_id, host_id);
        assert!(snapshot.active_project_groups.is_empty());
        assert!(snapshot.archived_project_groups.is_empty());
    }

    #[test]
    fn snapshot_marks_only_the_current_runtime_generation_degraded() {
        let temporary = tempdir().expect("temporary state root");
        let root_path = temporary.path().join("state");
        let mut state =
            state::fresh_create(&root_path, &RandomIdGenerator).expect("fresh schema 13 state");
        let registration = state
            .register_project_location_with_initial_workstream(
                Path::new("/fixture/project"),
                "fixture project",
                None,
                None,
                ProviderKind::OpenCode,
                &RandomIdGenerator,
            )
            .expect("registered project");
        let mut registry = state.into_host_registry().expect("current registry");
        let runtime = registry
            .reserve_runtime(registration.workstream.workstream_id)
            .expect("reserved runtime");
        drop(registry);
        let root = StateRoot::select(&root_path);
        state::write_observer_degraded_marker(
            root.base(),
            runtime.runtime_id,
            &runtime.tmux_generation,
            state::ObserverDegradedReason::BusyDeadline,
        )
        .expect("current degraded marker");
        let backend = HostRegistryApplicationBackend::open_with_installation_cache(
            root,
            provider::InstallationProbeCache::probe_with(
                |program| program == "tmux",
                provider::opencode::InstallationProbe::Available,
            ),
        )
        .expect("open backend");
        let registry = backend
            .open_registry(Authority::Action)
            .expect("current registry");
        let current = backend
            .workstream(
                &registry,
                registration.workstream.workstream_id,
                None,
                Some(ProviderKind::OpenCode),
            )
            .expect("current Workstream");
        assert!(matches!(
            backend.ensure_workstream_observer_available(&current, Authority::Action),
            Err(ApplicationError::ObserverUnavailable {
                readiness: ObserverReadiness::Unknown
            })
        ));
        let mut unaffected = current.clone();
        let unaffected_runtime = unaffected.runtime.as_mut().expect("current Runtime");
        unaffected_runtime.runtime_id = RuntimeId::from(uuid::Uuid::new_v4());
        unaffected_runtime.tmux_generation = "unaffected-generation".to_owned();
        assert!(
            backend
                .ensure_workstream_observer_available(&unaffected, Authority::Action)
                .is_ok()
        );
        drop(registry);
        let degraded = backend.read_snapshot().expect("degraded snapshot");
        assert_eq!(
            degraded.workstreams[0]
                .runtime
                .as_ref()
                .map(|runtime| (runtime.status, runtime.observer_degraded)),
            Some((RuntimeStatus::Starting, true))
        );
        let opencode = degraded
            .provider_capabilities
            .iter()
            .find(|capability| capability.provider == ProviderKind::OpenCode)
            .expect("OpenCode capability");
        assert_eq!(opencode.status, ProviderCapabilityStatus::Available);
        assert!(opencode.fresh_launch);

        state::clear_observer_degraded_marker(
            &root_path,
            runtime.runtime_id,
            &runtime.tmux_generation,
        )
        .expect("clear current marker");
        state::write_observer_degraded_marker(
            &root_path,
            runtime.runtime_id,
            "stale-generation",
            state::ObserverDegradedReason::CommitFailed,
        )
        .expect("stale marker");
        let current = backend.read_snapshot().expect("stale marker snapshot");
        assert_eq!(
            current.workstreams[0]
                .runtime
                .as_ref()
                .map(|runtime| runtime.observer_degraded),
            Some(false)
        );
    }

    #[test]
    fn browser_root_and_listing_use_exact_local_revisions() {
        let temporary = tempdir().expect("temporary state root");
        let root_path = temporary.path().join("state");
        let root = StateRoot::create(&root_path).expect("private state root");
        let state =
            state::fresh_create(&root_path, &RandomIdGenerator).expect("fresh schema 13 state");
        drop(state);
        let browser_root = temporary.path().join("browser");
        fs::create_dir(&browser_root).expect("browser root");
        let repository = browser_root.join("repository");
        fs::create_dir(&repository).expect("repository");
        fs::create_dir(repository.join(".git")).expect("git marker");
        fs::create_dir(browser_root.join("plain")).expect("plain directory");

        let mut backend = HostRegistryApplicationBackend::open_with_installation_cache(
            root,
            provider::InstallationProbeCache::probe_with(
                |_| false,
                provider::opencode::InstallationProbe::NotInstalled,
            ),
        )
        .expect("open backend");
        let first = backend
            .apply_action(&ApplicationAction::SetProjectBrowserRoot {
                root_path: BrowserRootPath::new(browser_root.to_string_lossy().to_string())
                    .unwrap(),
                expected_revision: Revision::INITIAL,
            })
            .expect("set browser root");
        assert_eq!(
            first,
            ApplicationOutcome::Applied {
                identity: RevisedIdentity::ProjectBrowserRoot(Revision::INITIAL),
            }
        );
        let listed = backend
            .apply_action(&ApplicationAction::ListProjectBrowser {
                relative_path: BrowserPath::root(),
                include_hidden: false,
            })
            .expect("list browser root");
        let ApplicationOutcome::BrowserListed(listing) = listed else {
            panic!("expected browser listing");
        };
        assert_eq!(listing.revision, Revision::INITIAL);
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.is_git_repository))
                .collect::<Vec<_>>(),
            vec![("repository", true), ("plain", false)]
        );

        let second_root = temporary.path().join("browser-second");
        fs::create_dir(&second_root).expect("second browser root");
        backend
            .apply_action(&ApplicationAction::SetProjectBrowserRoot {
                root_path: BrowserRootPath::new(second_root.to_string_lossy().to_string()).unwrap(),
                expected_revision: Revision::INITIAL,
            })
            .expect("update browser root");
        assert_eq!(
            backend.apply_action(&ApplicationAction::SetProjectBrowserRoot {
                root_path: BrowserRootPath::new(browser_root.to_string_lossy().to_string())
                    .unwrap(),
                expected_revision: Revision::INITIAL,
            }),
            Err(ApplicationError::StaleRevision {
                subject: RevisionSubject::ProjectBrowserRoot,
                expected: Revision::INITIAL,
                current: Revision::INITIAL.next(),
            })
        );
    }

    #[test]
    fn registration_atomically_supplies_the_initial_unstarted_workstream() {
        let temporary = tempdir().expect("temporary state root");
        let root_path = temporary.path().join("state");
        let root = StateRoot::create(&root_path).expect("private state root");
        drop(state::fresh_create(&root_path, &RandomIdGenerator).expect("fresh schema 13 state"));
        let browser_root = temporary.path().join("browser");
        let repository = browser_root.join("repository");
        fs::create_dir_all(&repository).expect("repository directory");
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&repository)
                .status()
                .expect("git init")
                .success()
        );

        let mut backend = HostRegistryApplicationBackend::open_with_installation_cache(
            root,
            provider::InstallationProbeCache::probe_with(
                |program| program == "tmux",
                provider::opencode::InstallationProbe::Available,
            ),
        )
        .expect("open backend");
        backend
            .apply_action(&ApplicationAction::SetProjectBrowserRoot {
                root_path: BrowserRootPath::new(browser_root.to_string_lossy().to_string())
                    .expect("browser root"),
                expected_revision: Revision::INITIAL,
            })
            .expect("set browser root");
        let outcome = backend
            .apply_action(&ApplicationAction::RegisterLocation {
                relative_path: BrowserPath::new("repository").expect("relative repository"),
                expected_browser_revision: Revision::INITIAL,
                provider: ProviderKind::OpenCode,
            })
            .expect("register Location and initial Workstream");
        let ApplicationOutcome::Created {
            workstream_id,
            location_id,
            revision,
        } = outcome
        else {
            panic!("registration must return its initial Workstream");
        };
        assert_eq!(revision, Revision::INITIAL);

        let input = backend.read_snapshot().expect("registered snapshot");
        assert_eq!(input.projects.len(), 1);
        assert_eq!(input.projects[0].locations.len(), 1);
        assert_eq!(input.projects[0].locations[0].location_id, location_id);
        assert_eq!(input.workstreams.len(), 1);
        assert_eq!(input.workstreams[0].workstream_id, workstream_id);
        assert_eq!(input.workstreams[0].location_id, location_id);
        assert_eq!(input.workstreams[0].provider, ProviderKind::OpenCode);
        assert!(input.workstreams[0].runtime.is_none());

        let project_id = input.projects[0].project_id;
        let project_revision = input.projects[0].revision;
        let location_revision = input.projects[0].locations[0].revision;
        let state_root = StateRoot::select(&root_path);
        let state = state::open_current_only(&state_root).expect("current schema 13");
        let mut registry = state.into_host_registry().expect("host registry");
        registry
            .archive_workstream(workstream_id, Revision::INITIAL, 0)
            .expect("archive the retained external source");
        let created = registry
            .create_independent_workstream_at_location(
                project_id,
                location_id,
                project_revision,
                location_revision,
                "d16-test:new-at-dormant-location",
                ProviderKind::OpenCode,
            )
            .expect("create at a dormant Location");
        assert_eq!(created.location_id, location_id);
        assert_eq!(created.source_workstream_id, workstream_id);
        assert_ne!(created.workstream_id, workstream_id);
        let replay = registry
            .create_independent_workstream_at_location(
                project_id,
                location_id,
                project_revision,
                location_revision,
                "d16-test:new-at-dormant-location",
                ProviderKind::OpenCode,
            )
            .expect("deduplicate Location creation");
        assert_eq!(replay, created);
        assert!(matches!(
            registry.create_independent_workstream_at_location(
                project_id,
                location_id,
                project_revision,
                location_revision,
                "d16-test:new-at-dormant-location",
                ProviderKind::Codex,
            ),
            Err(StateError::OperationRequestMismatch)
        ));
        assert!(matches!(
            registry.create_independent_workstream_at_location(
                project_id,
                location_id,
                project_revision.next(),
                location_revision,
                "d16-test:stale-location-evidence",
                ProviderKind::OpenCode,
            ),
            Err(StateError::ConcurrentWrite)
        ));
    }
}
