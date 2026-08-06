//! Bounded machine-to-machine frames used by local and SSH host adapters.
//!
//! The protocol deliberately contains only durable navigator metadata. It
//! never transports a provider prompt, response, terminal capture, or hook
//! payload. A project path may appear only in the one bounded registration
//! request sent to its selected host; it is never returned in a response.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    HostId, LocationId, OperationId, OperationKind, OperationPhase, ProviderKind, Revision,
    RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
};

pub const CURRENT_PROTOCOL_VERSION: u16 = 17;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 512;
pub const MAX_SNAPSHOT_WORKSTREAMS: usize = 128;
pub const SNAPSHOT_PAGE_WORKSTREAMS: usize = 32;
pub const MAX_SNAPSHOT_PAGES: usize = 256;

const MAX_ALIAS_BYTES: usize = 128;
const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_THREAD_NAME_BYTES: usize = 512;
const MAX_VERSION_BYTES: usize = 64;
const MAX_REGISTRY_GENERATION_BYTES: usize = 128;
const MAX_REQUEST_KEY_BYTES: usize = 128;
const MAX_CHECKOUT_PATH_BYTES: usize = 4096;
const MAX_PROJECT_BROWSER_ROOT_BYTES: usize = 4096;
const MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES: usize = 1024;
const MAX_PROJECT_BROWSER_LABEL_BYTES: usize = 512;
const MAX_PROJECT_BROWSER_ENTRY_NAME_BYTES: usize = 256;
pub const MAX_PROJECT_BROWSER_ENTRIES: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestEnvelope {
    pub version: u16,
    pub request: HostRequest,
}

impl RequestEnvelope {
    /// Validates a request before it can be dispatched by a host.
    ///
    /// # Errors
    ///
    /// Returns an error for an incompatible protocol version or malformed,
    /// unbounded request field.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        self.request.validate()
    }

    /// Serializes one validated request as a single bounded JSON frame.
    ///
    /// # Errors
    ///
    /// Returns an error if validation or JSON encoding fails, or the frame
    /// would exceed the protocol's bounded transport size.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        encode_frame(self)
    }

    /// Decodes and validates one bounded JSON request frame.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, malformed, or incompatible frames.
    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        let request: Self = decode_frame(frame)?;
        request.validate()?;
        Ok(request)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostRequest {
    Hello {
        client_alias: String,
    },
    Snapshot {
        cursor: Option<u32>,
    },
    Operations,
    /// Lists bounded child directories below the selected host's private
    /// project-browser root. The path is relative and cannot escape that root.
    ProjectDirectories {
        relative_path: String,
    },
    Attach {
        runtime_id: RuntimeId,
    },
    Apply {
        action: HostAction,
    },
}

impl HostRequest {
    fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Hello { client_alias } => {
                validate_bounded("client alias", client_alias, MAX_ALIAS_BYTES)
            }
            Self::Snapshot { .. } | Self::Operations | Self::Attach { .. } => Ok(()),
            Self::ProjectDirectories { relative_path } => {
                validate_relative_browser_path(relative_path)
            }
            Self::Apply { action } => action.validate(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostAction {
    /// Install or reconcile the exact WSNav-owned observer profile. Native
    /// hook approval remains an explicit interactive provider-surface action.
    PrepareObserver,
    /// Remove only an exact unchanged WSNav-owned observer profile after every
    /// managed Runtime on that host has stopped.
    RemoveObserver,
    /// Register one existing Git project only on this host. The bounded path is
    /// request-only and is never included in a response or snapshot.
    RegisterCheckout {
        checkout_path: String,
        provider: ProviderKind,
    },
    /// Register the currently selected host-private browser directory. Unlike
    /// `RegisterCheckout`, the client supplies only a validated relative
    /// cursor; the host resolves and inspects the actual path locally.
    RegisterProjectDirectory {
        relative_path: String,
        provider: ProviderKind,
    },
    /// Changes only the selected host's private directory-browser root. The
    /// path is request-only and never included in a response or snapshot.
    SetProjectBrowserRoot { root_path: String },
    AcknowledgeAttention {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    Park {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    /// Hide a Workstream from the active navigator scope without deleting its
    /// retained provider or project state.
    Archive {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    /// Return a Workstream to the active navigator scope without starting it.
    Restore {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    /// Set the canonical provider-owned title for one exact active Workstream.
    Rename {
        workstream_id: WorkstreamId,
        expected_revision: i64,
        name: String,
    },
    Start {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    /// Reopen a Workstream only through verified native provider resume evidence.
    Recover {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    /// Reconcile one explicitly selected unresolved creation operation.
    RecoverOperation { operation_id: OperationId },
    /// Start a fresh Workstream at the registered project root.
    NewWorkstream {
        source_workstream_id: WorkstreamId,
        expected_revision: i64,
        request_key: String,
        provider: ProviderKind,
    },
    /// Fork an active source through its last settled native provider turn.
    ForkWorkstream {
        source_workstream_id: WorkstreamId,
        expected_revision: i64,
        request_key: String,
    },
}

impl HostAction {
    fn validate(&self) -> Result<(), ProtocolError> {
        let expected_revision = match self {
            Self::AcknowledgeAttention {
                expected_revision, ..
            }
            | Self::Park {
                expected_revision, ..
            }
            | Self::Archive {
                expected_revision, ..
            }
            | Self::Restore {
                expected_revision, ..
            }
            | Self::Rename {
                expected_revision, ..
            }
            | Self::Start {
                expected_revision, ..
            }
            | Self::Recover {
                expected_revision, ..
            }
            | Self::NewWorkstream {
                expected_revision, ..
            }
            | Self::ForkWorkstream {
                expected_revision, ..
            } => Some(*expected_revision),
            Self::PrepareObserver
            | Self::RemoveObserver
            | Self::RegisterCheckout { .. }
            | Self::RegisterProjectDirectory { .. }
            | Self::SetProjectBrowserRoot { .. }
            | Self::RecoverOperation { .. } => None,
        };
        if let Some(expected_revision) = expected_revision {
            Revision::try_from(expected_revision).map_err(|_| ProtocolError::InvalidRevision)?;
        }
        match self {
            Self::RegisterCheckout { checkout_path, .. } => {
                validate_bounded("checkout path", checkout_path, MAX_CHECKOUT_PATH_BYTES)?;
            }
            Self::RegisterProjectDirectory { relative_path, .. } => {
                validate_relative_browser_path(relative_path)?;
            }
            Self::SetProjectBrowserRoot { root_path } => {
                validate_bounded(
                    "project browser root",
                    root_path,
                    MAX_PROJECT_BROWSER_ROOT_BYTES,
                )?;
                if root_path.chars().any(char::is_control) {
                    return Err(ProtocolError::InvalidProjectBrowserPath);
                }
            }
            Self::NewWorkstream { request_key, .. } | Self::ForkWorkstream { request_key, .. } => {
                validate_bounded("request key", request_key, MAX_REQUEST_KEY_BYTES)?;
            }
            Self::Rename { name, .. } => {
                validate_bounded("thread name", name, MAX_THREAD_NAME_BYTES)?;
                if name.trim().is_empty() {
                    return Err(ProtocolError::InvalidThreadName);
                }
            }
            Self::PrepareObserver
            | Self::RemoveObserver
            | Self::AcknowledgeAttention { .. }
            | Self::Park { .. }
            | Self::Archive { .. }
            | Self::Restore { .. }
            | Self::Start { .. }
            | Self::Recover { .. }
            | Self::RecoverOperation { .. } => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResponseEnvelope {
    pub version: u16,
    pub response: HostResponse,
}

impl ResponseEnvelope {
    /// Creates a bounded rejection frame safe for machine-oriented transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the diagnostic cannot safely fit in the protocol
    /// frame.
    pub fn rejected(diagnostic: String) -> Result<Self, ProtocolError> {
        validate_bounded("diagnostic", &diagnostic, MAX_DIAGNOSTIC_BYTES)?;
        Ok(Self {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Rejected { diagnostic },
        })
    }

    /// Validates a response received from a host before client state can use
    /// it for presentation or mutation authority.
    ///
    /// # Errors
    ///
    /// Returns an error for incompatible, malformed, or oversized response
    /// fields.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        self.response.validate()
    }

    /// Serializes one validated response as a single bounded JSON frame.
    ///
    /// # Errors
    ///
    /// Returns an error if validation or JSON encoding fails, or the frame
    /// would exceed the protocol's bounded transport size.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        encode_frame(self)
    }

    /// Decodes and validates one bounded JSON response frame.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, malformed, or incompatible frames.
    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        let response: Self = decode_frame(frame)?;
        response.validate()?;
        Ok(response)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostResponse {
    Hello(HelloResponse),
    Snapshot(SnapshotResponse),
    Operations(OperationsResponse),
    ProjectDirectories(ProjectDirectoriesResponse),
    Applied {
        revision: i64,
    },
    WorkstreamCreated {
        workstream_id: WorkstreamId,
        provider: ProviderKind,
        revision: i64,
    },
    Attach {
        runtime_id: RuntimeId,
    },
    Rejected {
        diagnostic: String,
    },
}

impl HostResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Hello(response) => response.validate(),
            Self::Snapshot(response) => response.validate(),
            Self::Operations(response) => response.validate(),
            Self::ProjectDirectories(response) => response.validate(),
            Self::Applied { revision } => Revision::try_from(*revision)
                .map(|_| ())
                .map_err(|_| ProtocolError::InvalidRevision),
            Self::WorkstreamCreated { revision, .. } => Revision::try_from(*revision)
                .map(|_| ())
                .map_err(|_| ProtocolError::InvalidRevision),
            Self::Attach { .. } => Ok(()),
            Self::Rejected { diagnostic } => {
                validate_bounded("diagnostic", diagnostic, MAX_DIAGNOSTIC_BYTES)
            }
        }
    }
}

/// One bounded, host-private directory browser response. The root is rendered
/// as a safe display label; absolute paths are never returned through the
/// control protocol.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDirectoriesResponse {
    pub root_label: String,
    pub relative_path: String,
    pub entries: Vec<ProjectDirectoryEntry>,
}

impl ProjectDirectoriesResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_project_browser_root_label(&self.root_label)?;
        validate_relative_browser_path(&self.relative_path)?;
        if self.entries.len() > MAX_PROJECT_BROWSER_ENTRIES {
            return Err(ProtocolError::SnapshotTooLarge);
        }
        self.entries
            .iter()
            .try_for_each(ProjectDirectoryEntry::validate)
    }
}

/// One direct child of a project-browser directory. This is display metadata,
/// never a repository path or durable `ProjectLocation`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectDirectoryEntry {
    pub name: String,
    pub is_git_repository: bool,
}

impl ProjectDirectoryEntry {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_bounded(
            "project browser entry name",
            &self.name,
            MAX_PROJECT_BROWSER_ENTRY_NAME_BYTES,
        )?;
        if self.name == "." || self.name == ".." || self.name.contains(['/', '\\']) {
            return Err(ProtocolError::InvalidProjectBrowserPath);
        }
        if self.name.chars().any(char::is_control) {
            return Err(ProtocolError::InvalidProjectBrowserPath);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HelloResponse {
    pub host_id: HostId,
    pub registry_generation: String,
    pub wsnav_version: String,
    pub capabilities: Capabilities,
}

impl HelloResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_bounded(
            "registry generation",
            &self.registry_generation,
            MAX_REGISTRY_GENERATION_BYTES,
        )?;
        validate_bounded("wsnav version", &self.wsnav_version, MAX_VERSION_BYTES)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub git: bool,
    pub tmux: bool,
}

/// Dynamic provider availability observed by one host snapshot. This is
/// deliberately excluded from fixed client host registration identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityStatus {
    Available,
    Unavailable,
    Unknown,
}

/// Bounded reason for a provider's dynamic availability state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityReason {
    None,
    AdapterUnavailable,
    NotInstalled,
    UnsupportedVersion,
    ObserverNotReady,
    RuntimePrerequisiteMissing,
    ProbeFailed,
}

/// One provider's bounded, read-only host capability evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapability {
    pub kind: ProviderKind,
    pub status: ProviderCapabilityStatus,
    pub reason: ProviderCapabilityReason,
    pub fresh_launch: bool,
    pub exact_resume: bool,
    pub observe: bool,
    pub metadata_read: bool,
    pub rename: bool,
    pub fork: bool,
}

impl ProviderCapability {
    /// Returns whether this provider may be selected for a recoverable New.
    #[must_use]
    pub const fn is_new_eligible(self) -> bool {
        matches!(self.status, ProviderCapabilityStatus::Available)
            && self.fresh_launch
            && self.exact_resume
            && self.observe
    }

    pub(crate) fn validate(&self) -> Result<(), ProtocolError> {
        match (self.status, self.reason) {
            (ProviderCapabilityStatus::Available, ProviderCapabilityReason::None)
            | (
                ProviderCapabilityStatus::Unavailable | ProviderCapabilityStatus::Unknown,
                ProviderCapabilityReason::AdapterUnavailable
                | ProviderCapabilityReason::NotInstalled
                | ProviderCapabilityReason::UnsupportedVersion
                | ProviderCapabilityReason::ObserverNotReady
                | ProviderCapabilityReason::RuntimePrerequisiteMissing
                | ProviderCapabilityReason::ProbeFailed,
            ) => {}
            _ => return Err(ProtocolError::InvalidProviderCapability),
        }
        if !matches!(self.status, ProviderCapabilityStatus::Available)
            && (self.fresh_launch
                || self.exact_resume
                || self.observe
                || self.metadata_read
                || self.rename
                || self.fork)
        {
            return Err(ProtocolError::InvalidProviderCapability);
        }
        Ok(())
    }
}

/// The fixed provider set known to this protocol version, in wire sort order.
pub const KNOWN_PROVIDER_KINDS: [ProviderKind; 2] = [ProviderKind::Codex, ProviderKind::OpenCode];

fn default_provider_capabilities() -> Vec<ProviderCapability> {
    KNOWN_PROVIDER_KINDS
        .into_iter()
        .map(|kind| ProviderCapability {
            kind,
            status: if kind == ProviderKind::Codex {
                ProviderCapabilityStatus::Unknown
            } else {
                ProviderCapabilityStatus::Unavailable
            },
            reason: if kind == ProviderKind::Codex {
                ProviderCapabilityReason::ProbeFailed
            } else {
                ProviderCapabilityReason::AdapterUnavailable
            },
            fresh_launch: false,
            exact_resume: false,
            observe: false,
            metadata_read: false,
            rename: false,
            fork: false,
        })
        .collect()
}

/// Bounded observer lifecycle state safe to expose to a trusted navigator.
/// It describes only `WSNav`'s exact owned Codex observer declaration; it never
/// represents provider prompts, hook payloads, or an arbitrary profile.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverStatus {
    #[default]
    NotInstalled,
    TrustPending,
    Ready,
    Modified,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotResponse {
    pub workstreams: Vec<SnapshotWorkstream>,
    pub unresolved_operation_count: u16,
    pub observer_status: ObserverStatus,
    pub provider_capabilities: Vec<ProviderCapability>,
    /// Opaque row offset for the next deterministic bounded page.
    pub next_cursor: Option<u32>,
}

impl Default for SnapshotResponse {
    fn default() -> Self {
        Self {
            workstreams: Vec::new(),
            unresolved_operation_count: 0,
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: default_provider_capabilities(),
            next_cursor: None,
        }
    }
}

impl SnapshotResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.workstreams.len() > MAX_SNAPSHOT_WORKSTREAMS {
            return Err(ProtocolError::SnapshotTooLarge);
        }
        self.workstreams
            .iter()
            .try_for_each(SnapshotWorkstream::validate)?;
        if self.provider_capabilities.len() != KNOWN_PROVIDER_KINDS.len()
            || self
                .provider_capabilities
                .iter()
                .zip(KNOWN_PROVIDER_KINDS)
                .any(|(capability, expected)| capability.kind != expected)
        {
            return Err(ProtocolError::InvalidProviderCapabilitySet);
        }
        self.provider_capabilities
            .iter()
            .try_for_each(ProviderCapability::validate)?;
        if usize::from(self.unresolved_operation_count) > MAX_SNAPSHOT_WORKSTREAMS {
            return Err(ProtocolError::SnapshotTooLarge);
        }
        Ok(())
    }
}

/// Bounded, opaque projection of one unresolved host-side creation operation.
/// `source_workstream_id` is present only for a Fork and lets the navigator
/// route a repeated Fork to its matching unfinished operation. It is never
/// rendered and does not disclose provider or filesystem state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub source_workstream_id: Option<WorkstreamId>,
    pub phase: OperationPhase,
    pub revision: i64,
}

impl OperationSnapshot {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.kind != OperationKind::Fork && self.source_workstream_id.is_some() {
            return Err(ProtocolError::InvalidOperationSource);
        }
        Revision::try_from(self.revision)
            .map(|_| ())
            .map_err(|_| ProtocolError::InvalidRevision)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationsResponse {
    pub operations: Vec<OperationSnapshot>,
}

impl OperationsResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.operations.len() > MAX_SNAPSHOT_WORKSTREAMS {
            return Err(ProtocolError::SnapshotTooLarge);
        }
        self.operations
            .iter()
            .try_for_each(OperationSnapshot::validate)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotWorkstream {
    pub workstream_id: WorkstreamId,
    pub location_id: LocationId,
    pub provider: ProviderKind,
    /// Bounded project label derived from the registered repository basename
    /// on the host. This is presentation metadata, never a repository or
    /// project path.
    pub project_display_name: String,
    /// Opaque credential-free canonical fetch-remote identity. `None` keeps
    /// this host location separate in the client Project catalog.
    pub repository_fingerprint: Option<String>,
    /// Credential-free normalized remote label. This is display-only and is
    /// absent when the fetch remote is missing or ambiguous.
    pub remote_identity_display: Option<String>,
    pub display_name: String,
    pub runtime_id: Option<RuntimeId>,
    pub runtime_status: RuntimeStatus,
    pub lifecycle: WorkstreamLifecycle,
    pub archived: bool,
    pub result_ready: bool,
    pub recovery_required: bool,
    pub attention_revision: Option<i64>,
    pub activity_sequence: i64,
    /// Optional host wall-clock metadata for display only. A missing value
    /// means the host has no truthful time for this legacy Workstream.
    pub last_activity_at_millis: Option<i64>,
    pub revision: i64,
}

impl SnapshotWorkstream {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_bounded(
            "project display name",
            &self.project_display_name,
            MAX_DISPLAY_NAME_BYTES,
        )?;
        if let Some(fingerprint) = self.repository_fingerprint.as_deref() {
            validate_repository_fingerprint(fingerprint)?;
        }
        if let Some(display) = self.remote_identity_display.as_deref() {
            validate_remote_identity_display(display)?;
        }
        validate_bounded("display name", &self.display_name, MAX_DISPLAY_NAME_BYTES)?;
        if self.activity_sequence < 0 {
            return Err(ProtocolError::InvalidActivitySequence);
        }
        if self.last_activity_at_millis.is_some_and(|value| value < 0) {
            return Err(ProtocolError::InvalidActivityTimestamp);
        }
        Revision::try_from(self.revision).map_err(|_| ProtocolError::InvalidRevision)?;
        if let Some(revision) = self.attention_revision {
            Revision::try_from(revision).map_err(|_| ProtocolError::InvalidRevision)?;
        }
        Ok(())
    }
}

fn validate_repository_fingerprint(value: &str) -> Result<(), ProtocolError> {
    let Some(hash) = value.strip_prefix("git-remote-v1:") else {
        return Err(ProtocolError::InvalidRepositoryFingerprint);
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProtocolError::InvalidRepositoryFingerprint);
    }
    Ok(())
}

fn validate_remote_identity_display(value: &str) -> Result<(), ProtocolError> {
    validate_bounded("remote identity display", value, MAX_DISPLAY_NAME_BYTES)?;
    if value.chars().any(char::is_control)
        || value.contains(['\\', '@', '?', '#'])
        || value.contains("//")
        || value.starts_with('/')
    {
        return Err(ProtocolError::InvalidRemoteIdentityDisplay);
    }
    Ok(())
}

fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let mut frame = serde_json::to_vec(value).map_err(ProtocolError::Encode)?;
    frame.push(b'\n');
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(frame)
}

fn decode_frame<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<T, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    serde_json::from_slice(frame).map_err(ProtocolError::Decode)
}

fn validate_bounded(name: &'static str, value: &str, maximum: usize) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        return Err(ProtocolError::EmptyField(name));
    }
    if value.len() > maximum {
        return Err(ProtocolError::FieldTooLong { name, maximum });
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(ProtocolError::ControlCharacter(name));
    }
    Ok(())
}

fn validate_relative_browser_path(value: &str) -> Result<(), ProtocolError> {
    if value.len() > MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES {
        return Err(ProtocolError::FieldTooLong {
            name: "project browser relative path",
            maximum: MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES,
        });
    }
    if value.chars().any(char::is_control)
        || value.contains('\\')
        || value.starts_with('/')
        || value.ends_with('/')
        || (!value.is_empty()
            && value
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | "..")))
    {
        return Err(ProtocolError::InvalidProjectBrowserPath);
    }
    Ok(())
}

fn validate_project_browser_root_label(value: &str) -> Result<(), ProtocolError> {
    validate_bounded(
        "project browser root label",
        value,
        MAX_PROJECT_BROWSER_LABEL_BYTES,
    )?;
    if value == "~" {
        return Ok(());
    }
    if let Some(relative_path) = value.strip_prefix("~/") {
        return validate_relative_browser_path(relative_path);
    }
    if value.starts_with('/') || value.contains(['/', '\\']) || value.chars().any(char::is_control)
    {
        return Err(ProtocolError::InvalidProjectBrowserPath);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("{0} cannot be empty")]
    EmptyField(&'static str),
    #[error("{0} contains a newline")]
    ControlCharacter(&'static str),
    #[error("{name} exceeds {maximum} bytes")]
    FieldTooLong { name: &'static str, maximum: usize },
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("revision must be positive")]
    InvalidRevision,
    #[error("operation source is valid only for a Fork")]
    InvalidOperationSource,
    #[error("thread name is invalid")]
    InvalidThreadName,
    #[error("project browser path is invalid")]
    InvalidProjectBrowserPath,
    #[error("activity sequence must not be negative")]
    InvalidActivitySequence,
    #[error("activity timestamp must not be negative")]
    InvalidActivityTimestamp,
    #[error("repository fingerprint is invalid")]
    InvalidRepositoryFingerprint,
    #[error("remote identity display is invalid")]
    InvalidRemoteIdentityDisplay,
    #[error("snapshot has too many workstreams")]
    SnapshotTooLarge,
    #[error("provider capability is inconsistent with its status or reason")]
    InvalidProviderCapability,
    #[error(
        "snapshot provider capabilities must contain each known provider exactly once in order"
    )]
    InvalidProviderCapabilitySet,
    #[error("protocol frame exceeds its maximum size")]
    FrameTooLarge,
    #[error("could not encode protocol frame")]
    Encode(serde_json::Error),
    #[error("could not decode protocol frame")]
    Decode(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_source_is_accepted_only_for_a_fork() {
        let snapshot = OperationSnapshot {
            operation_id: OperationId::new(),
            kind: OperationKind::Start,
            source_workstream_id: Some(WorkstreamId::new()),
            phase: OperationPhase::RecoveryRequired,
            revision: 1,
        };

        assert!(matches!(
            snapshot.validate(),
            Err(ProtocolError::InvalidOperationSource)
        ));
    }

    #[test]
    fn unknown_versions_fail_closed() {
        let envelope = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION + 1,
            request: HostRequest::Snapshot { cursor: None },
        };

        assert!(matches!(
            envelope.validate(),
            Err(ProtocolError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn protocol_sixteen_is_refused_after_the_provider_wire_bump() {
        let envelope = RequestEnvelope {
            version: 16,
            request: HostRequest::Snapshot { cursor: None },
        };

        assert!(matches!(
            envelope.validate(),
            Err(ProtocolError::UnsupportedVersion(16))
        ));

        let response = ResponseEnvelope {
            version: 16,
            response: HostResponse::Rejected {
                diagnostic: "unsupported protocol version".to_owned(),
            },
        };
        assert!(matches!(
            response.validate(),
            Err(ProtocolError::UnsupportedVersion(16))
        ));
    }

    #[test]
    fn unknown_provider_wire_values_fail_closed() {
        let frame = format!(
            "{{\"version\":{CURRENT_PROTOCOL_VERSION},\"request\":{{\"kind\":\"apply\",\"action\":{{\"kind\":\"register_checkout\",\"checkout_path\":\"/tmp/project\",\"provider\":\"other\"}}}}}}\n"
        );

        assert!(matches!(
            RequestEnvelope::decode(frame.as_bytes()),
            Err(ProtocolError::Decode(_))
        ));
    }

    #[test]
    fn snapshot_provider_capabilities_require_the_exact_sorted_set() {
        let mut duplicate = SnapshotResponse::default();
        duplicate.provider_capabilities[1] = duplicate.provider_capabilities[0];
        assert!(matches!(
            duplicate.validate(),
            Err(ProtocolError::InvalidProviderCapabilitySet)
        ));

        let mut out_of_order = SnapshotResponse::default();
        out_of_order.provider_capabilities.swap(0, 1);
        assert!(matches!(
            out_of_order.validate(),
            Err(ProtocolError::InvalidProviderCapabilitySet)
        ));

        let mut missing = SnapshotResponse::default();
        missing.provider_capabilities.pop();
        assert!(matches!(
            missing.validate(),
            Err(ProtocolError::InvalidProviderCapabilitySet)
        ));
    }

    #[test]
    fn snapshot_default_capabilities_are_conservative_evidence() {
        let capabilities = SnapshotResponse::default().provider_capabilities;

        assert_eq!(capabilities[0].kind, ProviderKind::Codex);
        assert_eq!(capabilities[0].status, ProviderCapabilityStatus::Unknown);
        assert_eq!(
            capabilities[0].reason,
            ProviderCapabilityReason::ProbeFailed
        );
        assert!(!capabilities[0].is_new_eligible());

        assert_eq!(capabilities[1].kind, ProviderKind::OpenCode);
        assert_eq!(
            capabilities[1].status,
            ProviderCapabilityStatus::Unavailable
        );
        assert_eq!(
            capabilities[1].reason,
            ProviderCapabilityReason::AdapterUnavailable
        );
        assert!(!capabilities[1].is_new_eligible());
    }

    #[test]
    fn provider_capability_status_and_reason_must_agree() {
        let mut snapshot = SnapshotResponse::default();
        snapshot.provider_capabilities[0].status = ProviderCapabilityStatus::Available;
        snapshot.provider_capabilities[0].reason = ProviderCapabilityReason::NotInstalled;
        assert!(matches!(
            snapshot.validate(),
            Err(ProtocolError::InvalidProviderCapability)
        ));
    }

    #[test]
    fn snapshot_workstream_preserves_its_provider_identity_on_the_wire() {
        let mut snapshot = SnapshotResponse::default();
        snapshot.workstreams.push(SnapshotWorkstream {
            workstream_id: WorkstreamId::new(),
            location_id: LocationId::new(),
            provider: ProviderKind::OpenCode,
            project_display_name: "project".to_owned(),
            repository_fingerprint: None,
            remote_identity_display: None,
            display_name: "thread".to_owned(),
            runtime_id: None,
            runtime_status: RuntimeStatus::Stopped,
            lifecycle: WorkstreamLifecycle::Open,
            archived: false,
            result_ready: false,
            recovery_required: false,
            attention_revision: None,
            activity_sequence: 0,
            last_activity_at_millis: None,
            revision: 1,
        });
        let envelope = ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Snapshot(snapshot),
        };
        let decoded = ResponseEnvelope::decode(&envelope.encode().unwrap()).unwrap();
        assert!(matches!(
            decoded.response,
            HostResponse::Snapshot(SnapshotResponse { workstreams, .. })
                if workstreams[0].provider == ProviderKind::OpenCode
        ));
    }

    #[test]
    fn client_alias_cannot_be_used_as_a_multiline_shell_fragment() {
        let envelope = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Hello {
                client_alias: "trusted\nuntrusted".to_owned(),
            },
        };

        assert!(matches!(
            envelope.validate(),
            Err(ProtocolError::ControlCharacter("client alias"))
        ));
    }

    #[test]
    fn response_diagnostics_are_bounded() {
        let diagnostic = "x".repeat(MAX_DIAGNOSTIC_BYTES + 1);

        assert!(matches!(
            ResponseEnvelope::rejected(diagnostic),
            Err(ProtocolError::FieldTooLong {
                name: "diagnostic",
                ..
            })
        ));
    }

    #[test]
    fn recovery_operation_needs_only_an_opaque_operation_id() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::RecoverOperation {
                    operation_id: OperationId::new(),
                },
            },
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn checkout_registration_accepts_only_one_bounded_single_line_request_path() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::RegisterCheckout {
                    checkout_path: "/workspace/project".to_owned(),
                    provider: ProviderKind::Codex,
                },
            },
        };
        assert!(request.validate().is_ok());

        let invalid = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::RegisterCheckout {
                    checkout_path: "project\nother".to_owned(),
                    provider: ProviderKind::Codex,
                },
            },
        };
        assert!(matches!(
            invalid.validate(),
            Err(ProtocolError::ControlCharacter("checkout path"))
        ));
    }

    #[test]
    fn project_browser_requests_cannot_escape_the_host_root() {
        let accepted = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::ProjectDirectories {
                relative_path: "workspace/wsnav".to_owned(),
            },
        };
        assert!(accepted.validate().is_ok());

        for relative_path in [
            "../outside",
            "/outside",
            "workspace//wsnav",
            "workspace\\wsnav",
        ] {
            let rejected = RequestEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                request: HostRequest::Apply {
                    action: HostAction::RegisterProjectDirectory {
                        relative_path: relative_path.to_owned(),
                        provider: ProviderKind::Codex,
                    },
                },
            };
            assert!(matches!(
                rejected.validate(),
                Err(ProtocolError::InvalidProjectBrowserPath)
            ));
        }
    }

    #[test]
    fn project_browser_responses_contain_only_safe_display_components() {
        let response = ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::ProjectDirectories(ProjectDirectoriesResponse {
                root_label: "~/code".to_owned(),
                relative_path: "workspace".to_owned(),
                entries: vec![ProjectDirectoryEntry {
                    name: "wsnav".to_owned(),
                    is_git_repository: true,
                }],
            }),
        };
        assert!(response.validate().is_ok());

        let invalid = ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::ProjectDirectories(ProjectDirectoriesResponse {
                root_label: "~/code".to_owned(),
                relative_path: String::new(),
                entries: vec![ProjectDirectoryEntry {
                    name: "../../private".to_owned(),
                    is_git_repository: false,
                }],
            }),
        };
        assert!(matches!(
            invalid.validate(),
            Err(ProtocolError::InvalidProjectBrowserPath)
        ));

        let path_leak = ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::ProjectDirectories(ProjectDirectoriesResponse {
                root_label: "/private/host/path".to_owned(),
                relative_path: String::new(),
                entries: Vec::new(),
            }),
        };
        assert!(matches!(
            path_leak.validate(),
            Err(ProtocolError::InvalidProjectBrowserPath)
        ));
    }

    #[test]
    fn frame_round_trip_uses_only_one_bounded_json_document() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Hello {
                client_alias: "laptop".to_owned(),
            },
        };

        let frame = request.encode().unwrap();

        assert!(frame.ends_with(b"\n"));
        assert_eq!(RequestEnvelope::decode(&frame).unwrap(), request);
    }

    #[test]
    fn invalid_action_revision_fails_before_dispatch() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::Archive {
                    workstream_id: WorkstreamId::new(),
                    expected_revision: 0,
                },
            },
        };

        assert!(matches!(
            request.validate(),
            Err(ProtocolError::InvalidRevision)
        ));
    }

    #[test]
    fn workstream_creation_requires_a_bounded_idempotency_key() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::ForkWorkstream {
                    source_workstream_id: WorkstreamId::new(),
                    expected_revision: 1,
                    request_key: "unsafe\nkey".to_owned(),
                },
            },
        };

        assert!(matches!(
            request.validate(),
            Err(ProtocolError::ControlCharacter("request key"))
        ));
    }

    #[test]
    fn oversized_frames_are_rejected_without_parsing() {
        let frame = vec![b'x'; MAX_FRAME_BYTES + 1];

        assert!(matches!(
            RequestEnvelope::decode(&frame),
            Err(ProtocolError::FrameTooLarge)
        ));
    }

    #[test]
    fn remote_project_display_name_is_bounded() {
        let snapshot = SnapshotResponse {
            workstreams: vec![SnapshotWorkstream {
                workstream_id: WorkstreamId::new(),
                location_id: LocationId::new(),
                provider: ProviderKind::Codex,
                project_display_name: "x".repeat(MAX_DISPLAY_NAME_BYTES + 1),
                repository_fingerprint: None,
                remote_identity_display: None,
                display_name: "thread".to_owned(),
                runtime_id: None,
                runtime_status: RuntimeStatus::Idle,
                lifecycle: WorkstreamLifecycle::Open,
                archived: false,
                result_ready: false,
                recovery_required: false,
                attention_revision: None,
                activity_sequence: 0,
                last_activity_at_millis: None,
                revision: 1,
            }],
            unresolved_operation_count: 0,
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: default_provider_capabilities(),
            next_cursor: None,
        };

        assert!(matches!(
            snapshot.validate(),
            Err(ProtocolError::FieldTooLong {
                name: "project display name",
                ..
            })
        ));
    }

    #[test]
    fn observer_lifecycle_actions_are_identifier_free_protocol_requests() {
        for action in [HostAction::PrepareObserver, HostAction::RemoveObserver] {
            let request = RequestEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                request: HostRequest::Apply { action },
            };

            let frame = request.encode().unwrap();
            let decoded = RequestEnvelope::decode(&frame).unwrap();

            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn repository_fingerprint_requires_the_versioned_hash_shape() {
        let mut snapshot = SnapshotResponse {
            workstreams: vec![SnapshotWorkstream {
                workstream_id: WorkstreamId::new(),
                location_id: LocationId::new(),
                provider: ProviderKind::Codex,
                project_display_name: "project".to_owned(),
                repository_fingerprint: Some("https://example.invalid/private.git".to_owned()),
                remote_identity_display: None,
                display_name: "thread".to_owned(),
                runtime_id: None,
                runtime_status: RuntimeStatus::Idle,
                lifecycle: WorkstreamLifecycle::Open,
                archived: false,
                result_ready: false,
                recovery_required: false,
                attention_revision: None,
                activity_sequence: 0,
                last_activity_at_millis: None,
                revision: 1,
            }],
            unresolved_operation_count: 0,
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: default_provider_capabilities(),
            next_cursor: None,
        };
        assert!(matches!(
            snapshot.validate(),
            Err(ProtocolError::InvalidRepositoryFingerprint)
        ));

        snapshot.workstreams[0].repository_fingerprint =
            Some(format!("git-remote-v1:{}", "a".repeat(64)));
        assert!(snapshot.validate().is_ok());
    }

    #[test]
    fn remote_identity_display_rejects_raw_remote_forms() {
        let mut snapshot = SnapshotResponse {
            workstreams: vec![SnapshotWorkstream {
                workstream_id: WorkstreamId::new(),
                location_id: LocationId::new(),
                provider: ProviderKind::Codex,
                project_display_name: "project".to_owned(),
                repository_fingerprint: Some(format!("git-remote-v1:{}", "a".repeat(64))),
                remote_identity_display: Some("github.com/owner/repo".to_owned()),
                display_name: "thread".to_owned(),
                runtime_id: None,
                runtime_status: RuntimeStatus::Idle,
                lifecycle: WorkstreamLifecycle::Open,
                archived: false,
                result_ready: false,
                recovery_required: false,
                attention_revision: None,
                activity_sequence: 0,
                last_activity_at_millis: None,
                revision: 1,
            }],
            unresolved_operation_count: 0,
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: default_provider_capabilities(),
            next_cursor: None,
        };
        assert!(snapshot.validate().is_ok());

        snapshot.workstreams[0].remote_identity_display =
            Some("https://token@github.com/owner/repo?secret=yes".to_owned());
        assert!(matches!(
            snapshot.validate(),
            Err(ProtocolError::InvalidRemoteIdentityDisplay)
        ));
    }
}
