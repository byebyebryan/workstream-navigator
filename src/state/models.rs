use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AttentionState, CompoundOperation, DomainError, HostId, LocationId, OperationId, OperationKind,
    OperationPhase, ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeStatus,
    WorkstreamId, WorkstreamLifecycle, WorkstreamOrigin,
};
use crate::provider::codex::profile::ProfileOwnership;
use crate::provider::lifecycle::LifecycleHint;
use crate::provider::names::NameState;

use super::utils::{
    default_provider_kind, set_private_directory_permissions, validate_registry_text,
};

/// Maximum number of direct children returned by the host-local project
/// browser.  Browser responses are state/application DTOs; they are not part
/// of the retired framed host protocol.
pub const MAX_PROJECT_BROWSER_ENTRIES: usize = 128;

/// One bounded host-local project-browser listing.  The root is represented by
/// a safe display label and a validated relative cursor; absolute paths never
/// cross the state/application boundary.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDirectoriesResponse {
    pub root_label: String,
    pub relative_path: String,
    pub include_hidden: bool,
    pub entries: Vec<ProjectDirectoryEntry>,
}

/// One direct child in a host-local project-browser listing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDirectoryEntry {
    pub name: String,
    pub is_git_repository: bool,
}

pub struct StateRoot {
    base: PathBuf,
}

impl std::fmt::Debug for StateRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StateRoot")
            .field("base", &"<private>")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("concurrent state write")]
    ConcurrentWrite,
    #[error("invalid persisted value: {0}")]
    InvalidPersistedValue(String),
    #[error("invalid registry field {0}")]
    InvalidRegistryField(&'static str),
    #[error("provider metadata is invalid")]
    InvalidProviderMetadata,
    #[error("provider identity does not match its Workstream, Runtime, or binding")]
    ProviderIdentityMismatch,
    #[error("project browser root is invalid")]
    InvalidProjectBrowserRoot,
    #[error("project browser relative path is invalid")]
    InvalidProjectBrowserRelativePath,
    #[error("project browser root is unavailable")]
    ProjectBrowserRootUnavailable,
    #[error("provider fork plan could not be encoded")]
    ForkPlanEncoding(serde_json::Error),
    #[error("provider fork plan is missing from its operation")]
    MissingForkPlan,
    #[error("provider fork plan is invalid")]
    InvalidForkPlan(serde_json::Error),
    #[error("provider fork plan has an invalid shape")]
    InvalidForkPlanShape,
    #[error("provider fork plan does not match the durable operation")]
    ForkPlanMismatch,
    #[error("provider fork operation is not ready to commit")]
    ForkOperationUnavailable,
    #[error("provider fork operation committed without its expected Workstream")]
    ForkCommitMissing,
    #[error("provider fork operation outcome is missing")]
    MissingForkOutcome,
    #[error("provider fork operation outcome is invalid")]
    InvalidForkOutcome(serde_json::Error),
    #[error("could not encode the OpenCode session-creation plan")]
    OpenCodeSessionCreationPlanEncoding(serde_json::Error),
    #[error("OpenCode session-creation plan is missing from its operation")]
    MissingOpenCodeSessionCreationPlan,
    #[error("OpenCode session-creation plan is invalid")]
    InvalidOpenCodeSessionCreationPlan(serde_json::Error),
    #[error("OpenCode session-creation plan has an invalid shape")]
    InvalidOpenCodeSessionCreationPlanShape,
    #[error("OpenCode session-creation operation is not available")]
    OpenCodeSessionCreationUnavailable,
    #[error("D17 onboarding operations are unavailable through the D16 state boundary")]
    OnboardingOperationUnavailable,
    #[error("D17 onboarding preparation evidence is invalid")]
    InvalidOnboardingPreparation,
    #[error("D17 onboarding capability is expired")]
    OnboardingCapabilityExpired,
    #[error("D17 onboarding capability does not match the prepared handoff")]
    OnboardingCapabilityRejected,
    #[error("request key was reused with different workstream intent")]
    OperationRequestMismatch,
    #[error("the source Workstream has no live exact settled conversation boundary")]
    ForkBoundaryUnavailable,
    #[error("too many Workstreams for one bounded navigator snapshot")]
    NavigatorSnapshotTooLarge,
    #[error("navigator Workstream page size is invalid")]
    InvalidNavigatorPageSize,
    #[error("navigator Workstream cursor overflowed")]
    NavigatorCursorOverflow,
    #[error("project display name is invalid")]
    InvalidProjectDisplayName,
    #[error("repository fingerprint is invalid")]
    InvalidRepositoryFingerprint,
    #[error("invalid persisted UUID: {0}")]
    InvalidPersistedUuid(uuid::Error),
    #[error("I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("missing operation for request key {0}")]
    MissingOperation(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "host state schema {0} belongs to the retired worktree-managed design; reset this host state and re-register projects"
    )]
    HostStateResetRequired(i64),
    #[error("unknown operation {0}")]
    UnknownOperation(OperationId),
    #[error("workstream {0} is unknown or not open")]
    UnknownOpenWorkstream(WorkstreamId),
    #[error("workstream {0} is already archived")]
    WorkstreamAlreadyArchived(WorkstreamId),
    #[error("workstream {0} is not archived")]
    WorkstreamNotArchived(WorkstreamId),
    #[error("workstream {0} is archived; restore it before starting or forking")]
    WorkstreamArchived(WorkstreamId),
    #[error("workstream {0} already has a live runtime")]
    RuntimeAlreadyLive(WorkstreamId),
    #[error("workstream {0} is not ready for explicit native recovery")]
    RecoveryUnavailable(WorkstreamId),
    #[error("hook evidence does not match the managed runtime")]
    HookEvidenceMismatch,
    #[error("unknown runtime {0}")]
    UnknownRuntime(RuntimeId),
    #[error("Codex observer ownership does not match the recorded profile")]
    IntegrationOwnershipMismatch,
    #[error("state recovery required: {0:?}")]
    StateRecoveryRequired(crate::state::d16::StateRecoveryReason),
    #[error("fresh state root is not adoptable: {0:?}")]
    FreshRootRejected(crate::state::d16::FreshRootRejection),
    #[error("D16 cutover is required")]
    CutoverRequired,
    #[error("fresh state creation is required")]
    FreshStateRequired,
    #[error("malformed host schema evidence")]
    MalformedHostSchema,
    #[error("unsupported future host schema {0}")]
    UnsupportedFutureHostSchema(i64),
    #[error("observer database deadline exceeded")]
    ObserverDatabaseDeadlineExceeded,
    #[error("observer degraded marker is invalid")]
    InvalidObserverDegradedMarker,
    #[error("OpenCode observer handover journal is invalid")]
    InvalidObserverHandoverJournal,
    #[error("OpenCode observer handover journal phase transition is invalid")]
    InvalidObserverHandoverTransition,
    #[error("the requested state transition requires a held transition lease")]
    TransitionLeaseRequired,
    #[error("the held transition lease does not match the requested state root")]
    TransitionLeaseRootMismatch,
    #[error("the held transition lease is no longer valid")]
    InvalidTransitionLease,
    #[error("the stable provisional lease is busy")]
    ProvisionalLeaseBusy,
    #[error("the held provisional lease is no longer valid")]
    InvalidProvisionalLease,
}

impl StateError {
    pub(in crate::state) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl StateRoot {
    /// Selects a state-root path without creating, chmodding, or otherwise
    /// inspecting it.  D16 startup uses this before it has classified a root
    /// as current, cutover, fresh, or recovery-only.
    #[must_use]
    pub fn select(base: impl AsRef<Path>) -> Self {
        Self {
            base: base.as_ref().to_path_buf(),
        }
    }

    /// Creates a private, classified empty state-root directory for tests and
    /// fresh-state orchestration.
    ///
    /// This helper never creates a database or adopts existing state. Callers
    /// that need a usable registry must go through [`crate::state::fresh_create`]
    /// (or an explicit current/cutover open), so root creation cannot bypass
    /// D16's fresh-root classifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or its permissions
    /// cannot be restricted.
    pub fn create(base: impl AsRef<Path>) -> Result<Self, StateError> {
        let base = base.as_ref().to_path_buf();
        match crate::state::d16::classify_fresh_root(&base)? {
            crate::state::d16::FreshRootClassification::Absent
            | crate::state::d16::FreshRootClassification::Empty => {}
            crate::state::d16::FreshRootClassification::TransitionLeaseOnly => {
                return Err(StateError::FreshStateRequired);
            }
        }
        fs::create_dir_all(&base).map_err(|source| StateError::Io {
            path: base.clone(),
            source,
        })?;
        set_private_directory_permissions(&base)?;
        Ok(Self { base })
    }

    #[must_use]
    pub fn host_database_path(&self) -> PathBuf {
        self.base.join("host.sqlite")
    }

    /// Returns the private state-root directory used for runtime path derivation.
    #[must_use]
    pub fn base(&self) -> &Path {
        &self.base
    }
}

#[derive(Debug)]
pub struct HostRegistry {
    pub(in crate::state) connection: Connection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostIdentity {
    pub host_id: HostId,
    pub registry_generation: String,
}

/// One registered project root and its initial Workstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalWorkstream {
    pub location_id: LocationId,
    pub workstream_id: WorkstreamId,
}

/// The persisted target of one native provider fork.
///
/// The project root is the exact launch directory for the destination. It is
/// host-private and never returned by application snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkPlan {
    pub operation: CompoundOperation,
    pub provider: ProviderKind,
    pub workstream_id: WorkstreamId,
    pub location_id: LocationId,
    pub origin: WorkstreamOrigin,
    pub source_workstream_id: WorkstreamId,
    pub project_root: PathBuf,
    pub source_runtime_id: Option<RuntimeId>,
    pub source_native_session_id: Option<ProviderSessionId>,
    pub last_settled_turn_id: Option<String>,
    pub source_native_name: Option<String>,
    /// Recorded immediately before the one non-idempotent provider fork.
    /// `None` proves this process has not crossed that external-effect point.
    pub fork_attempted_at_millis: Option<i64>,
}

/// Stable bounded outcome code for an `OpenCode` fork whose provider effect may
/// have happened but whose response cannot be trusted. Such an operation is
/// terminal and is never retried or reconciled by `WSNav`.
pub const EXTERNAL_EFFECT_UNKNOWN_CODE: &str = "external_effect_unknown";

/// One committed Workstream record returned after an independent creation or
/// exact provider fork.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedWorkstream {
    pub workstream_id: WorkstreamId,
    pub location_id: LocationId,
    pub provider: ProviderKind,
    pub origin: WorkstreamOrigin,
    pub source_workstream_id: WorkstreamId,
    pub revision: Revision,
}

/// The exact durable state returned before a provider fork effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkPreparation {
    pub plan: ForkPlan,
    /// `true` only when this call atomically recorded the external-effect plan.
    /// A later caller must reconcile the recorded plan rather than run Git again.
    pub newly_prepared: bool,
}

/// The durable journal for one exact, non-idempotent `OpenCode` blank-session
/// creation. The operation is keyed by Runtime ID plus Runtime generation;
/// it never stores the provider response beyond the bounded native session ID
/// needed to launch and corroborate that same session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeSessionCreationOperation {
    pub operation: CompoundOperation,
    pub runtime_id: RuntimeId,
    pub workstream_id: WorkstreamId,
    pub runtime_generation: String,
    pub native_session_id: Option<ProviderSessionId>,
}

pub(in crate::state) const OPENCODE_SESSION_CREATION_UNKNOWN_CODE: &str =
    "opencode_session_creation_external_effect_unknown";
pub(in crate::state) const OPENCODE_SESSION_CREATION_CLEANUP_UNKNOWN_CODE: &str =
    "opencode_session_creation_cleanup_unknown";
pub(in crate::state) const OPENCODE_SESSION_CREATION_PLAN_SCHEMA_VERSION: u8 = 1;

/// Bounded operator-visible state for one unresolved creation operation.
///
/// Request keys, project paths, provider identifiers, and raw effect evidence
/// remain host-private. A Fork additionally carries its already-visible source
/// Workstream identity so the navigator can route a repeated Fork directly to
/// its exact unfinished operation without displaying either identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationOverview {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    /// The provider captured by the typed private operation plan.  This is
    /// decoded by the state projection and is never inferred by callers from
    /// the operation kind.
    pub provider: ProviderKind,
    pub source_workstream_id: Option<WorkstreamId>,
    pub phase: OperationPhase,
    pub revision: Revision,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(in crate::state) struct PersistedForkPlan {
    pub(in crate::state) schema_version: u8,
    #[serde(default = "default_provider_kind")]
    pub(in crate::state) provider: ProviderKind,
    pub(in crate::state) workstream_id: WorkstreamId,
    pub(in crate::state) location_id: LocationId,
    pub(in crate::state) origin: WorkstreamOrigin,
    pub(in crate::state) source_workstream_id: WorkstreamId,
    pub(in crate::state) project_root: PathBuf,
    pub(in crate::state) source_runtime_id: Option<RuntimeId>,
    pub(in crate::state) source_native_session_id: Option<ProviderSessionId>,
    pub(in crate::state) last_settled_turn_id: Option<String>,
    #[serde(default)]
    pub(in crate::state) source_native_name: Option<String>,
    #[serde(default)]
    pub(in crate::state) fork_attempted_at_millis: Option<i64>,
}

impl<'de> Deserialize<'de> for PersistedForkPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            schema_version: u8,
            #[serde(default = "default_provider_kind")]
            provider: ProviderKind,
            workstream_id: WorkstreamId,
            location_id: LocationId,
            origin: WorkstreamOrigin,
            source_workstream_id: WorkstreamId,
            project_root: PathBuf,
            source_runtime_id: Option<RuntimeId>,
            #[serde(default)]
            source_native_session_id: Option<serde_json::Value>,
            last_settled_turn_id: Option<String>,
            #[serde(default)]
            source_native_name: Option<String>,
            #[serde(default)]
            fork_attempted_at_millis: Option<i64>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let source_native_session_id = wire
            .source_native_session_id
            .map(|value| match value {
                serde_json::Value::String(value) => {
                    ProviderSessionId::new(ProviderKind::Codex, value)
                }
                value => {
                    serde_json::from_value(value).map_err(DomainError::InvalidOperationOutcome)
                }
            })
            .transpose()
            .map_err(serde::de::Error::custom)?;
        if source_native_session_id
            .as_ref()
            .is_some_and(|session| session.provider() != wire.provider)
        {
            return Err(serde::de::Error::custom(
                StateError::ProviderIdentityMismatch,
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            provider: wire.provider,
            workstream_id: wire.workstream_id,
            location_id: wire.location_id,
            origin: wire.origin,
            source_workstream_id: wire.source_workstream_id,
            project_root: wire.project_root,
            source_runtime_id: wire.source_runtime_id,
            source_native_session_id,
            last_settled_turn_id: wire.last_settled_turn_id,
            source_native_name: wire.source_native_name,
            fork_attempted_at_millis: wire.fork_attempted_at_millis,
        })
    }
}

impl PersistedForkPlan {
    pub(in crate::state) fn encode(&self) -> Result<String, StateError> {
        serde_json::to_string(self).map_err(StateError::ForkPlanEncoding)
    }

    pub(in crate::state) fn decode(value: Option<&str>) -> Result<Self, StateError> {
        let value = value.ok_or(StateError::MissingForkPlan)?;
        let plan: Self = serde_json::from_str(value).map_err(StateError::InvalidForkPlan)?;
        if plan.schema_version != 1
            || plan.project_root.as_os_str().is_empty()
            || plan
                .source_native_name
                .as_deref()
                .is_some_and(|name| name.len() > 512 || name.contains(['\n', '\r']))
            || plan.fork_attempted_at_millis.is_some_and(|time| time < 0)
        {
            return Err(StateError::InvalidForkPlanShape);
        }
        Ok(plan)
    }

    pub(in crate::state) fn public_plan(&self, operation: CompoundOperation) -> ForkPlan {
        ForkPlan {
            operation,
            provider: self.provider,
            workstream_id: self.workstream_id,
            location_id: self.location_id,
            origin: self.origin,
            source_workstream_id: self.source_workstream_id,
            project_root: self.project_root.clone(),
            source_runtime_id: self.source_runtime_id,
            source_native_session_id: self.source_native_session_id.clone(),
            last_settled_turn_id: self.last_settled_turn_id.clone(),
            source_native_name: self.source_native_name.clone(),
            fork_attempted_at_millis: self.fork_attempted_at_millis,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(in crate::state) struct PersistedOpenCodeSessionCreationPlan {
    pub(in crate::state) schema_version: u8,
    pub(in crate::state) provider: ProviderKind,
    pub(in crate::state) runtime_id: RuntimeId,
    pub(in crate::state) workstream_id: WorkstreamId,
    pub(in crate::state) runtime_generation: String,
    #[serde(default)]
    pub(in crate::state) native_session_id: Option<ProviderSessionId>,
}

impl PersistedOpenCodeSessionCreationPlan {
    pub(in crate::state) fn encode(&self) -> Result<String, StateError> {
        serde_json::to_string(self).map_err(StateError::OpenCodeSessionCreationPlanEncoding)
    }

    pub(in crate::state) fn decode(value: Option<&str>) -> Result<Self, StateError> {
        let value = value.ok_or(StateError::MissingOpenCodeSessionCreationPlan)?;
        let plan: Self =
            serde_json::from_str(value).map_err(StateError::InvalidOpenCodeSessionCreationPlan)?;
        if plan.schema_version != OPENCODE_SESSION_CREATION_PLAN_SCHEMA_VERSION
            || plan.provider != ProviderKind::OpenCode
            || plan.runtime_id.as_uuid() == Uuid::nil()
            || plan.workstream_id.as_uuid() == Uuid::nil()
        {
            return Err(StateError::InvalidOpenCodeSessionCreationPlanShape);
        }
        validate_registry_text("runtime generation", &plan.runtime_generation)?;
        if plan
            .native_session_id
            .as_ref()
            .is_some_and(|session| session.provider() != ProviderKind::OpenCode)
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
        Ok(plan)
    }

    pub(in crate::state) fn public_plan(
        &self,
        operation: CompoundOperation,
    ) -> OpenCodeSessionCreationOperation {
        OpenCodeSessionCreationOperation {
            operation,
            runtime_id: self.runtime_id,
            workstream_id: self.workstream_id,
            runtime_generation: self.runtime_generation.clone(),
            native_session_id: self.native_session_id.clone(),
        }
    }
}

/// The persisted record that makes one native tmux process recoverable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeRecord {
    pub runtime_id: RuntimeId,
    pub workstream_id: WorkstreamId,
    pub provider: ProviderKind,
    pub tmux_generation: String,
    pub tmux_session: String,
    pub cwd: PathBuf,
    pub provider_pid: Option<u32>,
    pub process_birth: Option<String>,
    pub status: RuntimeStatus,
    pub revision: Revision,
}

/// The exact native Codex session currently bound to a managed runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderBinding {
    pub runtime_id: RuntimeId,
    pub provider: ProviderKind,
    pub native_session_id: ProviderSessionId,
    pub start_source: String,
    pub last_settled_turn_id: Option<String>,
    pub observed_thread_name: Option<String>,
    pub name_state: NameState,
    pub predecessor_native_session_id: Option<ProviderSessionId>,
    pub predecessor_effective_name: Option<String>,
    /// The private Runtime generation in which this native session was
    /// corroborated. A binding from an older generation is retained for
    /// exact Codex resume, but cannot authorize lifecycle or metadata writes
    /// until the matching `SessionStart` rotates it to the current generation.
    pub runtime_generation: String,
    pub revision: Revision,
}

/// Lifecycle of the hidden `OpenCode` SSE observer for one exact Runtime
/// generation.  This is host-private evidence and is never projected into a
/// public snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenCodeObserverStatus {
    Starting,
    Ready,
    Unknown,
    Stopped,
}

impl OpenCodeObserverStatus {
    pub(in crate::state) const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Unknown => "unknown",
            Self::Stopped => "stopped",
        }
    }
}

/// Host-private `OpenCode` endpoint and observer identity for one exact Runtime
/// generation.  Provider payloads, event content, and terminal captures are
/// intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeRuntimeHandle {
    pub runtime_id: RuntimeId,
    pub runtime_generation: String,
    pub endpoint_host: String,
    pub endpoint_port: u16,
    pub version: String,
    pub native_session_id: ProviderSessionId,
    pub observer_pid: Option<u32>,
    pub observer_birth: Option<String>,
    pub observer_status: OpenCodeObserverStatus,
    pub revision: Revision,
}

/// Exact evidence supplied by a private `OpenCode` observer for one Runtime
/// revision.  The state layer consumes this bounded, provider-neutral hint;
/// it never stores the originating SSE record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeLifecycleObservation {
    pub generation: String,
    pub cwd: PathBuf,
    pub runtime_revision: Revision,
    pub session: ProviderSessionId,
    pub observer_pid: u32,
    pub observer_birth: String,
    pub hint: LifecycleHint,
}

/// One bounded host-local record needed to render and act on a Workstream.
/// It deliberately excludes provider turns, prompts, terminal contents, and
/// process details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamOverview {
    pub workstream_id: WorkstreamId,
    pub location_id: LocationId,
    pub provider: ProviderKind,
    pub project_repository_path: PathBuf,
    pub project_display_name: String,
    pub remote_identity_fingerprint: Option<String>,
    pub remote_identity_display: Option<String>,
    pub lifecycle: WorkstreamLifecycle,
    /// Archive is an independent visibility state. It preserves all lifecycle,
    /// Runtime, binding, attention, project, and lineage records.
    pub archived_at_millis: Option<i64>,
    pub last_activity_sequence: i64,
    /// Wall-clock time of the most recent observed native conversation activity.
    /// `None` means no turn has been observed and no time is inferred.
    pub last_activity_at_millis: Option<i64>,
    pub revision: Revision,
    pub runtime: Option<RuntimeRecord>,
    pub binding: Option<ProviderBinding>,
    pub attention: Option<AttentionState>,
}

/// One deterministic bounded page of navigator-safe host state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamOverviewPage {
    pub workstreams: Vec<WorkstreamOverview>,
    pub next_cursor: Option<u32>,
}

/// One deterministic bounded page of unresolved navigator operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationOverviewPage {
    pub operations: Vec<OperationOverview>,
    pub next_cursor: Option<u32>,
}

pub(in crate::state) struct PersistedWorkstreamOverview {
    pub(in crate::state) workstream_id: String,
    pub(in crate::state) location_id: String,
    pub(in crate::state) provider: String,
    pub(in crate::state) project_repository_path: String,
    pub(in crate::state) project_display_name: String,
    pub(in crate::state) remote_identity_fingerprint: Option<String>,
    pub(in crate::state) remote_identity_display: Option<String>,
    pub(in crate::state) lifecycle: String,
    pub(in crate::state) archived_at_millis: Option<i64>,
    pub(in crate::state) activity_sequence: i64,
    pub(in crate::state) activity_at_millis: i64,
    pub(in crate::state) revision: i64,
}

/// Persisted ownership and native-trust state for the only managed Codex profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationLifecycle {
    TrustPending,
    Ready,
    Modified,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexIntegration {
    pub ownership: ProfileOwnership,
    pub lifecycle: IntegrationLifecycle,
    pub revision: Revision,
}
