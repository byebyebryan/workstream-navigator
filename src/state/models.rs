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
    OperationPhase, ProjectId, ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeStatus,
    WorkstreamId, WorkstreamLifecycle, WorkstreamOrigin,
};
use crate::protocol::{Capabilities, HelloResponse};
use crate::provider::codex::profile::ProfileOwnership;
use crate::provider::lifecycle::LifecycleHint;
use crate::provider::names::NameState;

use super::utils::{
    default_provider_kind, set_private_directory_permissions, validate_registry_text,
};

pub struct StateRoot {
    base: PathBuf,
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
    #[error("client project display name is invalid")]
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
    #[error("unsupported schema version {0}")]
    UnsupportedSchemaVersion(i64),
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
    #[error("local client catalog host identity does not match the host registry")]
    ClientHostIdentityMismatch,
    #[error("registered host generation no longer matches; reset and register the host again")]
    ClientHostGenerationMismatch,
    #[error("registered host capabilities no longer match; reset and register the host again")]
    ClientHostCapabilitiesMismatch,
    #[error("client host registration does not match the fixed recorded transport")]
    ClientHostRegistrationMismatch,
    #[error("this host identity is already registered under another alias")]
    ClientHostAlreadyRegistered,
    #[error("client host alias is invalid")]
    InvalidClientHostAlias,
    #[error("client host field {0} is invalid")]
    InvalidClientHostField(&'static str),
    #[error("persisted client host capabilities are invalid")]
    InvalidPersistedCapabilities,
    #[error("could not encode client host capabilities")]
    ClientCapabilitiesEncoding(serde_json::Error),
    #[error("client host is unknown")]
    UnknownClientHost,
    #[error("the local client host registration cannot be reset")]
    ClientHostResetRefused,
}

impl StateRoot {
    /// Creates a private state root and applies the host permission policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or its permissions
    /// cannot be restricted.
    pub fn create(base: impl AsRef<Path>) -> Result<Self, StateError> {
        let base = base.as_ref().to_path_buf();
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

    #[must_use]
    pub fn client_database_path(&self) -> PathBuf {
        self.base.join("client.sqlite")
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

/// Client-local project grouping for one registered host location. This is
/// presentation metadata only; the host registry remains operation authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientProjectLocation {
    pub project_id: ProjectId,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
}

/// The transport chosen through an explicit client-side host registration.
/// The host registry remains authoritative for every provider Runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientHostTransport {
    Local,
    Ssh { destination: String },
}

/// The fixed client-side trust record for one local or SSH host. A changed
/// host ID, registry generation, or capabilities is never silently adopted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientHost {
    pub alias: String,
    pub host_id: HostId,
    pub registry_generation: String,
    pub executable_path: PathBuf,
    pub transport: ClientHostTransport,
    pub capabilities: Capabilities,
    pub revision: Revision,
}

impl ClientHost {
    /// Verifies a fresh host handshake against this fixed client registration.
    /// The caller must leave the record untouched on a mismatch and require an
    /// explicit reset/re-registration before any remote mutation.
    ///
    /// # Errors
    ///
    /// Returns an error when the remote host identity, generation, or
    /// capabilities disagree with this registration.
    pub fn verify_hello(&self, hello: &HelloResponse) -> Result<(), StateError> {
        if self.host_id != hello.host_id {
            return Err(StateError::ClientHostIdentityMismatch);
        }
        if self.registry_generation != hello.registry_generation {
            return Err(StateError::ClientHostGenerationMismatch);
        }
        if self.capabilities != hello.capabilities {
            return Err(StateError::ClientHostCapabilitiesMismatch);
        }
        Ok(())
    }
}

/// One registered project root and its initial Workstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalWorkstream {
    pub location_id: LocationId,
    pub workstream_id: WorkstreamId,
}

/// One registered `ProjectLocation` whose presentation metadata has not yet
/// been inspected by a finite control path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRepositoryMetadata {
    pub location_id: LocationId,
    pub repository_path: PathBuf,
}

/// The persisted target of one native provider fork.
///
/// The project root is the exact launch directory for the destination. It is
/// host-private and never returned by snapshots or the SSH protocol.
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

/// One deterministic bounded page of navigator-safe host state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamOverviewPage {
    pub workstreams: Vec<WorkstreamOverview>,
    pub next_cursor: Option<u32>,
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
