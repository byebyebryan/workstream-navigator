use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AttentionState, CheckoutId, Clock, CompoundOperation, DomainError, HostId, IdGenerator,
    LocationId, OperationId, OperationKind, OperationPhase, ProjectId, RandomIdGenerator, Revision,
    RuntimeId, RuntimeStatus, SystemClock, WorkstreamId, WorkstreamLifecycle, WorkstreamOrigin,
};
use crate::protocol::{Capabilities, HelloResponse};
use crate::provider::codex::hooks::{HookObservation, LifecycleEvent};
use crate::provider::codex::names::NameState;
#[cfg(test)]
use crate::provider::codex::profile::OBSERVER_PROFILE_SCHEMA_VERSION;
use crate::provider::codex::profile::{OBSERVER_PROFILE_NAME, ProfileOwnership};

/// The newest host-registry schema this build can open or create.
///
/// This is safe release-probe metadata; it is not a host-state observation.
pub const HOST_SCHEMA_VERSION: i64 = 6;
const CLIENT_SCHEMA_VERSION: i64 = 4;
const MAX_NAVIGATOR_WORKSTREAMS: usize = 128;
const MAX_NAVIGATOR_WORKSTREAM_QUERY: i64 = 129;

const HOST_SCHEMA_SQL: &str = "
    CREATE TABLE host_identity (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        host_id TEXT NOT NULL UNIQUE,
        registry_generation TEXT NOT NULL,
        schema_version INTEGER NOT NULL
    );
    CREATE TABLE codex_integrations (
        integration_id TEXT PRIMARY KEY,
        profile_name TEXT NOT NULL UNIQUE,
        canonical_profile_path TEXT NOT NULL,
        owner_id TEXT NOT NULL,
        profile_schema_version INTEGER NOT NULL,
        hook_executable_path TEXT NOT NULL,
        generated_content_hash TEXT NOT NULL,
        lifecycle TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE project_locations (
        location_id TEXT PRIMARY KEY,
        repository_identity TEXT NOT NULL,
        repository_path TEXT NOT NULL,
        repository_display_name TEXT NOT NULL,
        remote_identity_fingerprint TEXT,
        default_base_ref TEXT NOT NULL,
        managed_worktree_root TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE checkouts (
        checkout_id TEXT PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        ownership TEXT NOT NULL,
        branch TEXT,
        creation_commit TEXT,
        repository_identity TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE workstreams (
        workstream_id TEXT PRIMARY KEY,
        location_id TEXT NOT NULL REFERENCES project_locations(location_id),
        origin TEXT NOT NULL,
        source_workstream_id TEXT REFERENCES workstreams(workstream_id),
        checkout_id TEXT NOT NULL UNIQUE REFERENCES checkouts(checkout_id),
        lifecycle TEXT NOT NULL,
        archived_at_millis INTEGER,
        last_activity_sequence INTEGER NOT NULL CHECK (last_activity_sequence >= 0),
        last_activity_at_millis INTEGER NOT NULL CHECK (last_activity_at_millis >= 0),
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE runtimes (
        runtime_id TEXT PRIMARY KEY,
        workstream_id TEXT NOT NULL UNIQUE REFERENCES workstreams(workstream_id),
        provider TEXT NOT NULL,
        tmux_generation TEXT NOT NULL,
        tmux_session TEXT NOT NULL,
        cwd TEXT NOT NULL,
        process_birth TEXT,
        lifecycle TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE provider_bindings (
        binding_id TEXT PRIMARY KEY,
        runtime_id TEXT NOT NULL UNIQUE REFERENCES runtimes(runtime_id),
        native_session_id TEXT NOT NULL,
        start_source TEXT NOT NULL,
        last_settled_turn_id TEXT,
        observed_thread_name TEXT,
        name_state TEXT NOT NULL,
        name_observed_at INTEGER,
        predecessor_native_session_id TEXT,
        predecessor_effective_name TEXT,
        runtime_generation TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE attention_states (
        workstream_id TEXT PRIMARY KEY,
        result_unseen_since_revision INTEGER,
        recovery_unseen_since_revision INTEGER,
        latest_native_session_id TEXT,
        latest_turn_id TEXT,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE compound_operations (
        operation_id TEXT PRIMARY KEY,
        request_key TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        phase TEXT NOT NULL,
        expected_revisions_json TEXT NOT NULL,
        effect_watermark TEXT,
        outcome_json TEXT,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE INDEX compound_operations_phase_idx ON compound_operations(phase);
";

const CLIENT_SCHEMA_SQL: &str = "
    CREATE TABLE hosts (
        host_alias TEXT PRIMARY KEY,
        host_id TEXT NOT NULL UNIQUE,
        registry_generation TEXT NOT NULL,
        executable_path TEXT NOT NULL,
        transport TEXT NOT NULL CHECK (transport IN ('local', 'ssh')),
        ssh_destination TEXT,
        capabilities_json TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE projects (
        project_id TEXT PRIMARY KEY,
        display_name TEXT NOT NULL,
        repository_fingerprint TEXT,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE project_locations (
        project_id TEXT NOT NULL REFERENCES projects(project_id),
        host_id TEXT NOT NULL,
        location_id TEXT NOT NULL,
        PRIMARY KEY(project_id, host_id, location_id)
    );
    CREATE UNIQUE INDEX project_location_identity_idx
        ON project_locations(host_id, location_id);
    CREATE UNIQUE INDEX project_repository_fingerprint_idx
        ON projects(repository_fingerprint)
        WHERE repository_fingerprint IS NOT NULL;
    CREATE TABLE ignored_project_locations (
        host_id TEXT NOT NULL,
        location_id TEXT NOT NULL,
        PRIMARY KEY(host_id, location_id)
    );
    CREATE TABLE preferences (
        key TEXT PRIMARY KEY,
        value_json TEXT NOT NULL
    );
";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateRoot {
    base: PathBuf,
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
    connection: Connection,
    managed_worktrees_root: PathBuf,
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

/// One V1 external checkout and its initial workstream registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalWorkstream {
    pub location_id: LocationId,
    pub checkout_id: CheckoutId,
    pub workstream_id: WorkstreamId,
    pub checkout_path: PathBuf,
    pub repository_identity: String,
    pub default_base_ref: String,
    pub managed_worktree_root: PathBuf,
}

/// One legacy `ProjectLocation` whose repository presentation metadata has not
/// yet been inspected by a finite control path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRepositoryMetadata {
    pub location_id: LocationId,
    pub repository_path: PathBuf,
}

/// Private Git metadata for creating one sibling Workstream from a registered
/// `ProjectLocation`. It is never returned by snapshots or the SSH protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamGitLocation {
    pub repository_path: PathBuf,
    pub default_base_ref: String,
    pub source_revision: Revision,
}

/// The persisted target of one non-destructive Git-backed Workstream creation.
///
/// The fields are host-private: they are never placed into a remote snapshot or
/// navigator row. The plan exists before Git is invoked so recovery can inspect
/// one exact path/branch/commit without guessing or scanning user worktrees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedWorkstreamPlan {
    pub operation: CompoundOperation,
    pub workstream_id: WorkstreamId,
    pub checkout_id: CheckoutId,
    pub location_id: LocationId,
    pub origin: WorkstreamOrigin,
    pub source_workstream_id: WorkstreamId,
    pub repository_path: PathBuf,
    pub repository_identity: String,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub base_commit: String,
    pub source_runtime_id: Option<RuntimeId>,
    pub source_native_session_id: Option<String>,
    pub last_settled_turn_id: Option<String>,
    pub source_native_name: Option<String>,
    /// Recorded immediately before the one non-idempotent provider fork.
    /// `None` proves this process has not crossed that external-effect point.
    pub fork_attempted_at_millis: Option<i64>,
}

/// One committed managed Workstream record returned after an exact Git effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedWorkstream {
    pub workstream_id: WorkstreamId,
    pub checkout_id: CheckoutId,
    pub location_id: LocationId,
    pub origin: WorkstreamOrigin,
    pub source_workstream_id: WorkstreamId,
    pub revision: Revision,
}

/// The exact durable state returned before any managed Git effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedWorkstreamPreparation {
    pub plan: ManagedWorkstreamPlan,
    /// `true` only when this call atomically recorded the external-effect plan.
    /// A later caller must reconcile the recorded plan rather than run Git again.
    pub newly_prepared: bool,
}

/// Bounded operator-visible state for one unresolved creation operation.
///
/// Request keys, checkout paths, provider identifiers, and raw effect evidence
/// remain host-private. The opaque operation ID is enough to reopen the exact
/// durable plan through the explicit recovery action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationOverview {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub revision: Revision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PersistedManagedWorktreePlan {
    schema_version: u8,
    workstream_id: WorkstreamId,
    checkout_id: CheckoutId,
    location_id: LocationId,
    origin: WorkstreamOrigin,
    source_workstream_id: WorkstreamId,
    repository_path: PathBuf,
    repository_identity: String,
    worktree_path: PathBuf,
    branch: String,
    base_commit: String,
    source_runtime_id: Option<RuntimeId>,
    source_native_session_id: Option<String>,
    last_settled_turn_id: Option<String>,
    #[serde(default)]
    source_native_name: Option<String>,
    #[serde(default)]
    fork_attempted_at_millis: Option<i64>,
}

impl PersistedManagedWorktreePlan {
    fn encode(&self) -> Result<String, StateError> {
        serde_json::to_string(self).map_err(StateError::ManagedWorktreePlanEncoding)
    }

    fn decode(value: Option<&str>) -> Result<Self, StateError> {
        let value = value.ok_or(StateError::MissingManagedWorktreePlan)?;
        let plan: Self =
            serde_json::from_str(value).map_err(StateError::InvalidManagedWorktreePlan)?;
        if plan.schema_version != 1
            || !is_managed_branch(&plan.branch)
            || !is_git_commit(&plan.base_commit)
            || plan.repository_path.as_os_str().is_empty()
            || plan.worktree_path.as_os_str().is_empty()
            || plan.repository_identity.is_empty()
            || plan
                .source_native_name
                .as_deref()
                .is_some_and(|name| name.len() > 512 || name.contains(['\n', '\r']))
            || plan.fork_attempted_at_millis.is_some_and(|time| time < 0)
        {
            return Err(StateError::InvalidManagedWorktreePlanShape);
        }
        Ok(plan)
    }

    fn public_plan(&self, operation: CompoundOperation) -> ManagedWorkstreamPlan {
        ManagedWorkstreamPlan {
            operation,
            workstream_id: self.workstream_id,
            checkout_id: self.checkout_id,
            location_id: self.location_id,
            origin: self.origin,
            source_workstream_id: self.source_workstream_id,
            repository_path: self.repository_path.clone(),
            repository_identity: self.repository_identity.clone(),
            worktree_path: self.worktree_path.clone(),
            branch: self.branch.clone(),
            base_commit: self.base_commit.clone(),
            source_runtime_id: self.source_runtime_id,
            source_native_session_id: self.source_native_session_id.clone(),
            last_settled_turn_id: self.last_settled_turn_id.clone(),
            source_native_name: self.source_native_name.clone(),
            fork_attempted_at_millis: self.fork_attempted_at_millis,
        }
    }
}

/// The persisted record that makes one native tmux process recoverable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeRecord {
    pub runtime_id: RuntimeId,
    pub workstream_id: WorkstreamId,
    pub tmux_generation: String,
    pub tmux_session: String,
    pub cwd: PathBuf,
    pub process_birth: Option<String>,
    pub status: RuntimeStatus,
    pub revision: Revision,
}

/// The exact native Codex session currently bound to a managed runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderBinding {
    pub runtime_id: RuntimeId,
    pub native_session_id: String,
    pub start_source: String,
    pub last_settled_turn_id: Option<String>,
    pub observed_thread_name: Option<String>,
    pub name_state: NameState,
    pub predecessor_native_session_id: Option<String>,
    pub predecessor_effective_name: Option<String>,
    pub revision: Revision,
}

/// One bounded host-local record needed to render and act on a Workstream.
/// It deliberately excludes provider turns, prompts, terminal contents, and
/// process details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstreamOverview {
    pub workstream_id: WorkstreamId,
    pub location_id: LocationId,
    pub project_repository_path: PathBuf,
    pub project_display_name: String,
    pub remote_identity_fingerprint: Option<String>,
    pub checkout_path: PathBuf,
    pub lifecycle: WorkstreamLifecycle,
    /// Archive is an independent visibility state. It preserves all lifecycle,
    /// Runtime, binding, attention, checkout, and lineage records.
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

struct PersistedWorkstreamOverview {
    workstream_id: String,
    location_id: String,
    project_repository_path: String,
    project_display_name: String,
    remote_identity_fingerprint: Option<String>,
    checkout_path: String,
    lifecycle: String,
    archived_at_millis: Option<i64>,
    activity_sequence: i64,
    activity_at_millis: i64,
    revision: i64,
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

impl HostRegistry {
    /// Opens the host registry, applying only known development migrations.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O, `SQLite`, permission, or unsupported-schema
    /// failures.
    pub fn open(root: &StateRoot) -> Result<Self, StateError> {
        Self::open_with_id_generator(root, &RandomIdGenerator)
    }

    /// Opens the host registry with an injected identity source.
    ///
    /// This is a deterministic seam for fresh-registry tests. Production
    /// callers should use [`Self::open`].
    ///
    /// # Errors
    ///
    /// Returns an error for I/O, `SQLite`, permission, or unsupported-schema
    /// failures.
    pub fn open_with_id_generator(
        root: &StateRoot,
        id_generator: &dyn IdGenerator,
    ) -> Result<Self, StateError> {
        let path = root.host_database_path();
        let mut connection = Connection::open(&path).map_err(StateError::Sqlite)?;
        set_private_file_permissions(&path)?;
        configure_connection(&connection)?;
        migrate_host_schema(&mut connection, root.base())?;
        initialize_host_identity(&connection, id_generator)?;
        Ok(Self {
            connection,
            managed_worktrees_root: root.base().join("worktrees"),
        })
    }

    /// Returns the stable identity and generation of this host registry.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity record is missing, malformed, or
    /// cannot be queried.
    pub fn identity(&self) -> Result<HostIdentity, StateError> {
        self.connection
            .query_row(
                "SELECT host_id, registry_generation FROM host_identity WHERE singleton = 1",
                [],
                |row| {
                    let host_id: String = row.get(0)?;
                    let registry_generation: String = row.get(1)?;
                    Ok((host_id, registry_generation))
                },
            )
            .map_err(StateError::Sqlite)
            .and_then(|(host_id, registry_generation)| {
                Uuid::parse_str(&host_id)
                    .map(HostId::from)
                    .map(|host_id| HostIdentity {
                        host_id,
                        registry_generation,
                    })
                    .map_err(StateError::InvalidPersistedUuid)
            })
    }

    /// Returns the host schema version recorded by `SQLite`.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read the schema version.
    pub fn schema_version(&self) -> Result<i64, StateError> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(StateError::Sqlite)
    }

    /// Reads the single `wsnav-observer` ownership record, if it exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be queried or contains invalid
    /// persisted state.
    pub fn codex_integration(&self) -> Result<Option<CodexIntegration>, StateError> {
        self.connection
            .query_row(
                "SELECT canonical_profile_path, owner_id, profile_schema_version,
                    hook_executable_path, generated_content_hash, lifecycle, revision
                 FROM codex_integrations WHERE profile_name = ?1",
                [OBSERVER_PROFILE_NAME],
                row_to_integration,
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Stores an exactly-owned observer profile after an explicit setup action.
    ///
    /// # Errors
    ///
    /// Returns an error if a different ownership record already exists or the
    /// private transaction cannot be committed.
    pub fn record_codex_integration(
        &mut self,
        ownership: ProfileOwnership,
        lifecycle: IntegrationLifecycle,
    ) -> Result<CodexIntegration, StateError> {
        let existing = self.codex_integration()?;
        if let Some(existing) = &existing
            && existing.ownership != ownership
        {
            return Err(StateError::IntegrationOwnershipMismatch);
        }
        let revision = existing
            .as_ref()
            .map_or(Revision::INITIAL, |record| record.revision.next());
        self.connection
            .execute(
                "INSERT INTO codex_integrations (
                integration_id, profile_name, canonical_profile_path, owner_id,
                profile_schema_version, hook_executable_path, generated_content_hash,
                lifecycle, revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(profile_name) DO UPDATE SET
                profile_schema_version = excluded.profile_schema_version,
                lifecycle = excluded.lifecycle, revision = excluded.revision",
                params![
                    Uuid::new_v4().to_string(),
                    OBSERVER_PROFILE_NAME,
                    ownership.canonical_path.to_string_lossy(),
                    ownership.owner_id,
                    i64::from(ownership.profile_schema_version),
                    ownership.hook_executable.to_string_lossy(),
                    ownership.content_hash,
                    integration_lifecycle_text(lifecycle),
                    revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        Ok(CodexIntegration {
            ownership,
            lifecycle,
            revision,
        })
    }

    /// Replaces an already verified observer declaration after an explicit
    /// update. This is the sole state path that may change the recorded hook
    /// executable or declaration hash; the replacement returns to native trust
    /// pending before any managed Runtime can start.
    ///
    /// # Errors
    ///
    /// Returns an error when the expected old ownership is absent or stale, or
    /// the replacement cannot commit atomically.
    pub fn replace_codex_integration(
        &mut self,
        expected: &ProfileOwnership,
        replacement: ProfileOwnership,
        lifecycle: IntegrationLifecycle,
    ) -> Result<CodexIntegration, StateError> {
        let current = self
            .codex_integration()?
            .ok_or(StateError::IntegrationOwnershipMismatch)?;
        if current.ownership != *expected {
            return Err(StateError::IntegrationOwnershipMismatch);
        }
        let revision = current.revision.next();
        let changed = self
            .connection
            .execute(
                "UPDATE codex_integrations SET canonical_profile_path = ?1, owner_id = ?2,
                profile_schema_version = ?3, hook_executable_path = ?4,
                generated_content_hash = ?5, lifecycle = ?6, revision = ?7
             WHERE profile_name = ?8 AND generated_content_hash = ?9 AND revision = ?10",
                params![
                    replacement.canonical_path.to_string_lossy(),
                    replacement.owner_id,
                    i64::from(replacement.profile_schema_version),
                    replacement.hook_executable.to_string_lossy(),
                    replacement.content_hash,
                    integration_lifecycle_text(lifecycle),
                    revision.value(),
                    OBSERVER_PROFILE_NAME,
                    expected.content_hash,
                    current.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        Ok(CodexIntegration {
            ownership: replacement,
            lifecycle,
            revision,
        })
    }

    /// Returns whether any managed runtime is not durably stopped.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime state cannot be queried.
    pub fn has_live_runtime(&self) -> Result<bool, StateError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runtimes WHERE lifecycle != 'stopped')",
                [],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)
    }

    /// Removes the observer ownership row after the exact profile file is removed.
    ///
    /// # Errors
    ///
    /// Returns an error for an absent/mismatched record or a failed state mutation.
    pub fn remove_codex_integration(
        &mut self,
        ownership: &ProfileOwnership,
    ) -> Result<(), StateError> {
        let current = self
            .codex_integration()?
            .ok_or(StateError::IntegrationOwnershipMismatch)?;
        if current.ownership != *ownership {
            return Err(StateError::IntegrationOwnershipMismatch);
        }
        let deleted = self.connection.execute(
            "DELETE FROM codex_integrations WHERE profile_name = ?1 AND generated_content_hash = ?2",
            params![OBSERVER_PROFILE_NAME, ownership.content_hash],
        ).map_err(StateError::Sqlite)?;
        if deleted == 1 {
            Ok(())
        } else {
            Err(StateError::ConcurrentWrite)
        }
    }

    /// Registers one existing local checkout as an external initial workstream.
    ///
    /// # Errors
    ///
    /// Returns an error if an input field is unsafe, the checkout path already
    /// exists in registry state, or the transaction cannot be committed.
    pub fn register_external_workstream(
        &mut self,
        checkout_path: PathBuf,
        repository_identity: String,
        default_base_ref: String,
    ) -> Result<ExternalWorkstream, StateError> {
        let display_name = checkout_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("local project")
            .to_owned();
        let repository_path = checkout_path.clone();
        self.register_external_workstream_with_metadata(
            checkout_path,
            &repository_path,
            repository_identity,
            default_base_ref,
            &display_name,
            None,
        )
    }

    /// Registers an external checkout with separately discovered project-level
    /// repository metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if an input field is unsafe, the checkout path already
    /// exists in registry state, or the transaction cannot be committed.
    #[allow(clippy::too_many_arguments)]
    pub fn register_external_workstream_with_metadata(
        &mut self,
        checkout_path: PathBuf,
        repository_path: &Path,
        repository_identity: String,
        default_base_ref: String,
        repository_display_name: &str,
        remote_identity_fingerprint: Option<&str>,
    ) -> Result<ExternalWorkstream, StateError> {
        validate_registry_text("repository identity", &repository_identity)?;
        validate_registry_text("default base ref", &default_base_ref)?;
        validate_project_display_name(repository_display_name)?;
        validate_repository_fingerprint(remote_identity_fingerprint)?;
        let location_id = LocationId::new();
        let registration = ExternalWorkstream {
            location_id,
            checkout_id: CheckoutId::new(),
            workstream_id: WorkstreamId::new(),
            checkout_path,
            repository_identity,
            default_base_ref,
            managed_worktree_root: self.managed_worktrees_root.join(location_id.to_string()),
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let activity_sequence = next_activity_sequence(&transaction)?;
        transaction
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_identity, repository_path,
                    repository_display_name, remote_identity_fingerprint,
                    default_base_ref, managed_worktree_root, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
                params![
                    registration.location_id.to_string(),
                    registration.repository_identity,
                    repository_path.to_string_lossy(),
                    repository_display_name,
                    remote_identity_fingerprint.unwrap_or(""),
                    registration.default_base_ref,
                    registration.managed_worktree_root.to_string_lossy(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO checkouts (
                    checkout_id, path, ownership, branch, creation_commit,
                    repository_identity, revision
                 ) VALUES (?1, ?2, 'external', NULL, NULL, ?3, 1)",
                params![
                    registration.checkout_id.to_string(),
                    registration.checkout_path.to_string_lossy(),
                    registration.repository_identity,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, origin, source_workstream_id,
                    checkout_id, lifecycle, last_activity_sequence,
                    last_activity_at_millis, revision
                 ) VALUES (?1, ?2, 'external', NULL, ?3, 'open', ?4, ?5, 1)",
                params![
                    registration.workstream_id.to_string(),
                    registration.location_id.to_string(),
                    registration.checkout_id.to_string(),
                    activity_sequence,
                    0_i64,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(registration)
    }

    /// Returns legacy `ProjectLocations` that still need one bounded metadata
    /// refresh after the D6.1 development-schema migration.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed persisted identities or an unavailable
    /// registry.
    pub fn pending_repository_metadata(
        &self,
    ) -> Result<Vec<PendingRepositoryMetadata>, StateError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT location_id, repository_path FROM project_locations
                 WHERE repository_display_name = '' OR remote_identity_fingerprint IS NULL
                 ORDER BY location_id LIMIT ?1",
            )
            .map_err(StateError::Sqlite)?;
        statement
            .query_map([MAX_NAVIGATOR_WORKSTREAM_QUERY], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(StateError::Sqlite)?
            .map(|row| {
                let (location_id, repository_path) = row.map_err(StateError::Sqlite)?;
                Ok(PendingRepositoryMetadata {
                    location_id: Uuid::parse_str(&location_id)
                        .map(LocationId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    repository_path: PathBuf::from(repository_path),
                })
            })
            .collect()
    }

    /// Records one bounded metadata observation for an existing location.
    /// `None` is persisted as an explicit unavailable fingerprint so snapshots
    /// do not repeatedly spawn Git for repositories without a canonical remote.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe metadata, a stale location, or a failed
    /// atomic update.
    pub fn record_repository_metadata(
        &mut self,
        location_id: LocationId,
        repository_path: &Path,
        display_name: &str,
        remote_identity_fingerprint: Option<&str>,
    ) -> Result<(), StateError> {
        validate_project_display_name(display_name)?;
        validate_repository_fingerprint(remote_identity_fingerprint)?;
        let changed = self
            .connection
            .execute(
                "UPDATE project_locations
                 SET repository_path = ?1, repository_display_name = ?2,
                     remote_identity_fingerprint = ?3, revision = revision + 1
                 WHERE location_id = ?4
                   AND (repository_display_name = '' OR remote_identity_fingerprint IS NULL)",
                params![
                    repository_path.to_string_lossy(),
                    display_name,
                    remote_identity_fingerprint.unwrap_or(""),
                    location_id.to_string(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::ConcurrentWrite)
        }
    }

    /// Returns the exact host-private Git location configuration for an
    /// existing Workstream. The source revision lets callers resolve the
    /// configured ref first, then fail closed if a concurrent lifecycle update
    /// changes the source before durable preparation.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream or its registered location is
    /// missing, malformed, or cannot be queried.
    pub fn workstream_git_location(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<WorkstreamGitLocation, StateError> {
        self.workstream_git_location_with_archived(workstream_id, false)
    }

    /// Returns the configured Git location for an independent Workstream
    /// start. An archived source remains a retained `ProjectLocation` and is
    /// therefore a valid base for this one operation; it is not reopened,
    /// resumed, or otherwise made active by the lookup.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream or its registered location is
    /// missing or malformed.
    pub fn workstream_git_location_for_start(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<WorkstreamGitLocation, StateError> {
        self.workstream_git_location_with_archived(workstream_id, true)
    }

    fn workstream_git_location_with_archived(
        &self,
        workstream_id: WorkstreamId,
        include_archived: bool,
    ) -> Result<WorkstreamGitLocation, StateError> {
        let location: (String, String, i64, Option<i64>) = self
            .connection
            .query_row(
                "SELECT project_locations.repository_path,
                        project_locations.default_base_ref,
                        workstreams.revision,
                        workstreams.archived_at_millis
                 FROM workstreams
                 JOIN project_locations ON project_locations.location_id = workstreams.location_id
                 WHERE workstreams.workstream_id = ?1",
                [workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
        if location.3.is_some() && !include_archived {
            return Err(StateError::WorkstreamArchived(workstream_id));
        }
        Ok(WorkstreamGitLocation {
            repository_path: PathBuf::from(location.0),
            default_base_ref: location.1,
            source_revision: Revision::try_from(location.2)?,
        })
    }

    /// Atomically records the exact managed worktree plan before any Git
    /// command is permitted. Reusing a request key returns the original plan
    /// and never allocates a second branch, path, checkout, or Workstream ID.
    ///
    /// The caller must resolve `base_commit` with Git before calling this
    /// method. It is intentionally an exact commit rather than a moving ref.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale/unknown source, unavailable settled fork
    /// boundary, malformed base commit, request mismatch, or state failure.
    pub fn prepare_managed_workstream(
        &mut self,
        request_key: String,
        kind: OperationKind,
        source_workstream_id: WorkstreamId,
        expected_source_revision: Revision,
        base_commit: String,
    ) -> Result<ManagedWorkstreamPreparation, StateError> {
        if !matches!(kind, OperationKind::Start | OperationKind::Fork)
            || !is_git_commit(&base_commit)
        {
            return Err(StateError::InvalidManagedWorktreePlanShape);
        }
        let expected_revisions_json = serde_json::json!({
            "source_workstream_id": source_workstream_id,
            "source_workstream_revision": expected_source_revision,
        })
        .to_string();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;

        if let Some(operation) = load_operation_by_request_key(&transaction, &request_key)? {
            if operation.kind != kind
                || operation.expected_revisions_json != expected_revisions_json
            {
                return Err(StateError::OperationRequestMismatch);
            }
            let plan = PersistedManagedWorktreePlan::decode(operation.effect_watermark.as_deref())?;
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(ManagedWorkstreamPreparation {
                plan: plan.public_plan(operation),
                newly_prepared: false,
            });
        }

        let plan = managed_worktree_plan_for_source(
            &transaction,
            kind,
            source_workstream_id,
            expected_source_revision,
            base_commit,
        )?;
        let mut operation = CompoundOperation::new(request_key, kind, expected_revisions_json)?;
        operation.transition(
            OperationPhase::ExternalEffectStarted,
            Some(plan.encode()?),
            None,
        )?;
        transaction
            .execute(
                "INSERT INTO compound_operations (
                    operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                params![
                    operation.id.to_string(),
                    operation.request_key,
                    operation_kind_text(operation.kind),
                    operation_phase_text(operation.phase),
                    operation.expected_revisions_json,
                    operation.effect_watermark,
                    operation.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(ManagedWorkstreamPreparation {
            plan: plan.public_plan(operation),
            newly_prepared: true,
        })
    }

    /// Commits one exact, already-inspected managed worktree effect. This adds
    /// no Runtime and launches no provider process; callers start the returned
    /// Workstream through the normal explicit start path.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable plan is stale, mismatched, incomplete,
    /// or cannot be atomically recorded as a managed Checkout and Workstream.
    pub fn commit_managed_workstream(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
    ) -> Result<ManagedWorkstream, StateError> {
        self.commit_managed_workstream_with_destination(prepared, None, false)
    }

    /// Commits a Start plan only after the explicit recovery action has
    /// rechecked its exact external Git effect.
    ///
    /// Ordinary creation paths deliberately cannot use this method: a
    /// recovery-required phase is not permission to retry or guess.
    ///
    /// # Errors
    ///
    /// Returns an error when the recovered plan no longer matches its durable
    /// operation or cannot be atomically committed.
    pub fn commit_recovered_managed_workstream(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
    ) -> Result<ManagedWorkstream, StateError> {
        self.commit_managed_workstream_with_destination(prepared, None, true)
    }

    /// Commits a confirmed provider fork together with its destination
    /// Workstream and an exact stopped Runtime binding. The ordinary start
    /// path then launches `codex resume` from that durable binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not a fork, the provider identifier is
    /// unsafe, or the combined durable commit cannot be completed exactly once.
    pub fn commit_forked_managed_workstream(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
        destination_native_session_id: &str,
    ) -> Result<ManagedWorkstream, StateError> {
        if prepared.origin != WorkstreamOrigin::Fork {
            return Err(StateError::ManagedWorktreePlanMismatch);
        }
        validate_provider_metadata(destination_native_session_id)?;
        self.commit_managed_workstream_with_destination(
            prepared,
            Some(destination_native_session_id),
            false,
        )
    }

    /// Commits a Fork plan only after explicit recovery has found exactly one
    /// provider destination for the recorded attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination identifier is invalid, the plan
    /// is stale, or the exact recovered effect cannot be atomically committed.
    pub fn commit_recovered_forked_managed_workstream(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
        destination_native_session_id: &str,
    ) -> Result<ManagedWorkstream, StateError> {
        if prepared.origin != WorkstreamOrigin::Fork {
            return Err(StateError::ManagedWorktreePlanMismatch);
        }
        validate_provider_metadata(destination_native_session_id)?;
        self.commit_managed_workstream_with_destination(
            prepared,
            Some(destination_native_session_id),
            true,
        )
    }

    fn commit_managed_workstream_with_destination(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
        destination_native_session_id: Option<&str>,
        allow_recovery_required: bool,
    ) -> Result<ManagedWorkstream, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let mut operation = load_operation_by_id(&transaction, prepared.operation.id)?
            .ok_or(StateError::UnknownOperation(prepared.operation.id))?;
        let persisted =
            PersistedManagedWorktreePlan::decode(operation.effect_watermark.as_deref())?;
        if operation.id != prepared.operation.id
            || operation.kind != prepared.operation.kind
            || persisted.public_plan(prepared.operation.clone()) != *prepared
        {
            return Err(StateError::ManagedWorktreePlanMismatch);
        }
        if operation.phase == OperationPhase::Committed {
            let created = managed_workstream_from_outcome(
                &transaction,
                &operation,
                &persisted,
                destination_native_session_id,
            )?;
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(created);
        }
        if operation.phase != OperationPhase::ExternalEffectStarted
            && !(allow_recovery_required && operation.phase == OperationPhase::RecoveryRequired)
        {
            return Err(StateError::ManagedWorktreeOperationUnavailable);
        }
        if matches!(persisted.origin, WorkstreamOrigin::Fork)
            != destination_native_session_id.is_some()
        {
            return Err(StateError::ManagedWorktreePlanMismatch);
        }
        insert_managed_workstream_records(&transaction, &persisted, destination_native_session_id)?;
        commit_managed_operation(
            &transaction,
            &mut operation,
            &persisted,
            destination_native_session_id,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(managed_workstream_from_plan(&persisted))
    }

    /// Marks an unresolved managed Git operation as recovery-required without
    /// changing any existing Checkout, branch, or Workstream.
    ///
    /// # Errors
    ///
    /// Returns an error when the prepared operation is stale, already terminal,
    /// or cannot transition atomically.
    pub fn mark_managed_workstream_recovery(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
    ) -> Result<(), StateError> {
        if prepared.operation.phase == OperationPhase::RecoveryRequired {
            return Ok(());
        }
        let operation = self.transition_operation(
            prepared.operation.id,
            prepared.operation.revision,
            OperationPhase::RecoveryRequired,
            prepared.operation.effect_watermark.clone(),
            None,
        )?;
        if operation.kind != prepared.operation.kind {
            return Err(StateError::ManagedWorktreePlanMismatch);
        }
        Ok(())
    }

    /// Atomically records the exact instant at which a fork request may be
    /// sent to Codex. Once this succeeds, callers must reconcile provider
    /// evidence and must never issue another `thread/fork` request.
    ///
    /// # Errors
    ///
    /// Returns an error when the prepared plan is stale, not a pending fork,
    /// already has a provider attempt marker, or cannot be updated exactly.
    pub fn record_managed_fork_attempt(
        &mut self,
        prepared: &ManagedWorkstreamPlan,
    ) -> Result<ManagedWorkstreamPlan, StateError> {
        if prepared.origin != WorkstreamOrigin::Fork {
            return Err(StateError::ManagedWorktreePlanMismatch);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let operation = load_operation_by_id(&transaction, prepared.operation.id)?
            .ok_or(StateError::UnknownOperation(prepared.operation.id))?;
        let mut persisted =
            PersistedManagedWorktreePlan::decode(operation.effect_watermark.as_deref())?;
        if operation != prepared.operation
            || persisted.public_plan(operation.clone()) != *prepared
            || !matches!(
                operation.phase,
                OperationPhase::ExternalEffectStarted | OperationPhase::RecoveryRequired
            )
            || persisted.fork_attempted_at_millis.is_some()
        {
            return Err(StateError::ManagedWorktreeOperationUnavailable);
        }
        persisted.fork_attempted_at_millis = Some(SystemClock.now_millis()?);
        let next_revision = operation.revision.next();
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET effect_watermark = ?1, revision = ?2
                 WHERE operation_id = ?3 AND revision = ?4
                   AND phase IN ('external_effect_started', 'recovery_required')",
                params![
                    persisted.encode()?,
                    next_revision.value(),
                    operation.id.to_string(),
                    operation.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        let mut next_operation = operation;
        next_operation.revision = next_revision;
        next_operation.effect_watermark = Some(persisted.encode()?);
        Ok(persisted.public_plan(next_operation))
    }

    /// Reserves the single Runtime record for an open workstream before launch.
    ///
    /// # Errors
    ///
    /// Returns an error when the workstream is unknown, not open, already live,
    /// or durable state cannot be changed.
    pub fn reserve_runtime(
        &mut self,
        workstream_id: WorkstreamId,
    ) -> Result<RuntimeRecord, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (checkout_path, workstream_lifecycle, archived_at_millis): (String, String, Option<i64>) = transaction
            .query_row(
                "SELECT checkouts.path, workstreams.lifecycle, workstreams.archived_at_millis FROM workstreams
                 JOIN checkouts ON checkouts.checkout_id = workstreams.checkout_id
                 WHERE workstreams.workstream_id = ?1
                   AND workstreams.lifecycle IN ('open', 'parked')",
                [workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
        if archived_at_millis.is_some() {
            return Err(StateError::WorkstreamArchived(workstream_id));
        }
        let current: Option<RuntimeRecord> = transaction
            .query_row(
                "SELECT runtime_id, tmux_generation, tmux_session, cwd, process_birth, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let generation = Uuid::new_v4().to_string();
        let record = if let Some(current) = current {
            if !matches!(
                current.status,
                RuntimeStatus::Stopped | RuntimeStatus::Unknown
            ) {
                return Err(StateError::RuntimeAlreadyLive(workstream_id));
            }
            let next = RuntimeRecord {
                tmux_generation: generation,
                tmux_session: format!("wsnav-{}", current.runtime_id),
                cwd: PathBuf::from(checkout_path),
                process_birth: None,
                status: RuntimeStatus::Starting,
                revision: current.revision.next(),
                ..current
            };
            transaction
                .execute(
                    "UPDATE runtimes SET tmux_generation = ?1, tmux_session = ?2, cwd = ?3,
                     process_birth = NULL, lifecycle = 'starting', revision = ?4
                     WHERE runtime_id = ?5 AND revision = ?6",
                    params![
                        next.tmux_generation,
                        next.tmux_session,
                        next.cwd.to_string_lossy(),
                        next.revision.value(),
                        next.runtime_id.to_string(),
                        current.revision.value()
                    ],
                )
                .map_err(StateError::Sqlite)?;
            next
        } else {
            let runtime_id = RuntimeId::new();
            let record = RuntimeRecord {
                runtime_id,
                workstream_id,
                tmux_generation: generation,
                tmux_session: format!("wsnav-{runtime_id}"),
                cwd: PathBuf::from(checkout_path),
                process_birth: None,
                status: RuntimeStatus::Starting,
                revision: Revision::INITIAL,
            };
            transaction
                .execute(
                    "INSERT INTO runtimes (
                    runtime_id, workstream_id, provider, tmux_generation, tmux_session,
                    cwd, process_birth, lifecycle, revision
                 ) VALUES (?1, ?2, 'codex', ?3, ?4, ?5, NULL, 'starting', 1)",
                    params![
                        record.runtime_id.to_string(),
                        workstream_id.to_string(),
                        record.tmux_generation,
                        record.tmux_session,
                        record.cwd.to_string_lossy()
                    ],
                )
                .map_err(StateError::Sqlite)?;
            record
        };
        if workstream_lifecycle == "parked" {
            reopen_parked_workstream(&transaction, workstream_id)?;
        } else {
            touch_workstream(&transaction, &workstream_id.to_string(), None)?;
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(record)
    }

    /// Reserves a new private tmux generation for an explicitly recovering
    /// Workstream. The Workstream remains `recovery_required` until a verified
    /// native `SessionStart(source=resume)` binds the launched Codex process.
    ///
    /// # Errors
    ///
    /// Returns an error unless this Workstream has one runtime in the exact
    /// `unknown` state established by [`Self::mark_runtime_recovery_required`].
    pub fn reserve_runtime_recovery(
        &mut self,
        workstream_id: WorkstreamId,
    ) -> Result<RuntimeRecord, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (checkout_path, archived_at_millis): (String, Option<i64>) = transaction
            .query_row(
                "SELECT checkouts.path, workstreams.archived_at_millis FROM workstreams
                 JOIN checkouts ON checkouts.checkout_id = workstreams.checkout_id
                 WHERE workstreams.workstream_id = ?1
                   AND workstreams.lifecycle = 'recovery_required'",
                [workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::RecoveryUnavailable(workstream_id))?;
        if archived_at_millis.is_some() {
            return Err(StateError::WorkstreamArchived(workstream_id));
        }
        let current: RuntimeRecord = transaction
            .query_row(
                "SELECT runtime_id, tmux_generation, tmux_session, cwd, process_birth, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1 AND lifecycle = 'unknown'",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::RecoveryUnavailable(workstream_id))?;
        let next = RuntimeRecord {
            tmux_generation: Uuid::new_v4().to_string(),
            tmux_session: format!("wsnav-{}", current.runtime_id),
            cwd: PathBuf::from(checkout_path),
            process_birth: None,
            status: RuntimeStatus::Starting,
            revision: current.revision.next(),
            ..current
        };
        let changed = transaction
            .execute(
                "UPDATE runtimes SET tmux_generation = ?1, tmux_session = ?2, cwd = ?3,
                 process_birth = NULL, lifecycle = 'starting', revision = ?4
                 WHERE runtime_id = ?5 AND revision = ?6 AND lifecycle = 'unknown'",
                params![
                    next.tmux_generation,
                    next.tmux_session,
                    next.cwd.to_string_lossy(),
                    next.revision.value(),
                    next.runtime_id.to_string(),
                    current.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        touch_workstream(&transaction, &workstream_id.to_string(), None)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(next)
    }

    /// Reads the single persisted runtime record for a workstream.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains invalid
    /// persisted runtime data.
    pub fn runtime_for_workstream(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        self.connection
            .query_row(
            "SELECT runtime_id, tmux_generation, tmux_session, cwd, process_birth, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Reads one exact persisted Runtime by its opaque identity.
    ///
    /// This is used only to validate an explicit native terminal attachment.
    /// It does not expose checkout paths or tmux details to a remote caller.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains an
    /// invalid persisted Runtime record.
    pub fn runtime_by_id(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        self.connection
            .query_row(
                "SELECT workstream_id, tmux_generation, tmux_session, cwd, process_birth,
                        lifecycle, revision
                 FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| {
                    let workstream_id: String = row.get(0)?;
                    let workstream_id = Uuid::parse_str(&workstream_id)
                        .map(WorkstreamId::from)
                        .map_err(to_from_sql_error)?;
                    row_to_runtime_with_id(row, runtime_id, workstream_id)
                },
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Returns only current, process-fingerprinted private Runtimes that may
    /// corroborate a passive Codex hook. This is host-local evidence; callers
    /// must still probe the exact private tmux pane and require one match.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted Runtime identity is malformed or the
    /// private registry cannot be queried.
    pub fn hook_runtime_candidates(&self) -> Result<Vec<RuntimeRecord>, StateError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT runtime_id, workstream_id, tmux_generation, tmux_session,
                        cwd, process_birth, lifecycle, revision
                 FROM runtimes
                 WHERE lifecycle IN ('starting', 'idle', 'working', 'attention')
                   AND process_birth IS NOT NULL",
            )
            .map_err(StateError::Sqlite)?;
        statement
            .query_map([], |row| {
                let runtime_id: String = row.get(0)?;
                let workstream_id: String = row.get(1)?;
                let tmux_generation: String = row.get(2)?;
                let tmux_session: String = row.get(3)?;
                let cwd: String = row.get(4)?;
                let process_birth: Option<String> = row.get(5)?;
                let lifecycle: String = row.get(6)?;
                let revision: i64 = row.get(7)?;
                Ok((
                    runtime_id,
                    workstream_id,
                    tmux_generation,
                    tmux_session,
                    cwd,
                    process_birth,
                    lifecycle,
                    revision,
                ))
            })
            .map_err(StateError::Sqlite)?
            .map(|row| {
                let (
                    runtime_id,
                    workstream_id,
                    tmux_generation,
                    tmux_session,
                    cwd,
                    process_birth,
                    lifecycle,
                    revision,
                ) = row.map_err(StateError::Sqlite)?;
                Ok(RuntimeRecord {
                    runtime_id: Uuid::parse_str(&runtime_id)
                        .map(RuntimeId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    workstream_id: Uuid::parse_str(&workstream_id)
                        .map(WorkstreamId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    tmux_generation,
                    tmux_session,
                    cwd: PathBuf::from(cwd),
                    process_birth,
                    status: runtime_status_from_text(&lifecycle)?,
                    revision: Revision::try_from(revision)?,
                })
            })
            .collect()
    }

    /// Confirms that one exact Runtime ended through the explicit park action.
    ///
    /// This is intentionally stricter than a stopped runtime alone: an
    /// unexpected native-process exit also leaves a Runtime stopped, but does
    /// not park its Workstream. Attachment helpers use this distinction after
    /// their private tmux client exits unexpectedly.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains an
    /// invalid persisted lifecycle value.
    pub fn runtime_is_deliberately_parked(
        &self,
        runtime_id: RuntimeId,
        workstream_id: WorkstreamId,
    ) -> Result<bool, StateError> {
        let lifecycle: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT runtimes.lifecycle, workstreams.lifecycle
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1 AND runtimes.workstream_id = ?2",
                params![runtime_id.to_string(), workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((runtime_lifecycle, workstream_lifecycle)) = lifecycle else {
            return Ok(false);
        };
        Ok(
            runtime_status_from_text(&runtime_lifecycle)? == RuntimeStatus::Stopped
                && workstream_lifecycle_from_text(&workstream_lifecycle)?
                    == WorkstreamLifecycle::Parked,
        )
    }

    /// Returns the bounded state needed by one local navigator snapshot.
    /// Provider content, terminal captures, and hook payloads are not queried
    /// or returned.
    ///
    /// # Errors
    ///
    /// Returns an error when a persisted identity, lifecycle, or revision is
    /// malformed, or when the registry cannot be queried.
    pub fn workstream_overviews(&self) -> Result<Vec<WorkstreamOverview>, StateError> {
        let mut workstreams = Vec::new();
        let mut cursor = 0;
        loop {
            let page = self.workstream_overview_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
            workstreams.extend(page.workstreams);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(workstreams);
            };
            cursor = next_cursor;
        }
    }

    /// Hides one exact Workstream from the active navigator scope without
    /// deleting its Runtime, provider binding, attention, checkout, or
    /// lineage. The caller is responsible for any necessary Runtime park
    /// before this durable visibility transition.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream is missing, already archived, its
    /// revision is stale, the timestamp is invalid, or the transaction fails.
    pub fn archive_workstream(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        archived_at_millis: i64,
    ) -> Result<Revision, StateError> {
        if archived_at_millis < 0 {
            return Err(StateError::InvalidRegistryField("archive timestamp"));
        }
        self.transition_workstream_archive(
            workstream_id,
            expected_revision,
            Some(archived_at_millis),
        )
    }

    /// Returns one archived Workstream to the active navigator scope without
    /// starting or resuming a provider Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream is missing, not archived, its
    /// revision is stale, or the transaction fails.
    pub fn restore_workstream(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    ) -> Result<Revision, StateError> {
        self.transition_workstream_archive(workstream_id, expected_revision, None)
    }

    fn transition_workstream_archive(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        archived_at_millis: Option<i64>,
    ) -> Result<Revision, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let existing = transaction
            .query_row(
                "SELECT revision, archived_at_millis FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
        let current_revision = Revision::try_from(existing.0)?;
        if current_revision != expected_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: current_revision,
            }));
        }
        match (existing.1, archived_at_millis) {
            (Some(_), Some(_)) => return Err(StateError::WorkstreamAlreadyArchived(workstream_id)),
            (None, None) => return Err(StateError::WorkstreamNotArchived(workstream_id)),
            (None, Some(_)) | (Some(_), None) => {}
        }
        let next_revision = current_revision.next();
        let updated = transaction
            .execute(
                "UPDATE workstreams SET archived_at_millis = ?1, revision = ?2
                 WHERE workstream_id = ?3 AND revision = ?4",
                params![
                    archived_at_millis,
                    next_revision.value(),
                    workstream_id.to_string(),
                    current_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(next_revision)
    }

    /// Returns one deterministic bounded Workstream page ordered by latest
    /// activity, checkout, and opaque Workstream identity.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid page size, cursor overflow, malformed
    /// persisted state, or an unavailable registry.
    pub fn workstream_overview_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<WorkstreamOverviewPage, StateError> {
        if page_size == 0 || page_size > MAX_NAVIGATOR_WORKSTREAMS {
            return Err(StateError::InvalidNavigatorPageSize);
        }
        let query_limit =
            i64::try_from(page_size + 1).map_err(|_| StateError::InvalidNavigatorPageSize)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT workstreams.workstream_id, workstreams.location_id,
                        project_locations.repository_path,
                        project_locations.repository_display_name,
                        project_locations.remote_identity_fingerprint,
                        checkouts.path,
                        workstreams.lifecycle,
                        workstreams.archived_at_millis,
                        workstreams.last_activity_sequence,
                        workstreams.last_activity_at_millis, workstreams.revision
                 FROM workstreams
                 JOIN project_locations
                   ON project_locations.location_id = workstreams.location_id
                 JOIN checkouts ON checkouts.checkout_id = workstreams.checkout_id
                 ORDER BY workstreams.last_activity_sequence DESC,
                          checkouts.path, workstreams.workstream_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(StateError::Sqlite)?;
        let mut bases = statement
            .query_map(params![query_limit, i64::from(cursor)], |row| {
                Ok(PersistedWorkstreamOverview {
                    workstream_id: row.get(0)?,
                    location_id: row.get(1)?,
                    project_repository_path: row.get(2)?,
                    project_display_name: row.get(3)?,
                    remote_identity_fingerprint: row.get(4)?,
                    checkout_path: row.get(5)?,
                    lifecycle: row.get(6)?,
                    archived_at_millis: row.get(7)?,
                    activity_sequence: row.get(8)?,
                    activity_at_millis: row.get(9)?,
                    revision: row.get(10)?,
                })
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        let has_more = bases.len() > page_size;
        bases.truncate(page_size);
        let page_len =
            u32::try_from(bases.len()).map_err(|_| StateError::NavigatorCursorOverflow)?;
        let next_cursor = has_more
            .then(|| {
                cursor
                    .checked_add(page_len)
                    .ok_or(StateError::NavigatorCursorOverflow)
            })
            .transpose()?;
        let workstreams = bases
            .into_iter()
            .map(|base| self.hydrate_workstream_overview(base))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkstreamOverviewPage {
            workstreams,
            next_cursor,
        })
    }

    fn hydrate_workstream_overview(
        &self,
        base: PersistedWorkstreamOverview,
    ) -> Result<WorkstreamOverview, StateError> {
        let workstream_id = Uuid::parse_str(&base.workstream_id)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let location_id = Uuid::parse_str(&base.location_id)
            .map(LocationId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let lifecycle = workstream_lifecycle_from_text(&base.lifecycle)?;
        let revision = Revision::try_from(base.revision)?;
        let runtime = self.runtime_for_workstream(workstream_id)?;
        let binding = runtime
            .as_ref()
            .map(|runtime| self.binding_for_runtime(runtime.runtime_id))
            .transpose()?
            .flatten();
        let attention = self.attention(workstream_id)?;
        Ok(WorkstreamOverview {
            workstream_id,
            location_id,
            project_repository_path: PathBuf::from(base.project_repository_path),
            project_display_name: base.project_display_name,
            remote_identity_fingerprint: base
                .remote_identity_fingerprint
                .filter(|fingerprint| !fingerprint.is_empty()),
            checkout_path: PathBuf::from(base.checkout_path),
            lifecycle,
            archived_at_millis: base.archived_at_millis,
            last_activity_sequence: base.activity_sequence,
            last_activity_at_millis: (base.activity_at_millis != 0)
                .then_some(base.activity_at_millis),
            revision,
            runtime,
            binding,
            attention,
        })
    }

    /// Lists only durable creation operations that still require an explicit
    /// operator decision. This is presentation metadata, not provider or
    /// checkout discovery.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded operation projection cannot be read
    /// or contains an invalid persisted identity, kind, phase, or revision.
    pub fn unresolved_operation_overviews(&self) -> Result<Vec<OperationOverview>, StateError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT operation_id, kind, phase, revision
                 FROM compound_operations
                 WHERE phase IN ('external_effect_started', 'awaiting_reconciliation', 'recovery_required')
                 ORDER BY operation_id
                 LIMIT ?1",
            )
            .map_err(StateError::Sqlite)?;
        let operations = statement
            .query_map([MAX_NAVIGATOR_WORKSTREAM_QUERY], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        if operations.len() > MAX_NAVIGATOR_WORKSTREAMS {
            return Err(StateError::NavigatorSnapshotTooLarge);
        }
        operations
            .into_iter()
            .map(|(operation_id, kind, phase, revision)| {
                Ok(OperationOverview {
                    operation_id: Uuid::parse_str(&operation_id)
                        .map(OperationId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    kind: operation_kind_from_text(&kind)?,
                    phase: operation_phase_from_text(&phase)?,
                    revision: Revision::try_from(revision)?,
                })
            })
            .collect()
    }

    /// Loads the one host-private managed-worktree plan owned by an explicit
    /// operation ID. It never scans worktrees or provider history.
    ///
    /// # Errors
    ///
    /// Returns an error when the operation is unknown, has no valid managed
    /// plan, or contains malformed persisted state.
    pub fn managed_workstream_plan(
        &self,
        operation_id: OperationId,
    ) -> Result<ManagedWorkstreamPlan, StateError> {
        let operation = self
            .connection
            .query_row(
                "SELECT operation_id, request_key, kind, phase, expected_revisions_json,
                        effect_watermark, outcome_json, revision
                 FROM compound_operations WHERE operation_id = ?1",
                [operation_id.to_string()],
                row_to_operation,
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOperation(operation_id))?;
        let plan = PersistedManagedWorktreePlan::decode(operation.effect_watermark.as_deref())?;
        Ok(plan.public_plan(operation))
    }

    /// Reads the current exact native-session binding for one runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains invalid
    /// persisted binding data.
    pub fn binding_for_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<ProviderBinding>, StateError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let binding = load_binding(&transaction, runtime_id)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(binding)
    }

    /// Persists the exact private-pane process birth only while the Runtime is
    /// prepared for its initial native lifecycle binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime is stale, not starting, or the birth
    /// token is invalid.
    pub fn record_runtime_process_birth(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
        process_birth: &str,
    ) -> Result<(), StateError> {
        validate_registry_text("process birth", process_birth)?;
        let changed = self
            .connection
            .execute(
                "UPDATE runtimes SET process_birth = ?1, revision = revision + 1
                 WHERE runtime_id = ?2 AND lifecycle = 'starting' AND revision = ?3",
                params![process_birth, runtime_id.to_string(), expected.value()],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::ConcurrentWrite)
        }
    }

    /// Returns the prepared provider process fingerprint for one exact runtime
    /// generation. This is evidence for hook ancestry, never hook authority by
    /// itself.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime/generation is unknown or no process
    /// fingerprint was recorded for the prepared launch.
    pub fn expected_hook_process_birth(
        &self,
        runtime_id: RuntimeId,
        generation: &str,
    ) -> Result<String, StateError> {
        let row: Option<(String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT tmux_generation, process_birth FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((recorded_generation, process_birth)) = row else {
            return Err(StateError::UnknownRuntime(runtime_id));
        };
        if recorded_generation != generation {
            return Err(StateError::HookEvidenceMismatch);
        }
        process_birth.ok_or(StateError::HookEvidenceMismatch)
    }

    /// Caches an exact managed thread name after a successful canonical provider mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding is missing, changed, or cannot be
    /// transactionally updated.
    pub fn record_thread_name(
        &mut self,
        runtime_id: RuntimeId,
        native_session_id: &str,
        name: &str,
    ) -> Result<(), StateError> {
        self.record_thread_metadata(runtime_id, native_session_id, Some(name))
    }

    /// Records only the bounded canonical name from an exact provider metadata
    /// read. A missing native name is distinct from an unavailable read; the
    /// latter leaves the existing cached value untouched.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding is missing, changed, or cannot be
    /// transactionally updated.
    pub fn record_thread_metadata(
        &mut self,
        runtime_id: RuntimeId,
        native_session_id: &str,
        name: Option<&str>,
    ) -> Result<(), StateError> {
        let (name, name_state) = match name.filter(|value| !value.trim().is_empty()) {
            Some(name) => {
                validate_registry_text("thread name", name)?;
                (Some(name), "named")
            }
            None => (None, "known_empty"),
        };
        let changed = self
            .connection
            .execute(
                "UPDATE provider_bindings SET observed_thread_name = ?1, name_state = ?2,
             revision = revision + 1 WHERE runtime_id = ?3 AND native_session_id = ?4",
                params![name, name_state, runtime_id.to_string(), native_session_id],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::HookEvidenceMismatch)
        }
    }

    /// Marks the reserved Runtime stopped after its exact private tmux server is parked.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown runtime, stale state, or failed transaction.
    pub fn mark_runtime_stopped(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
    ) -> Result<(), StateError> {
        let changed = self
            .connection
            .execute(
                "UPDATE runtimes SET lifecycle = 'stopped', revision = revision + 1
             WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        Ok(())
    }

    /// Records that an owned private Runtime disappeared without a deliberate
    /// park or verified native end. Its provider binding and checkout are
    /// retained, but neither a blank start nor a stale hook may continue it.
    ///
    /// This operation is idempotent after the first transition so cleanup of a
    /// failed recovery launch cannot erase the original recovery evidence.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown or stale runtime, or a failed atomic
    /// transition of the Runtime, Workstream, and attention state.
    pub fn mark_runtime_recovery_required(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
    ) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let workstream_id: String = transaction
            .query_row(
                "SELECT workstream_id FROM runtimes
                 WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        let workstream_id = Uuid::parse_str(&workstream_id)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let runtime_changed = transaction
            .execute(
                "UPDATE runtimes SET lifecycle = 'unknown', revision = revision + 1
                 WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
            )
            .map_err(StateError::Sqlite)?;
        if runtime_changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        let activity_sequence = next_activity_sequence(&transaction)?;
        let workstream_changed = transaction
            .execute(
                "UPDATE workstreams SET lifecycle = 'recovery_required',
                 last_activity_sequence = ?1, revision = revision + 1
                 WHERE workstream_id = ?2 AND lifecycle IN ('open', 'parked')",
                params![activity_sequence, workstream_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        if workstream_changed == 0 {
            let lifecycle: String = transaction
                .query_row(
                    "SELECT lifecycle FROM workstreams WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(StateError::Sqlite)?;
            if lifecycle != "recovery_required" {
                return Err(StateError::ConcurrentWrite);
            }
        }
        ensure_recovery_attention_in_transaction(&transaction, workstream_id)?;
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Records an explicit user park after the exact private tmux server has
    /// stopped. Provider history and the checkout are retained, while the
    /// Workstream's durable lifecycle becomes `parked`.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown or stale runtime, or when the
    /// Workstream state cannot be updated atomically with the stopped Runtime.
    pub fn park_runtime(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
    ) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let workstream_id: String = transaction
            .query_row(
                "SELECT workstream_id FROM runtimes WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        let runtime_changed = transaction
            .execute(
                "UPDATE runtimes SET lifecycle = 'stopped', revision = revision + 1
                 WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
            )
            .map_err(StateError::Sqlite)?;
        let activity_sequence = next_activity_sequence(&transaction)?;
        let workstream_changed = transaction
            .execute(
                "UPDATE workstreams SET lifecycle = CASE
                    WHEN lifecycle = 'open' THEN 'parked' ELSE lifecycle END,
                    last_activity_sequence = ?1,
                    revision = revision + 1
                 WHERE workstream_id = ?2",
                params![activity_sequence, workstream_id],
            )
            .map_err(StateError::Sqlite)?;
        if runtime_changed != 1 || workstream_changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Applies one already-authorized lifecycle observation to its exact runtime.
    ///
    /// Hooks supply evidence only: an initial session can bind solely while the
    /// runtime is `starting`. The one proven native same-TUI replacement is a
    /// distinct `SessionStart(source=clear)` after an idle or attention state;
    /// all other replacement claims fail closed. A settled result and its
    /// sticky attention state commit in the same `SQLite` transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime generation, cwd, binding, lifecycle, or
    /// revision evidence is ambiguous or does not match a managed runtime.
    pub fn apply_hook_observation(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        observation: HookObservation,
    ) -> Result<(), StateError> {
        let activity_at_millis = match observation.event {
            LifecycleEvent::UserPromptSubmit | LifecycleEvent::Stop => {
                Some(SystemClock.now_millis()?)
            }
            LifecycleEvent::SessionStart | LifecycleEvent::SessionEnd => None,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let runtime = transaction
            .query_row(
                "SELECT runtimes.workstream_id, runtimes.tmux_generation, runtimes.cwd,
                        runtimes.lifecycle, runtimes.revision, workstreams.lifecycle
                 FROM runtimes JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1",
                [runtime_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownRuntime(runtime_id))?;
        let workstream_id = Uuid::parse_str(&runtime.0)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let revision = Revision::try_from(runtime.4)?;
        if runtime.1 != generation || runtime.2 != observation.cwd {
            return Err(StateError::HookEvidenceMismatch);
        }
        let existing = load_binding(&transaction, runtime_id)?;
        match observation.event {
            LifecycleEvent::SessionStart => apply_session_start(
                &transaction,
                &SessionStartContext {
                    runtime_id,
                    runtime_status: &runtime.3,
                    runtime_revision: revision,
                    generation,
                    workstream_id,
                    workstream_lifecycle: workstream_lifecycle_from_text(&runtime.5)?,
                },
                existing,
                &observation.native_session_id,
                observation.source.as_deref(),
            )?,
            LifecycleEvent::UserPromptSubmit => {
                require_matching_binding(existing.as_ref(), &observation.native_session_id)?;
                update_runtime_lifecycle(&transaction, runtime_id, revision, "working")?;
            }
            LifecycleEvent::Stop => {
                let turn_id = observation
                    .turn_id
                    .ok_or(StateError::HookEvidenceMismatch)?;
                require_matching_binding(existing.as_ref(), &observation.native_session_id)?;
                let changed = transaction.execute(
                    "UPDATE provider_bindings SET last_settled_turn_id = ?1, revision = revision + 1
                     WHERE runtime_id = ?2", params![turn_id, runtime_id.to_string()]
                ).map_err(StateError::Sqlite)?;
                if changed != 1 {
                    return Err(StateError::ConcurrentWrite);
                }
                update_runtime_lifecycle(&transaction, runtime_id, revision, "attention")?;
                mark_result_attention_in_transaction(
                    &transaction,
                    workstream_id,
                    observation.native_session_id,
                    turn_id,
                )?;
            }
            LifecycleEvent::SessionEnd => {
                require_matching_binding(existing.as_ref(), &observation.native_session_id)?;
                update_runtime_lifecycle(&transaction, runtime_id, revision, "stopped")?;
            }
        }
        touch_workstream(&transaction, &runtime.0, activity_at_millis)?;
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Creates a durable operation or returns the operation for the request key.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation is invalid, state cannot be read or
    /// written, or a previous request key cannot be resolved.
    pub fn create_or_get_operation(
        &mut self,
        request_key: String,
        kind: OperationKind,
        expected_revisions_json: String,
    ) -> Result<(CompoundOperation, bool), StateError> {
        self.create_or_get_operation_with_id_generator(
            request_key,
            kind,
            expected_revisions_json,
            &RandomIdGenerator,
        )
    }

    /// Creates or gets an operation with an injected identity source.
    ///
    /// This is a deterministic seam for recovery fixtures. Production callers
    /// should use [`Self::create_or_get_operation`].
    ///
    /// # Errors
    ///
    /// Returns an error if the operation is invalid, state cannot be read or
    /// written, or a previous request key cannot be resolved.
    pub fn create_or_get_operation_with_id_generator(
        &mut self,
        request_key: String,
        kind: OperationKind,
        expected_revisions_json: String,
        id_generator: &dyn IdGenerator,
    ) -> Result<(CompoundOperation, bool), StateError> {
        let candidate = CompoundOperation::with_id(
            OperationId::from(id_generator.uuid()),
            request_key,
            kind,
            expected_revisions_json,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;

        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO compound_operations (
                    operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    candidate.id.to_string(),
                    candidate.request_key,
                    operation_kind_text(candidate.kind),
                    operation_phase_text(candidate.phase),
                    candidate.expected_revisions_json,
                    candidate.effect_watermark,
                    candidate.outcome_json,
                    candidate.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;

        let operation = if inserted == 1 {
            candidate
        } else {
            load_operation_by_request_key(&transaction, &candidate.request_key)?
                .ok_or_else(|| StateError::MissingOperation(candidate.request_key.clone()))?
        };
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok((operation, inserted == 1))
    }

    /// Advances an operation with an optimistic revision guard.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale revision, an invalid transition, missing
    /// operation, or failed state transaction.
    pub fn transition_operation(
        &mut self,
        operation_id: OperationId,
        expected_revision: Revision,
        next_phase: OperationPhase,
        effect_watermark: Option<String>,
        outcome_json: Option<String>,
    ) -> Result<CompoundOperation, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let mut operation = load_operation_by_id(&transaction, operation_id)?
            .ok_or(StateError::UnknownOperation(operation_id))?;
        if operation.revision != expected_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: operation.revision,
            }));
        }
        operation.transition(next_phase, effect_watermark, outcome_json)?;
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = ?1, effect_watermark = ?2, outcome_json = ?3, revision = ?4
                 WHERE operation_id = ?5 AND revision = ?6",
                params![
                    operation_phase_text(operation.phase),
                    operation.effect_watermark,
                    operation.outcome_json,
                    operation.revision.value(),
                    operation.id.to_string(),
                    expected_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(operation)
    }

    /// Records a settled provider result and leaves prior unseen result attention sticky.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid provider identifier or failed state
    /// transaction.
    pub fn mark_result_attention(
        &mut self,
        workstream_id: WorkstreamId,
        session_id: String,
        turn_id: String,
    ) -> Result<AttentionState, StateError> {
        self.update_attention(workstream_id, |attention| {
            attention.mark_result(session_id, turn_id)
        })
    }

    /// Records a recovery-required attention condition.
    ///
    /// # Errors
    ///
    /// Returns an error when the state transaction cannot be completed.
    pub fn mark_recovery_attention(
        &mut self,
        workstream_id: WorkstreamId,
    ) -> Result<AttentionState, StateError> {
        self.update_attention(workstream_id, |attention| {
            attention.mark_recovery_required();
            Ok(())
        })
    }

    /// Clears result attention only at the caller's observed revision.
    ///
    /// # Errors
    ///
    /// Returns an error when a newer attention update exists or the state
    /// transaction cannot be completed.
    pub fn acknowledge_result_attention(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    ) -> Result<AttentionState, StateError> {
        self.update_attention(workstream_id, |attention| {
            attention.acknowledge_result(expected_revision)
        })
    }

    /// Reads the durable attention state for one workstream.
    ///
    /// # Errors
    ///
    /// Returns an error when the state cannot be queried or contains invalid
    /// persisted data.
    pub fn attention(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<Option<AttentionState>, StateError> {
        load_attention_from_connection(&self.connection, workstream_id)
    }

    fn update_attention(
        &mut self,
        workstream_id: WorkstreamId,
        update: impl FnOnce(&mut AttentionState) -> Result<(), DomainError>,
    ) -> Result<AttentionState, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let mut attention = load_attention_from_transaction(&transaction, workstream_id)?
            .unwrap_or_else(|| AttentionState::new(workstream_id));
        let prior_revision = attention.revision;
        update(&mut attention)?;
        let changed = transaction
            .execute(
                "INSERT INTO attention_states (
                    workstream_id, result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_turn_id, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(workstream_id) DO UPDATE SET
                    result_unseen_since_revision = excluded.result_unseen_since_revision,
                    recovery_unseen_since_revision = excluded.recovery_unseen_since_revision,
                    latest_native_session_id = excluded.latest_native_session_id,
                    latest_turn_id = excluded.latest_turn_id,
                    revision = excluded.revision
                 WHERE attention_states.revision = ?7",
                params![
                    attention.workstream_id.to_string(),
                    attention.result_unseen_since_revision.map(Revision::value),
                    attention
                        .recovery_unseen_since_revision
                        .map(Revision::value),
                    attention.latest_native_session_id,
                    attention.latest_turn_id,
                    attention.revision.value(),
                    prior_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(attention)
    }
}

struct ManagedWorkstreamSource {
    location_id: LocationId,
    revision: Revision,
    repository_path: PathBuf,
    repository_identity: String,
    managed_worktree_root: PathBuf,
    runtime_id: Option<RuntimeId>,
    runtime_lifecycle: Option<String>,
    native_session_id: Option<String>,
    last_settled_turn_id: Option<String>,
    native_name: Option<String>,
}

fn managed_worktree_plan_for_source(
    transaction: &rusqlite::Transaction<'_>,
    kind: OperationKind,
    source_workstream_id: WorkstreamId,
    expected_source_revision: Revision,
    base_commit: String,
) -> Result<PersistedManagedWorktreePlan, StateError> {
    let source = load_managed_workstream_source(
        transaction,
        source_workstream_id,
        kind == OperationKind::Start,
    )?;
    if source.revision != expected_source_revision {
        return Err(StateError::Domain(DomainError::RevisionConflict {
            expected: expected_source_revision,
            current: source.revision,
        }));
    }
    validate_registry_text("repository identity", &source.repository_identity)?;
    if source.managed_worktree_root.as_os_str().is_empty() {
        return Err(StateError::InvalidManagedWorktreePlanShape);
    }
    let (source_native_session_id, last_settled_turn_id) = if kind == OperationKind::Fork {
        fork_boundary(&source)?
    } else {
        (None, None)
    };
    let workstream_id = WorkstreamId::new();
    Ok(PersistedManagedWorktreePlan {
        schema_version: 1,
        workstream_id,
        checkout_id: CheckoutId::new(),
        location_id: source.location_id,
        origin: if kind == OperationKind::Fork {
            WorkstreamOrigin::Fork
        } else {
            WorkstreamOrigin::Independent
        },
        source_workstream_id,
        repository_path: source.repository_path,
        repository_identity: source.repository_identity,
        worktree_path: source.managed_worktree_root.join(workstream_id.to_string()),
        branch: format!("wsnav/workstream/{workstream_id}"),
        base_commit,
        source_runtime_id: source.runtime_id,
        source_native_session_id,
        last_settled_turn_id,
        source_native_name: (kind == OperationKind::Fork)
            .then_some(source.native_name)
            .flatten(),
        fork_attempted_at_millis: None,
    })
}

fn load_managed_workstream_source(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
    include_archived: bool,
) -> Result<ManagedWorkstreamSource, StateError> {
    let source = transaction
        .query_row(
            "SELECT workstreams.location_id, workstreams.revision,
                    project_locations.repository_path,
                    project_locations.repository_identity,
                    project_locations.managed_worktree_root,
                    workstreams.archived_at_millis,
                    runtimes.runtime_id, runtimes.lifecycle,
                    provider_bindings.native_session_id,
                    provider_bindings.last_settled_turn_id,
                    provider_bindings.observed_thread_name
             FROM workstreams
             JOIN project_locations ON project_locations.location_id = workstreams.location_id
             LEFT JOIN runtimes ON runtimes.workstream_id = workstreams.workstream_id
             LEFT JOIN provider_bindings ON provider_bindings.runtime_id = runtimes.runtime_id
             WHERE workstreams.workstream_id = ?1",
            [workstream_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
    if source.5.is_some() && !include_archived {
        return Err(StateError::WorkstreamArchived(workstream_id));
    }
    Ok(ManagedWorkstreamSource {
        location_id: Uuid::parse_str(&source.0)
            .map(LocationId::from)
            .map_err(StateError::InvalidPersistedUuid)?,
        revision: Revision::try_from(source.1)?,
        repository_path: PathBuf::from(source.2),
        repository_identity: source.3,
        managed_worktree_root: PathBuf::from(source.4),
        runtime_id: source
            .6
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(StateError::InvalidPersistedUuid)?
            .map(RuntimeId::from),
        runtime_lifecycle: source.7,
        native_session_id: source.8,
        last_settled_turn_id: source.9,
        native_name: source.10,
    })
}

fn fork_boundary(
    source: &ManagedWorkstreamSource,
) -> Result<(Option<String>, Option<String>), StateError> {
    let runtime_is_live = matches!(
        source.runtime_lifecycle.as_deref(),
        Some("idle" | "working" | "attention")
    );
    let session_id = source
        .native_session_id
        .clone()
        .ok_or(StateError::ForkBoundaryUnavailable)?;
    let settled_turn_id = source
        .last_settled_turn_id
        .clone()
        .ok_or(StateError::ForkBoundaryUnavailable)?;
    if !runtime_is_live || source.runtime_id.is_none() {
        return Err(StateError::ForkBoundaryUnavailable);
    }
    validate_provider_metadata(&session_id)?;
    validate_provider_metadata(&settled_turn_id)?;
    Ok((Some(session_id), Some(settled_turn_id)))
}

fn insert_managed_workstream_records(
    transaction: &rusqlite::Transaction<'_>,
    plan: &PersistedManagedWorktreePlan,
    destination_native_session_id: Option<&str>,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    transaction
        .execute(
            "INSERT INTO checkouts (
                checkout_id, path, ownership, branch, creation_commit,
                repository_identity, revision
             ) VALUES (?1, ?2, 'managed', ?3, ?4, ?5, 1)",
            params![
                plan.checkout_id.to_string(),
                plan.worktree_path.to_string_lossy(),
                plan.branch,
                plan.base_commit,
                plan.repository_identity,
            ],
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO workstreams (
                workstream_id, location_id, origin, source_workstream_id,
                checkout_id, lifecycle, last_activity_sequence,
                last_activity_at_millis, revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, 0, 1)",
            params![
                plan.workstream_id.to_string(),
                plan.location_id.to_string(),
                workstream_origin_text(plan.origin),
                plan.source_workstream_id.to_string(),
                plan.checkout_id.to_string(),
                activity_sequence,
            ],
        )
        .map_err(StateError::Sqlite)?;
    if let Some(destination_native_session_id) = destination_native_session_id {
        insert_pending_fork_runtime(transaction, plan, destination_native_session_id)?;
    }
    Ok(())
}

fn insert_pending_fork_runtime(
    transaction: &rusqlite::Transaction<'_>,
    plan: &PersistedManagedWorktreePlan,
    destination_native_session_id: &str,
) -> Result<(), StateError> {
    let runtime_id = RuntimeId::new();
    let runtime_generation = format!("pending-fork-{}", Uuid::new_v4());
    let tmux_session = format!("wsnav-{runtime_id}");
    transaction
        .execute(
            "INSERT INTO runtimes (
                runtime_id, workstream_id, provider, tmux_generation, tmux_session,
                cwd, process_birth, lifecycle, revision
             ) VALUES (?1, ?2, 'codex', ?3, ?4, ?5, NULL, 'stopped', 1)",
            params![
                runtime_id.to_string(),
                plan.workstream_id.to_string(),
                runtime_generation,
                tmux_session,
                plan.worktree_path.to_string_lossy(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO provider_bindings (
                binding_id, runtime_id, native_session_id, start_source,
                last_settled_turn_id, observed_thread_name, name_state,
                name_observed_at, predecessor_native_session_id,
                predecessor_effective_name, runtime_generation, revision
             ) VALUES (?1, ?2, ?3, 'resume', NULL, NULL, 'unavailable', NULL,
                NULL, NULL, ?4, 1)",
            params![
                Uuid::new_v4().to_string(),
                runtime_id.to_string(),
                destination_native_session_id,
                runtime_generation,
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(())
}

fn commit_managed_operation(
    transaction: &rusqlite::Transaction<'_>,
    operation: &mut CompoundOperation,
    plan: &PersistedManagedWorktreePlan,
    destination_native_session_id: Option<&str>,
) -> Result<(), StateError> {
    let outcome = serde_json::json!({
        "workstream_id": plan.workstream_id,
        "destination_native_session_id": destination_native_session_id,
    })
    .to_string();
    let prior_revision = operation.revision;
    operation.transition(
        OperationPhase::Committed,
        operation.effect_watermark.clone(),
        Some(outcome),
    )?;
    let updated = transaction
        .execute(
            "UPDATE compound_operations
             SET phase = ?1, effect_watermark = ?2, outcome_json = ?3, revision = ?4
             WHERE operation_id = ?5 AND revision = ?6",
            params![
                operation_phase_text(operation.phase),
                operation.effect_watermark,
                operation.outcome_json,
                operation.revision.value(),
                operation.id.to_string(),
                prior_revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if updated != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    Ok(())
}

fn managed_workstream_from_plan(plan: &PersistedManagedWorktreePlan) -> ManagedWorkstream {
    ManagedWorkstream {
        workstream_id: plan.workstream_id,
        checkout_id: plan.checkout_id,
        location_id: plan.location_id,
        origin: plan.origin,
        source_workstream_id: plan.source_workstream_id,
        revision: Revision::INITIAL,
    }
}

#[derive(Debug)]
pub struct ClientCatalog {
    connection: Connection,
}

impl ClientCatalog {
    /// Opens the client catalog, applying only known development migrations.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O, `SQLite`, permission, or unsupported-schema
    /// failures.
    pub fn open(root: &StateRoot) -> Result<Self, StateError> {
        let path = root.client_database_path();
        let mut connection = Connection::open(&path).map_err(StateError::Sqlite)?;
        set_private_file_permissions(&path)?;
        configure_connection(&connection)?;
        migrate_client_schema(&mut connection)?;
        Ok(Self { connection })
    }

    /// Returns the client schema version recorded by `SQLite`.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read the schema version.
    pub fn schema_version(&self) -> Result<i64, StateError> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(StateError::Sqlite)
    }

    /// Creates the local client-side Project grouping for a newly registered
    /// host location. The generated Project identity is never inferred from a
    /// path; the supplied label is only initial presentation text.
    ///
    /// # Errors
    ///
    /// Returns an error when the local host identity changes unexpectedly, a
    /// display label is unsafe, or the client catalog cannot commit atomically.
    pub fn register_local_project_location(
        &mut self,
        host: &HostIdentity,
        location_id: LocationId,
        executable_path: &Path,
        display_name: &str,
    ) -> Result<ClientProjectLocation, StateError> {
        self.register_local_project_location_with_identity(
            host,
            location_id,
            executable_path,
            display_name,
            None,
        )
    }

    /// Associates one local host location with a presentation Project,
    /// reusing an existing Project when the repository fingerprint matches.
    ///
    /// # Errors
    ///
    /// Returns an error when local host trust changed, metadata is unsafe, or
    /// the client catalog cannot commit atomically.
    pub fn register_local_project_location_with_identity(
        &mut self,
        host: &HostIdentity,
        location_id: LocationId,
        executable_path: &Path,
        display_name: &str,
        repository_fingerprint: Option<&str>,
    ) -> Result<ClientProjectLocation, StateError> {
        validate_project_display_name(display_name)?;
        validate_repository_fingerprint(repository_fingerprint)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        ensure_local_client_host(&transaction, host, executable_path)?;
        let project = associate_project_location(
            &transaction,
            host.host_id,
            location_id,
            display_name,
            repository_fingerprint,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(project)
    }

    /// Associates a location on an already trusted host with a presentation
    /// Project. This changes only the local client catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is unknown, metadata is unsafe, or the
    /// client catalog cannot commit atomically.
    pub fn register_host_project_location(
        &mut self,
        host_id: HostId,
        location_id: LocationId,
        display_name: &str,
        repository_fingerprint: Option<&str>,
    ) -> Result<ClientProjectLocation, StateError> {
        validate_project_display_name(display_name)?;
        validate_repository_fingerprint(repository_fingerprint)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let known: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM hosts WHERE host_id = ?1)",
                [host_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if !known {
            return Err(StateError::UnknownClientHost);
        }
        let project = associate_project_location(
            &transaction,
            host_id,
            location_id,
            display_name,
            repository_fingerprint,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(project)
    }

    /// Records one explicit SSH host registration after a successful bounded
    /// protocol handshake. A changed identity, generation, executable, or
    /// capability fingerprint is rejected until the user explicitly resets
    /// the registration.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe client record, a conflicting existing
    /// registration, or a failed atomic catalog update.
    pub fn register_ssh_host(
        &mut self,
        alias: &str,
        identity: &HostIdentity,
        executable_path: &Path,
        destination: &str,
        capabilities: Capabilities,
    ) -> Result<ClientHost, StateError> {
        validate_client_host_alias(alias)?;
        if alias == "local" {
            return Err(StateError::ClientHostRegistrationMismatch);
        }
        validate_client_host_text("remote executable", &executable_path.to_string_lossy())?;
        validate_client_host_text("SSH destination", destination)?;
        validate_client_host_text("registry generation", &identity.registry_generation)?;
        let capabilities_json = serialize_capabilities(&capabilities)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let existing = load_client_host_by_alias(&transaction, alias)?;
        let host = ClientHost {
            alias: alias.to_owned(),
            host_id: identity.host_id,
            registry_generation: identity.registry_generation.clone(),
            executable_path: executable_path.to_path_buf(),
            transport: ClientHostTransport::Ssh {
                destination: destination.to_owned(),
            },
            capabilities,
            revision: Revision::INITIAL,
        };
        if let Some(existing) = existing {
            validate_unchanged_ssh_registration(&existing, &host)?;
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(existing);
        }
        let duplicate_alias: Option<String> = transaction
            .query_row(
                "SELECT host_alias FROM hosts WHERE host_id = ?1",
                [host.host_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        if duplicate_alias.is_some() {
            return Err(StateError::ClientHostAlreadyRegistered);
        }
        transaction
            .execute(
                "INSERT INTO hosts (
                    host_alias, host_id, registry_generation, executable_path,
                    transport, ssh_destination, capabilities_json, revision
                 ) VALUES (?1, ?2, ?3, ?4, 'ssh', ?5, ?6, 1)",
                params![
                    host.alias,
                    host.host_id.to_string(),
                    host.registry_generation,
                    host.executable_path.to_string_lossy(),
                    destination,
                    capabilities_json,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(host)
    }

    /// Returns the exact client-side registration for one host alias.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog cannot be queried or contains an
    /// invalid persisted host record.
    pub fn host(&self, alias: &str) -> Result<Option<ClientHost>, StateError> {
        self.connection
            .query_row(
                "SELECT host_alias, host_id, registry_generation, executable_path,
                        transport, ssh_destination, capabilities_json, revision
                 FROM hosts WHERE host_alias = ?1",
                [alias],
                row_to_client_host,
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Returns every explicitly registered SSH host in deterministic alias
    /// order. Local host bookkeeping is deliberately excluded.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog cannot be queried or contains an
    /// invalid persisted host record.
    pub fn ssh_hosts(&self) -> Result<Vec<ClientHost>, StateError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT host_alias, host_id, registry_generation, executable_path,
                        transport, ssh_destination, capabilities_json, revision
                 FROM hosts WHERE transport = 'ssh' ORDER BY host_alias",
            )
            .map_err(StateError::Sqlite)?;
        statement
            .query_map([], row_to_client_host)
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)
    }

    /// Verifies fresh `hello` evidence against the user's fixed registration.
    /// A mismatch does not update the catalog and callers must disable remote
    /// mutation until the operator resets and re-registers the host.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is unknown or its identity, generation,
    /// or capabilities differ from the recorded registration.
    pub fn verify_hello(
        &self,
        alias: &str,
        hello: &HelloResponse,
    ) -> Result<ClientHost, StateError> {
        let host = self.host(alias)?.ok_or(StateError::UnknownClientHost)?;
        host.verify_hello(hello)?;
        Ok(host)
    }

    /// Removes one explicit SSH host registration and its client-side project
    /// associations. It never contacts the host or mutates the host registry.
    ///
    /// # Errors
    ///
    /// Returns an error for the protected local record, an unknown alias, or a
    /// failed atomic catalog update.
    pub fn reset_ssh_host(&mut self, alias: &str) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let host =
            load_client_host_by_alias(&transaction, alias)?.ok_or(StateError::UnknownClientHost)?;
        if !matches!(host.transport, ClientHostTransport::Ssh { .. }) {
            return Err(StateError::ClientHostResetRefused);
        }
        transaction
            .execute(
                "DELETE FROM project_locations WHERE host_id = ?1",
                [host.host_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "DELETE FROM ignored_project_locations WHERE host_id = ?1",
                [host.host_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        let deleted = transaction
            .execute("DELETE FROM hosts WHERE host_alias = ?1", [alias])
            .map_err(StateError::Sqlite)?;
        if deleted != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Looks up the client-local Project label for one exact local host
    /// location. Missing data is a normal fallback condition during D2.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog cannot be queried or contains an
    /// invalid persisted identity.
    pub fn local_project_location(
        &self,
        host_id: HostId,
        location_id: LocationId,
    ) -> Result<Option<ClientProjectLocation>, StateError> {
        self.connection
            .query_row(
                "SELECT projects.project_id, projects.display_name,
                        projects.repository_fingerprint
                 FROM project_locations
                 JOIN projects ON projects.project_id = project_locations.project_id
                 WHERE project_locations.host_id = ?1 AND project_locations.location_id = ?2",
                params![host_id.to_string(), location_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)
            .and_then(|row| {
                row.map_or(Ok(None), |(project_id, display_name, fingerprint)| {
                    Uuid::parse_str(&project_id)
                        .map(ProjectId::from)
                        .map(|project_id| {
                            Some(ClientProjectLocation {
                                project_id,
                                display_name,
                                repository_fingerprint: fingerprint,
                            })
                        })
                        .map_err(StateError::InvalidPersistedUuid)
                })
            })
    }

    /// Returns whether this host-owned location was explicitly forgotten from
    /// the client navigator without mutating its host registry.
    ///
    /// # Errors
    ///
    /// Returns an error when the client catalog cannot be queried.
    pub fn project_location_is_ignored(
        &self,
        host_id: HostId,
        location_id: LocationId,
    ) -> Result<bool, StateError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM ignored_project_locations
                    WHERE host_id = ?1 AND location_id = ?2
                 )",
                params![host_id.to_string(), location_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)
    }

    /// Hides every client-visible location in one Project. Host registries,
    /// checkouts, runtimes, and provider sessions remain untouched.
    ///
    /// # Errors
    ///
    /// Returns an error when the client catalog cannot commit the exact
    /// client-only visibility change atomically.
    pub fn ignore_project_locations(&mut self, project_id: ProjectId) -> Result<usize, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let hidden = transaction
            .execute(
                "INSERT OR IGNORE INTO ignored_project_locations (host_id, location_id)
                 SELECT host_id, location_id FROM project_locations WHERE project_id = ?1",
                [project_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(hidden)
    }
}

fn associate_project_location(
    transaction: &rusqlite::Transaction<'_>,
    host_id: HostId,
    location_id: LocationId,
    display_name: &str,
    repository_fingerprint: Option<&str>,
) -> Result<ClientProjectLocation, StateError> {
    let existing: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT projects.project_id, projects.display_name,
                    projects.repository_fingerprint
             FROM project_locations
             JOIN projects ON projects.project_id = project_locations.project_id
             WHERE project_locations.host_id = ?1 AND project_locations.location_id = ?2",
            params![host_id.to_string(), location_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let matching = repository_fingerprint
        .map(|fingerprint| {
            transaction
                .query_row(
                    "SELECT project_id, display_name FROM projects
                     WHERE repository_fingerprint = ?1",
                    [fingerprint],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(StateError::Sqlite)
        })
        .transpose()?
        .flatten();

    if let Some(existing) = existing {
        return reassociate_existing_project(
            transaction,
            host_id,
            location_id,
            display_name,
            repository_fingerprint,
            existing,
            matching,
        );
    }

    let project = if let Some((project_id, display_name)) = matching {
        project_location(
            &project_id,
            display_name,
            repository_fingerprint.map(str::to_owned),
        )?
    } else {
        create_project(transaction, display_name, repository_fingerprint)?
    };
    transaction
        .execute(
            "INSERT INTO project_locations (project_id, host_id, location_id)
             VALUES (?1, ?2, ?3)",
            params![
                project.project_id.to_string(),
                host_id.to_string(),
                location_id.to_string(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(project)
}

#[allow(clippy::too_many_arguments)]
fn reassociate_existing_project(
    transaction: &rusqlite::Transaction<'_>,
    host_id: HostId,
    location_id: LocationId,
    display_name: &str,
    repository_fingerprint: Option<&str>,
    existing: (String, String, Option<String>),
    matching: Option<(String, String)>,
) -> Result<ClientProjectLocation, StateError> {
    let (existing_id, existing_name, existing_fingerprint) = existing;
    if let Some((matching_id, matching_name)) = matching {
        if matching_id != existing_id {
            transaction
                .execute(
                    "UPDATE project_locations SET project_id = ?1
                     WHERE host_id = ?2 AND location_id = ?3 AND project_id = ?4",
                    params![
                        matching_id,
                        host_id.to_string(),
                        location_id.to_string(),
                        existing_id,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            delete_orphan_project(transaction, &existing_id)?;
        }
        return project_location(
            &matching_id,
            matching_name,
            repository_fingerprint.map(str::to_owned),
        );
    }
    if repository_fingerprint.is_none() {
        if existing_fingerprint.is_none() && existing_name != display_name {
            let location_count = project_location_count(transaction, &existing_id)?;
            if location_count == 1 {
                transaction
                    .execute(
                        "UPDATE projects SET display_name = ?1,
                         revision = revision + 1 WHERE project_id = ?2",
                        params![display_name, existing_id],
                    )
                    .map_err(StateError::Sqlite)?;
                return project_location(&existing_id, display_name.to_owned(), None);
            }
        }
        return project_location(&existing_id, existing_name, existing_fingerprint);
    }
    if existing_fingerprint.as_deref() == repository_fingerprint {
        return project_location(&existing_id, existing_name, existing_fingerprint);
    }

    if project_location_count(transaction, &existing_id)? == 1 {
        transaction
            .execute(
                "UPDATE projects SET repository_fingerprint = ?1,
                     display_name = ?2, revision = revision + 1
                 WHERE project_id = ?3",
                params![repository_fingerprint, display_name, existing_id],
            )
            .map_err(StateError::Sqlite)?;
        return project_location(
            &existing_id,
            display_name.to_owned(),
            repository_fingerprint.map(str::to_owned),
        );
    }

    let project = create_project(transaction, display_name, repository_fingerprint)?;
    transaction
        .execute(
            "UPDATE project_locations SET project_id = ?1
             WHERE host_id = ?2 AND location_id = ?3 AND project_id = ?4",
            params![
                project.project_id.to_string(),
                host_id.to_string(),
                location_id.to_string(),
                existing_id,
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(project)
}

fn project_location_count(
    transaction: &rusqlite::Transaction<'_>,
    project_id: &str,
) -> Result<i64, StateError> {
    transaction
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

fn create_project(
    transaction: &rusqlite::Transaction<'_>,
    display_name: &str,
    repository_fingerprint: Option<&str>,
) -> Result<ClientProjectLocation, StateError> {
    let project_id = ProjectId::new();
    transaction
        .execute(
            "INSERT INTO projects (
                project_id, display_name, repository_fingerprint, revision
             ) VALUES (?1, ?2, ?3, 1)",
            params![project_id.to_string(), display_name, repository_fingerprint],
        )
        .map_err(StateError::Sqlite)?;
    Ok(ClientProjectLocation {
        project_id,
        display_name: display_name.to_owned(),
        repository_fingerprint: repository_fingerprint.map(str::to_owned),
    })
}

fn project_location(
    project_id: &str,
    display_name: String,
    repository_fingerprint: Option<String>,
) -> Result<ClientProjectLocation, StateError> {
    Ok(ClientProjectLocation {
        project_id: Uuid::parse_str(project_id)
            .map(ProjectId::from)
            .map_err(StateError::InvalidPersistedUuid)?,
        display_name,
        repository_fingerprint,
    })
}

fn delete_orphan_project(
    transaction: &rusqlite::Transaction<'_>,
    project_id: &str,
) -> Result<(), StateError> {
    transaction
        .execute(
            "DELETE FROM projects WHERE project_id = ?1
             AND NOT EXISTS (
                SELECT 1 FROM project_locations WHERE project_id = ?1
             )",
            [project_id],
        )
        .map_err(StateError::Sqlite)?;
    Ok(())
}

fn configure_connection(connection: &Connection) -> Result<(), StateError> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
        )
        .map_err(StateError::Sqlite)
}

fn initialize_host_identity(
    connection: &Connection,
    id_generator: &dyn IdGenerator,
) -> Result<(), StateError> {
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO host_identity (
                singleton, host_id, registry_generation, schema_version
             ) VALUES (1, ?1, ?2, ?3)",
            params![
                HostId::from(id_generator.uuid()).to_string(),
                id_generator.uuid().to_string(),
                HOST_SCHEMA_VERSION,
            ],
        )
        .map_err(StateError::Sqlite)?;
    if inserted != 1 && inserted != 0 {
        return Err(StateError::ConcurrentWrite);
    }
    Ok(())
}

fn migrate_host_schema(connection: &mut Connection, state_root: &Path) -> Result<(), StateError> {
    let current: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StateError::Sqlite)?;
    if current > HOST_SCHEMA_VERSION {
        return Err(StateError::UnsupportedSchemaVersion(current));
    }
    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    match current {
        0 => transaction
            .execute_batch(HOST_SCHEMA_SQL)
            .map_err(StateError::Sqlite)?,
        1 => transaction
            .execute_batch(
                "ALTER TABLE workstreams
                 ADD COLUMN last_activity_sequence INTEGER NOT NULL DEFAULT 0
                 CHECK (last_activity_sequence >= 0);
                 ALTER TABLE workstreams
                 ADD COLUMN last_activity_at_millis INTEGER NOT NULL DEFAULT 0
                 CHECK (last_activity_at_millis >= 0);",
            )
            .map_err(StateError::Sqlite)?,
        2 => transaction
            .execute_batch(
                "ALTER TABLE workstreams
                 ADD COLUMN last_activity_at_millis INTEGER NOT NULL DEFAULT 0
                 CHECK (last_activity_at_millis >= 0);",
            )
            .map_err(StateError::Sqlite)?,
        3 => {
            let mut statement = transaction
                .prepare(
                    "SELECT location_id FROM project_locations
                     WHERE managed_worktree_root = ''",
                )
                .map_err(StateError::Sqlite)?;
            let location_ids = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(StateError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StateError::Sqlite)?;
            drop(statement);
            for location_id in location_ids {
                let root = state_root.join("worktrees").join(&location_id);
                transaction
                    .execute(
                        "UPDATE project_locations SET managed_worktree_root = ?1
                         WHERE location_id = ?2 AND managed_worktree_root = ''",
                        params![root.to_string_lossy(), location_id],
                    )
                    .map_err(StateError::Sqlite)?;
            }
        }
        4 | 5 => {}
        HOST_SCHEMA_VERSION => return Ok(()),
        _ => return Err(StateError::UnsupportedSchemaVersion(current)),
    }
    ensure_host_repository_metadata_columns(&transaction)?;
    ensure_workstream_archive_column(&transaction)?;
    transaction
        .execute(&format!("PRAGMA user_version = {HOST_SCHEMA_VERSION}"), [])
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE host_identity SET schema_version = ?1 WHERE singleton = 1",
            [HOST_SCHEMA_VERSION],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

fn ensure_workstream_archive_column(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StateError> {
    let table_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'workstreams'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !table_exists {
        return Ok(());
    }
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('workstreams')
                WHERE name = 'archived_at_millis'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !exists {
        transaction
            .execute_batch("ALTER TABLE workstreams ADD COLUMN archived_at_millis INTEGER;")
            .map_err(StateError::Sqlite)?;
    }
    Ok(())
}

fn ensure_host_repository_metadata_columns(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StateError> {
    let table_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'project_locations'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !table_exists {
        return Ok(());
    }
    let display_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('project_locations')
                WHERE name = 'repository_display_name'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !display_exists {
        transaction
            .execute_batch(
                "ALTER TABLE project_locations
                 ADD COLUMN repository_display_name TEXT NOT NULL DEFAULT '';",
            )
            .map_err(StateError::Sqlite)?;
    }
    let fingerprint_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('project_locations')
                WHERE name = 'remote_identity_fingerprint'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !fingerprint_exists {
        transaction
            .execute_batch(
                "ALTER TABLE project_locations
                 ADD COLUMN remote_identity_fingerprint TEXT;",
            )
            .map_err(StateError::Sqlite)?;
    }
    Ok(())
}

fn migrate_client_schema(connection: &mut Connection) -> Result<(), StateError> {
    let current: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StateError::Sqlite)?;
    if current > CLIENT_SCHEMA_VERSION {
        return Err(StateError::UnsupportedSchemaVersion(current));
    }
    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    match current {
        0 => transaction
            .execute_batch(CLIENT_SCHEMA_SQL)
            .map_err(StateError::Sqlite)?,
        1 => transaction
            .execute_batch(
                "ALTER TABLE hosts ADD COLUMN registry_generation TEXT NOT NULL DEFAULT '';
                 ALTER TABLE hosts ADD COLUMN transport TEXT NOT NULL DEFAULT 'local';
                 ALTER TABLE hosts ADD COLUMN ssh_destination TEXT;
                 ALTER TABLE hosts ADD COLUMN capabilities_json TEXT NOT NULL
                    DEFAULT '{\"codex\":false,\"git\":false,\"tmux\":false}';
                 CREATE TABLE projects (
                    project_id TEXT PRIMARY KEY,
                    display_name TEXT NOT NULL,
                    repository_fingerprint TEXT,
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 CREATE TABLE project_locations (
                    project_id TEXT NOT NULL REFERENCES projects(project_id),
                    host_id TEXT NOT NULL,
                    location_id TEXT NOT NULL,
                    PRIMARY KEY(project_id, host_id, location_id)
                 );
                 CREATE TABLE preferences (
                    key TEXT PRIMARY KEY,
                    value_json TEXT NOT NULL
                 );
                 CREATE UNIQUE INDEX project_location_identity_idx
                    ON project_locations(host_id, location_id);
                 CREATE UNIQUE INDEX project_repository_fingerprint_idx
                    ON projects(repository_fingerprint)
                    WHERE repository_fingerprint IS NOT NULL;",
            )
            .map_err(StateError::Sqlite)?,
        2 => transaction
            .execute_batch(
                "ALTER TABLE projects ADD COLUMN repository_fingerprint TEXT;
                 CREATE UNIQUE INDEX project_location_identity_idx
                    ON project_locations(host_id, location_id);
                 CREATE UNIQUE INDEX project_repository_fingerprint_idx
                    ON projects(repository_fingerprint)
                    WHERE repository_fingerprint IS NOT NULL;",
            )
            .map_err(StateError::Sqlite)?,
        3 => transaction
            .execute_batch(
                "CREATE TABLE ignored_project_locations (
                    host_id TEXT NOT NULL,
                    location_id TEXT NOT NULL,
                    PRIMARY KEY(host_id, location_id)
                 );",
            )
            .map_err(StateError::Sqlite)?,
        CLIENT_SCHEMA_VERSION => return Ok(()),
        _ => return Err(StateError::UnsupportedSchemaVersion(current)),
    }
    transaction
        .execute(
            &format!("PRAGMA user_version = {CLIENT_SCHEMA_VERSION}"),
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

fn ensure_local_client_host(
    transaction: &rusqlite::Transaction<'_>,
    identity: &HostIdentity,
    executable_path: &Path,
) -> Result<(), StateError> {
    let existing = load_client_host_by_alias(transaction, "local")?;
    let Some(existing) = existing else {
        transaction
            .execute(
                "INSERT INTO hosts (
                    host_alias, host_id, registry_generation, executable_path,
                    transport, ssh_destination, capabilities_json, revision
                 ) VALUES ('local', ?1, ?2, ?3, 'local', NULL, ?4, 1)",
                params![
                    identity.host_id.to_string(),
                    identity.registry_generation,
                    executable_path.to_string_lossy(),
                    serialize_capabilities(&Capabilities::default())?,
                ],
            )
            .map_err(StateError::Sqlite)?;
        return Ok(());
    };
    if existing.host_id != identity.host_id {
        return Err(StateError::ClientHostIdentityMismatch);
    }
    if !matches!(existing.transport, ClientHostTransport::Local) {
        return Err(StateError::ClientHostRegistrationMismatch);
    }
    if !existing.registry_generation.is_empty()
        && existing.registry_generation != identity.registry_generation
    {
        return Err(StateError::ClientHostGenerationMismatch);
    }
    if existing.registry_generation == identity.registry_generation
        && existing.executable_path == executable_path
    {
        return Ok(());
    }
    let changed = transaction
        .execute(
            "UPDATE hosts SET registry_generation = ?1, executable_path = ?2,
                 revision = revision + 1
             WHERE host_alias = 'local' AND host_id = ?3 AND revision = ?4",
            params![
                identity.registry_generation,
                executable_path.to_string_lossy(),
                identity.host_id.to_string(),
                existing.revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    Ok(())
}

fn load_client_host_by_alias(
    connection: &rusqlite::Transaction<'_>,
    alias: &str,
) -> Result<Option<ClientHost>, StateError> {
    connection
        .query_row(
            "SELECT host_alias, host_id, registry_generation, executable_path,
                    transport, ssh_destination, capabilities_json, revision
             FROM hosts WHERE host_alias = ?1",
            [alias],
            row_to_client_host,
        )
        .optional()
        .map_err(StateError::Sqlite)
}

fn row_to_client_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClientHost> {
    let alias: String = row.get(0)?;
    let host_id: String = row.get(1)?;
    let registry_generation: String = row.get(2)?;
    let executable_path: String = row.get(3)?;
    let transport: String = row.get(4)?;
    let destination: Option<String> = row.get(5)?;
    let capabilities_json: String = row.get(6)?;
    let revision: i64 = row.get(7)?;
    let host_id = Uuid::parse_str(&host_id)
        .map(HostId::from)
        .map_err(to_from_sql_error)?;
    let capabilities = serde_json::from_str(&capabilities_json)
        .map_err(|_| to_from_sql_error(StateError::InvalidPersistedCapabilities))?;
    let transport = match transport.as_str() {
        "local" => ClientHostTransport::Local,
        "ssh" => ClientHostTransport::Ssh {
            destination: destination.ok_or_else(|| {
                to_from_sql_error(StateError::InvalidPersistedValue(
                    "missing SSH destination".to_owned(),
                ))
            })?,
        },
        _ => {
            return Err(to_from_sql_error(StateError::InvalidPersistedValue(
                transport,
            )));
        }
    };
    Ok(ClientHost {
        alias,
        host_id,
        registry_generation,
        executable_path: PathBuf::from(executable_path),
        transport,
        capabilities,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn validate_unchanged_ssh_registration(
    existing: &ClientHost,
    candidate: &ClientHost,
) -> Result<(), StateError> {
    if existing.host_id != candidate.host_id {
        return Err(StateError::ClientHostIdentityMismatch);
    }
    if existing.registry_generation != candidate.registry_generation {
        return Err(StateError::ClientHostGenerationMismatch);
    }
    if existing.capabilities != candidate.capabilities {
        return Err(StateError::ClientHostCapabilitiesMismatch);
    }
    if existing.executable_path != candidate.executable_path
        || existing.transport != candidate.transport
    {
        return Err(StateError::ClientHostRegistrationMismatch);
    }
    Ok(())
}

fn serialize_capabilities(capabilities: &Capabilities) -> Result<String, StateError> {
    serde_json::to_string(capabilities).map_err(StateError::ClientCapabilitiesEncoding)
}

fn load_operation_by_request_key(
    transaction: &rusqlite::Transaction<'_>,
    request_key: &str,
) -> Result<Option<CompoundOperation>, StateError> {
    let operation = transaction
        .query_row(
            "SELECT operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
             FROM compound_operations WHERE request_key = ?1",
            [request_key],
            row_to_operation,
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(operation)
}

fn load_operation_by_id(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
) -> Result<Option<CompoundOperation>, StateError> {
    let operation = transaction
        .query_row(
            "SELECT operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
             FROM compound_operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            row_to_operation,
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(operation)
}

#[derive(Deserialize)]
struct ManagedWorkstreamOutcome {
    workstream_id: WorkstreamId,
    destination_native_session_id: Option<String>,
}

fn managed_workstream_from_outcome(
    transaction: &rusqlite::Transaction<'_>,
    operation: &CompoundOperation,
    plan: &PersistedManagedWorktreePlan,
    expected_destination_native_session_id: Option<&str>,
) -> Result<ManagedWorkstream, StateError> {
    let outcome = operation
        .outcome_json
        .as_deref()
        .ok_or(StateError::MissingManagedWorktreeOutcome)?;
    let outcome: ManagedWorkstreamOutcome =
        serde_json::from_str(outcome).map_err(StateError::InvalidManagedWorktreeOutcome)?;
    if outcome.workstream_id != plan.workstream_id
        || outcome.destination_native_session_id.as_deref()
            != expected_destination_native_session_id
    {
        return Err(StateError::ManagedWorktreePlanMismatch);
    }
    let record = transaction
        .query_row(
            "SELECT checkout_id, location_id, origin, source_workstream_id, revision
             FROM workstreams WHERE workstream_id = ?1",
            [plan.workstream_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::ManagedWorktreeCommitMissing)?;
    let checkout_id = Uuid::parse_str(&record.0)
        .map(CheckoutId::from)
        .map_err(StateError::InvalidPersistedUuid)?;
    let location_id = Uuid::parse_str(&record.1)
        .map(LocationId::from)
        .map_err(StateError::InvalidPersistedUuid)?;
    let source_workstream_id = record
        .3
        .as_deref()
        .ok_or(StateError::ManagedWorktreePlanMismatch)
        .and_then(|value| {
            Uuid::parse_str(value)
                .map(WorkstreamId::from)
                .map_err(StateError::InvalidPersistedUuid)
        })?;
    if checkout_id != plan.checkout_id
        || location_id != plan.location_id
        || source_workstream_id != plan.source_workstream_id
        || record.2 != workstream_origin_text(plan.origin)
    {
        return Err(StateError::ManagedWorktreePlanMismatch);
    }
    Ok(ManagedWorkstream {
        workstream_id: plan.workstream_id,
        checkout_id,
        location_id,
        origin: plan.origin,
        source_workstream_id,
        revision: Revision::try_from(record.4)?,
    })
}

fn row_to_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<CompoundOperation> {
    let id: String = row.get(0)?;
    let kind: String = row.get(2)?;
    let phase: String = row.get(3)?;
    let revision: i64 = row.get(7)?;
    Ok(CompoundOperation {
        id: Uuid::parse_str(&id)
            .map(OperationId::from)
            .map_err(to_from_sql_error)?,
        request_key: row.get(1)?,
        kind: operation_kind_from_text(&kind).map_err(to_from_sql_error)?,
        phase: operation_phase_from_text(&phase).map_err(to_from_sql_error)?,
        expected_revisions_json: row.get(4)?,
        effect_watermark: row.get(5)?,
        outcome_json: row.get(6)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn row_to_runtime(
    row: &rusqlite::Row<'_>,
    workstream_id: WorkstreamId,
) -> rusqlite::Result<RuntimeRecord> {
    let runtime_id: String = row.get(0)?;
    let generation: String = row.get(1)?;
    let session: String = row.get(2)?;
    let cwd: String = row.get(3)?;
    let process_birth: Option<String> = row.get(4)?;
    let lifecycle: String = row.get(5)?;
    let revision: i64 = row.get(6)?;
    Ok(RuntimeRecord {
        runtime_id: Uuid::parse_str(&runtime_id)
            .map(RuntimeId::from)
            .map_err(to_from_sql_error)?,
        workstream_id,
        tmux_generation: generation,
        tmux_session: session,
        cwd: PathBuf::from(cwd),
        process_birth,
        status: runtime_status_from_text(&lifecycle).map_err(to_from_sql_error)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn row_to_runtime_with_id(
    row: &rusqlite::Row<'_>,
    runtime_id: RuntimeId,
    workstream_id: WorkstreamId,
) -> rusqlite::Result<RuntimeRecord> {
    let generation: String = row.get(1)?;
    let session: String = row.get(2)?;
    let cwd: String = row.get(3)?;
    let process_birth: Option<String> = row.get(4)?;
    let lifecycle: String = row.get(5)?;
    let revision: i64 = row.get(6)?;
    Ok(RuntimeRecord {
        runtime_id,
        workstream_id,
        tmux_generation: generation,
        tmux_session: session,
        cwd: PathBuf::from(cwd),
        process_birth,
        status: runtime_status_from_text(&lifecycle).map_err(to_from_sql_error)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn row_to_integration(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodexIntegration> {
    let profile_schema_version = u8::try_from(row.get::<_, i64>(2)?).map_err(to_from_sql_error)?;
    let lifecycle: String = row.get(5)?;
    let revision: i64 = row.get(6)?;
    Ok(CodexIntegration {
        ownership: ProfileOwnership {
            canonical_path: PathBuf::from(row.get::<_, String>(0)?),
            owner_id: row.get(1)?,
            profile_schema_version,
            hook_executable: PathBuf::from(row.get::<_, String>(3)?),
            content_hash: row.get(4)?,
        },
        lifecycle: integration_lifecycle_from_text(&lifecycle).map_err(to_from_sql_error)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn load_binding(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<Option<ProviderBinding>, StateError> {
    transaction
        .query_row(
            "SELECT native_session_id, start_source, last_settled_turn_id,
                    observed_thread_name, name_state, predecessor_native_session_id,
                    predecessor_effective_name, revision
             FROM provider_bindings WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| {
                Ok(ProviderBinding {
                    runtime_id,
                    native_session_id: row.get(0)?,
                    start_source: row.get(1)?,
                    last_settled_turn_id: row.get(2)?,
                    observed_thread_name: row.get(3)?,
                    name_state: name_state_from_text(&row.get::<_, String>(4)?)
                        .map_err(to_from_sql_error)?,
                    predecessor_native_session_id: row.get(5)?,
                    predecessor_effective_name: row.get(6)?,
                    revision: Revision::try_from(row.get::<_, i64>(7)?)
                        .map_err(to_from_sql_error)?,
                })
            },
        )
        .optional()
        .map_err(StateError::Sqlite)
}

struct SessionStartContext<'a> {
    runtime_id: RuntimeId,
    runtime_status: &'a str,
    runtime_revision: Revision,
    generation: &'a str,
    workstream_id: WorkstreamId,
    workstream_lifecycle: WorkstreamLifecycle,
}

fn apply_session_start(
    transaction: &rusqlite::Transaction<'_>,
    context: &SessionStartContext<'_>,
    existing: Option<ProviderBinding>,
    session_id: &str,
    source: Option<&str>,
) -> Result<(), StateError> {
    let Some(binding) = existing else {
        return insert_initial_binding(transaction, context, session_id, source);
    };
    if binding.native_session_id == session_id {
        // A persisted binding appears at `starting` only when an exact parked
        // session is resumed in a fresh private tmux generation. Repeated live
        // SessionStart evidence must not mark a working turn idle.
        if context.runtime_status != "starting" {
            return Err(StateError::HookEvidenceMismatch);
        }
        if context.workstream_lifecycle == WorkstreamLifecycle::RecoveryRequired
            && source != Some("resume")
        {
            return Err(StateError::HookEvidenceMismatch);
        }
        return complete_session_start(transaction, context);
    }
    if source != Some("clear") || !matches!(context.runtime_status, "idle" | "attention") {
        return Err(StateError::HookEvidenceMismatch);
    }
    let changed = transaction
        .execute(
            "UPDATE provider_bindings SET
                native_session_id = ?1,
                start_source = 'clear',
                last_settled_turn_id = NULL,
                observed_thread_name = NULL,
                name_state = 'unavailable',
                name_observed_at = NULL,
                predecessor_native_session_id = ?2,
                predecessor_effective_name = ?3,
                revision = revision + 1
             WHERE runtime_id = ?4 AND native_session_id = ?2 AND revision = ?5",
            params![
                session_id,
                binding.native_session_id,
                binding.observed_thread_name,
                context.runtime_id.to_string(),
                binding.revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    update_runtime_lifecycle(
        transaction,
        context.runtime_id,
        context.runtime_revision,
        "idle",
    )
}

fn insert_initial_binding(
    transaction: &rusqlite::Transaction<'_>,
    context: &SessionStartContext<'_>,
    session_id: &str,
    source: Option<&str>,
) -> Result<(), StateError> {
    if context.runtime_status != "starting" || !matches!(source, Some("startup" | "resume")) {
        return Err(StateError::HookEvidenceMismatch);
    }
    if context.workstream_lifecycle == WorkstreamLifecycle::RecoveryRequired
        && source != Some("resume")
    {
        return Err(StateError::HookEvidenceMismatch);
    }
    transaction
        .execute(
            "INSERT INTO provider_bindings (
                binding_id, runtime_id, native_session_id, start_source,
                last_settled_turn_id, observed_thread_name, name_state,
                name_observed_at, predecessor_native_session_id,
                predecessor_effective_name, runtime_generation, revision
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, 'unavailable', NULL,
                NULL, NULL, ?5, 1)",
            params![
                Uuid::new_v4().to_string(),
                context.runtime_id.to_string(),
                session_id,
                source.unwrap_or("startup"),
                context.generation,
            ],
        )
        .map_err(StateError::Sqlite)?;
    complete_session_start(transaction, context)
}

fn complete_session_start(
    transaction: &rusqlite::Transaction<'_>,
    context: &SessionStartContext<'_>,
) -> Result<(), StateError> {
    update_runtime_lifecycle(
        transaction,
        context.runtime_id,
        context.runtime_revision,
        "idle",
    )?;
    if context.workstream_lifecycle == WorkstreamLifecycle::RecoveryRequired {
        reopen_recovery_workstream(transaction, context.workstream_id)?;
        clear_recovery_attention_in_transaction(transaction, context.workstream_id)?;
    }
    Ok(())
}

fn require_matching_binding(
    binding: Option<&ProviderBinding>,
    session_id: &str,
) -> Result<(), StateError> {
    if binding.is_some_and(|binding| binding.native_session_id == session_id) {
        Ok(())
    } else {
        Err(StateError::HookEvidenceMismatch)
    }
}

fn update_runtime_lifecycle(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    expected_revision: Revision,
    lifecycle: &'static str,
) -> Result<(), StateError> {
    let updated = transaction
        .execute(
            "UPDATE runtimes SET lifecycle = ?1, revision = revision + 1
             WHERE runtime_id = ?2 AND revision = ?3",
            params![lifecycle, runtime_id.to_string(), expected_revision.value()],
        )
        .map_err(StateError::Sqlite)?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

/// Returns the next durable activity order for a Workstream update.
///
/// This sequence is intentionally logical rather than wall-clock time. It
/// makes newest-first navigation deterministic even when host clocks differ.
fn next_activity_sequence(transaction: &rusqlite::Transaction<'_>) -> Result<i64, StateError> {
    transaction
        .query_row(
            "SELECT COALESCE(MAX(last_activity_sequence), 0) + 1 FROM workstreams",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

/// Records meaningful runtime or provider lifecycle activity for ordering.
fn touch_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: &str,
    activity_at_millis: Option<i64>,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET last_activity_sequence = ?1,
             last_activity_at_millis = COALESCE(?2, last_activity_at_millis),
             revision = revision + 1
             WHERE workstream_id = ?3",
            params![activity_sequence, activity_at_millis, workstream_id],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

fn reopen_parked_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET lifecycle = 'open', last_activity_sequence = ?1,
             revision = revision + 1
             WHERE workstream_id = ?2 AND lifecycle = 'parked'",
            params![activity_sequence, workstream_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

fn reopen_recovery_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET lifecycle = 'open', last_activity_sequence = ?1,
             revision = revision + 1
             WHERE workstream_id = ?2 AND lifecycle = 'recovery_required'",
            params![activity_sequence, workstream_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

fn ensure_recovery_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let mut attention = load_attention_from_transaction(transaction, workstream_id)?
        .unwrap_or_else(|| AttentionState::new(workstream_id));
    if attention.recovery_unseen_since_revision.is_some() {
        return Ok(());
    }
    let prior_revision = attention.revision;
    attention.mark_recovery_required();
    save_attention_in_transaction(transaction, &attention, prior_revision)
}

fn clear_recovery_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let mut attention = load_attention_from_transaction(transaction, workstream_id)?
        .ok_or(StateError::HookEvidenceMismatch)?;
    if attention.recovery_unseen_since_revision.is_none() {
        return Err(StateError::HookEvidenceMismatch);
    }
    let prior_revision = attention.revision;
    attention.clear_recovery_required();
    save_attention_in_transaction(transaction, &attention, prior_revision)
}

fn save_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    attention: &AttentionState,
    prior_revision: Revision,
) -> Result<(), StateError> {
    let changed = transaction
        .execute(
            "INSERT INTO attention_states (
            workstream_id, result_unseen_since_revision,
            recovery_unseen_since_revision, latest_native_session_id,
            latest_turn_id, revision
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(workstream_id) DO UPDATE SET
            result_unseen_since_revision = excluded.result_unseen_since_revision,
            recovery_unseen_since_revision = excluded.recovery_unseen_since_revision,
            latest_native_session_id = excluded.latest_native_session_id,
            latest_turn_id = excluded.latest_turn_id,
            revision = excluded.revision
         WHERE attention_states.revision = ?7",
            params![
                attention.workstream_id.to_string(),
                attention.result_unseen_since_revision.map(Revision::value),
                attention
                    .recovery_unseen_since_revision
                    .map(Revision::value),
                attention.latest_native_session_id,
                attention.latest_turn_id,
                attention.revision.value(),
                prior_revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

fn mark_result_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
    session_id: String,
    turn_id: String,
) -> Result<(), StateError> {
    let current = load_attention_from_transaction(transaction, workstream_id)?;
    let mut attention = current.unwrap_or_else(|| AttentionState::new(workstream_id));
    let prior_revision = attention.revision;
    attention.mark_result(session_id, turn_id)?;
    save_attention_in_transaction(transaction, &attention, prior_revision)
}

fn load_attention_from_connection(
    connection: &Connection,
    workstream_id: WorkstreamId,
) -> Result<Option<AttentionState>, StateError> {
    let attention = connection
        .query_row(
            "SELECT result_unseen_since_revision, recovery_unseen_since_revision,
                    latest_native_session_id, latest_turn_id, revision
             FROM attention_states WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| row_to_attention(row, workstream_id),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(attention)
}

fn load_attention_from_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<Option<AttentionState>, StateError> {
    let attention = transaction
        .query_row(
            "SELECT result_unseen_since_revision, recovery_unseen_since_revision,
                    latest_native_session_id, latest_turn_id, revision
             FROM attention_states WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| row_to_attention(row, workstream_id),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(attention)
}

fn row_to_attention(
    row: &rusqlite::Row<'_>,
    workstream_id: WorkstreamId,
) -> rusqlite::Result<AttentionState> {
    let result: Option<i64> = row.get(0)?;
    let recovery: Option<i64> = row.get(1)?;
    let revision: i64 = row.get(4)?;
    Ok(AttentionState {
        workstream_id,
        result_unseen_since_revision: result
            .map(Revision::try_from)
            .transpose()
            .map_err(to_from_sql_error)?,
        recovery_unseen_since_revision: recovery
            .map(Revision::try_from)
            .transpose()
            .map_err(to_from_sql_error)?,
        latest_native_session_id: row.get(2)?,
        latest_turn_id: row.get(3)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn to_from_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

const fn operation_kind_text(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Start => "start",
        OperationKind::Fork => "fork",
    }
}

fn operation_kind_from_text(value: &str) -> Result<OperationKind, StateError> {
    match value {
        "start" => Ok(OperationKind::Start),
        "fork" => Ok(OperationKind::Fork),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

const fn workstream_origin_text(origin: WorkstreamOrigin) -> &'static str {
    match origin {
        WorkstreamOrigin::External => "external",
        WorkstreamOrigin::Independent => "independent",
        WorkstreamOrigin::Fork => "fork",
    }
}

const fn operation_phase_text(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::Prepared => "prepared",
        OperationPhase::ExternalEffectStarted => "external_effect_started",
        OperationPhase::AwaitingReconciliation => "awaiting_reconciliation",
        OperationPhase::Committed => "committed",
        OperationPhase::RecoveryRequired => "recovery_required",
        OperationPhase::Failed => "failed",
    }
}

fn operation_phase_from_text(value: &str) -> Result<OperationPhase, StateError> {
    match value {
        "prepared" => Ok(OperationPhase::Prepared),
        "external_effect_started" => Ok(OperationPhase::ExternalEffectStarted),
        "awaiting_reconciliation" => Ok(OperationPhase::AwaitingReconciliation),
        "committed" => Ok(OperationPhase::Committed),
        "recovery_required" => Ok(OperationPhase::RecoveryRequired),
        "failed" => Ok(OperationPhase::Failed),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

fn runtime_status_from_text(value: &str) -> Result<RuntimeStatus, StateError> {
    match value {
        "starting" => Ok(RuntimeStatus::Starting),
        "idle" => Ok(RuntimeStatus::Idle),
        "working" => Ok(RuntimeStatus::Working),
        "attention" => Ok(RuntimeStatus::Attention),
        "stopped" => Ok(RuntimeStatus::Stopped),
        "unknown" => Ok(RuntimeStatus::Unknown),
        "unreachable" => Ok(RuntimeStatus::Unreachable),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

fn workstream_lifecycle_from_text(value: &str) -> Result<WorkstreamLifecycle, StateError> {
    match value {
        "open" => Ok(WorkstreamLifecycle::Open),
        "parked" => Ok(WorkstreamLifecycle::Parked),
        "recovery_required" => Ok(WorkstreamLifecycle::RecoveryRequired),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

fn name_state_from_text(value: &str) -> Result<NameState, StateError> {
    match value {
        "named" => Ok(NameState::Named),
        "known_empty" => Ok(NameState::KnownEmpty),
        "unavailable" => Ok(NameState::Unavailable),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

const fn integration_lifecycle_text(lifecycle: IntegrationLifecycle) -> &'static str {
    match lifecycle {
        IntegrationLifecycle::TrustPending => "trust_pending",
        IntegrationLifecycle::Ready => "ready",
        IntegrationLifecycle::Modified => "modified",
        IntegrationLifecycle::Disabled => "disabled",
    }
}

fn integration_lifecycle_from_text(value: &str) -> Result<IntegrationLifecycle, StateError> {
    match value {
        "trust_pending" => Ok(IntegrationLifecycle::TrustPending),
        "ready" => Ok(IntegrationLifecycle::Ready),
        "modified" => Ok(IntegrationLifecycle::Modified),
        "disabled" => Ok(IntegrationLifecycle::Disabled),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

fn validate_registry_text(name: &'static str, value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') || value.contains('\n') {
        return Err(StateError::InvalidRegistryField(name));
    }
    Ok(())
}

fn validate_provider_metadata(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\n', '\r']) {
        return Err(StateError::InvalidProviderMetadata);
    }
    Ok(())
}

fn is_managed_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.starts_with("wsnav/")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/-_.".contains(&byte))
        && !value.contains("..")
        && !value.ends_with(['.', '/'])
}

fn is_git_commit(value: &str) -> bool {
    (40..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_project_display_name(value: &str) -> Result<(), StateError> {
    if value.trim().is_empty() || value.chars().count() > 128 || value.contains(['\0', '\n', '\r'])
    {
        return Err(StateError::InvalidProjectDisplayName);
    }
    Ok(())
}

fn validate_repository_fingerprint(value: Option<&str>) -> Result<(), StateError> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(hash) = value.strip_prefix("git-remote-v1:") else {
        return Err(StateError::InvalidRepositoryFingerprint);
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StateError::InvalidRepositoryFingerprint);
    }
    Ok(())
}

fn validate_client_host_alias(value: &str) -> Result<(), StateError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(StateError::InvalidClientHostAlias);
    }
    Ok(())
}

fn validate_client_host_text(name: &'static str, value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 1024 || value.contains(['\0', '\n', '\r']) {
        return Err(StateError::InvalidClientHostField(name));
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), StateError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), StateError> {
    Ok(())
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
    #[error("managed worktree plan could not be encoded")]
    ManagedWorktreePlanEncoding(serde_json::Error),
    #[error("managed worktree plan is missing from its operation")]
    MissingManagedWorktreePlan,
    #[error("managed worktree plan is invalid")]
    InvalidManagedWorktreePlan(serde_json::Error),
    #[error("managed worktree plan has an invalid shape")]
    InvalidManagedWorktreePlanShape,
    #[error("managed worktree plan does not match the durable operation")]
    ManagedWorktreePlanMismatch,
    #[error("managed worktree operation is not ready to commit")]
    ManagedWorktreeOperationUnavailable,
    #[error("managed worktree operation committed without its expected Workstream")]
    ManagedWorktreeCommitMissing,
    #[error("managed worktree operation outcome is missing")]
    MissingManagedWorktreeOutcome,
    #[error("managed worktree operation outcome is invalid")]
    InvalidManagedWorktreeOutcome(serde_json::Error),
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[derive(Default)]
    struct SequenceIds(AtomicU64);

    impl IdGenerator for SequenceIds {
        fn uuid(&self) -> Uuid {
            Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed) + 1))
        }
    }

    fn registry() -> (tempfile::TempDir, HostRegistry) {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        (temporary, registry)
    }

    fn settled_runtime(registry: &mut HostRegistry, workstream_id: WorkstreamId) -> RuntimeRecord {
        let initial = registry.reserve_runtime(workstream_id).unwrap();
        let cwd = initial.cwd.to_string_lossy().into_owned();
        registry
            .record_runtime_process_birth(initial.runtime_id, initial.revision, "birth-a")
            .unwrap();
        for event in [
            HookObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
            HookObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: None,
            },
            HookObservation {
                event: LifecycleEvent::Stop,
                cwd,
                native_session_id: "session-a".to_owned(),
                turn_id: Some("settled-a".to_owned()),
                source: None,
            },
        ] {
            let runtime = registry
                .runtime_for_workstream(workstream_id)
                .unwrap()
                .unwrap();
            registry
                .apply_hook_observation(runtime.runtime_id, &runtime.tmux_generation, event)
                .unwrap();
        }
        registry
            .runtime_for_workstream(workstream_id)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn newly_reserved_runtime_records_the_complete_private_session_identity() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();

        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();

        assert_eq!(
            runtime.tmux_session,
            format!("wsnav-{}", runtime.runtime_id)
        );
    }

    #[test]
    fn hook_candidates_are_limited_to_live_process_fingerprinted_runtimes() {
        let (_temporary, mut registry) = registry();
        let first = registry
            .register_external_workstream(
                PathBuf::from("/disposable/first"),
                "first-repository".to_owned(),
                "first-commit".to_owned(),
            )
            .unwrap();
        let second = registry
            .register_external_workstream(
                PathBuf::from("/disposable/second"),
                "second-repository".to_owned(),
                "second-commit".to_owned(),
            )
            .unwrap();
        let first_runtime = registry.reserve_runtime(first.workstream_id).unwrap();
        let second_runtime = registry.reserve_runtime(second.workstream_id).unwrap();
        registry
            .record_runtime_process_birth(
                first_runtime.runtime_id,
                first_runtime.revision,
                "first-birth",
            )
            .unwrap();

        let candidates = registry.hook_runtime_candidates().unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].runtime_id, first_runtime.runtime_id);
        assert_ne!(candidates[0].runtime_id, second_runtime.runtime_id);
    }

    #[test]
    fn request_key_deduplicates_an_ambiguous_fork() {
        let (_temporary, mut registry) = registry();
        let (first, inserted_first) = registry
            .create_or_get_operation("fork-1".to_owned(), OperationKind::Fork, "{}".to_owned())
            .unwrap();
        let transitioned = registry
            .transition_operation(
                first.id,
                first.revision,
                OperationPhase::ExternalEffectStarted,
                Some("before-provider-call".to_owned()),
                None,
            )
            .unwrap();
        let (second, inserted_second) = registry
            .create_or_get_operation("fork-1".to_owned(), OperationKind::Fork, "{}".to_owned())
            .unwrap();

        assert!(inserted_first);
        assert!(!inserted_second);
        assert_eq!(second.id, first.id);
        assert_eq!(second.phase, transitioned.phase);
    }

    #[test]
    fn stale_operation_revision_cannot_commit() {
        let (_temporary, mut registry) = registry();
        let (operation, _) = registry
            .create_or_get_operation("start-1".to_owned(), OperationKind::Start, "{}".to_owned())
            .unwrap();
        let transitioned = registry
            .transition_operation(
                operation.id,
                operation.revision,
                OperationPhase::ExternalEffectStarted,
                None,
                None,
            )
            .unwrap();

        assert!(matches!(
            registry.transition_operation(
                operation.id,
                operation.revision,
                OperationPhase::Committed,
                None,
                Some("{}".to_owned()),
            ),
            Err(StateError::Domain(DomainError::RevisionConflict { .. }))
        ));
        assert_eq!(transitioned.phase, OperationPhase::ExternalEffectStarted);
    }

    #[test]
    fn result_attention_stays_unseen_until_the_current_revision_acknowledges_it() {
        let (_temporary, mut registry) = registry();
        let workstream_id = WorkstreamId::new();
        let first = registry
            .mark_result_attention(workstream_id, "session-a".to_owned(), "turn-a".to_owned())
            .unwrap();
        let second = registry
            .mark_result_attention(workstream_id, "session-a".to_owned(), "turn-b".to_owned())
            .unwrap();

        assert_eq!(
            first.result_unseen_since_revision,
            second.result_unseen_since_revision
        );
        assert!(matches!(
            registry.acknowledge_result_attention(workstream_id, first.revision),
            Err(StateError::Domain(DomainError::RevisionConflict { .. }))
        ));
        let acknowledged = registry
            .acknowledge_result_attention(workstream_id, second.revision)
            .unwrap();
        assert_eq!(acknowledged.result_unseen_since_revision, None);
    }

    #[test]
    fn archive_and_restore_change_only_visibility_with_revision_guards() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir".to_owned(),
                "main".to_owned(),
            )
            .unwrap();
        let attention = registry
            .mark_result_attention(
                registered.workstream_id,
                "native-session".to_owned(),
                "settled-turn".to_owned(),
            )
            .unwrap();
        let before = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == registered.workstream_id)
            .unwrap();

        let archived_revision = registry
            .archive_workstream(registered.workstream_id, before.revision, 1_234)
            .unwrap();
        let archived = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == registered.workstream_id)
            .unwrap();
        assert_eq!(archived.archived_at_millis, Some(1_234));
        assert_eq!(archived.lifecycle, before.lifecycle);
        assert_eq!(archived.checkout_path, before.checkout_path);
        assert_eq!(archived.attention, before.attention);
        assert_eq!(archived.revision, archived_revision);
        assert!(matches!(
            registry.reserve_runtime(registered.workstream_id),
            Err(StateError::WorkstreamArchived(id)) if id == registered.workstream_id
        ));
        assert!(matches!(
            registry.workstream_git_location(registered.workstream_id),
            Err(StateError::WorkstreamArchived(id)) if id == registered.workstream_id
        ));
        assert_eq!(
            registry
                .workstream_git_location_for_start(registered.workstream_id)
                .unwrap()
                .repository_path,
            PathBuf::from("/disposable/repository")
        );
        let prepared = registry
            .prepare_managed_workstream(
                "archived-location-start".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                archived_revision,
                "a".repeat(40),
            )
            .unwrap();
        assert_eq!(prepared.plan.source_workstream_id, registered.workstream_id);
        assert!(matches!(
            registry.prepare_managed_workstream(
                "archived-location-fork".to_owned(),
                OperationKind::Fork,
                registered.workstream_id,
                archived_revision,
                "b".repeat(40),
            ),
            Err(StateError::WorkstreamArchived(id)) if id == registered.workstream_id
        ));
        assert!(matches!(
            registry.restore_workstream(registered.workstream_id, before.revision),
            Err(StateError::Domain(DomainError::RevisionConflict { .. }))
        ));

        let restored_revision = registry
            .restore_workstream(registered.workstream_id, archived_revision)
            .unwrap();
        let restored = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == registered.workstream_id)
            .unwrap();
        assert_eq!(restored.archived_at_millis, None);
        assert_eq!(restored.lifecycle, before.lifecycle);
        assert_eq!(restored.attention, Some(attention));
        assert_eq!(restored.revision, restored_revision);
        assert!(matches!(
            registry.restore_workstream(registered.workstream_id, restored_revision),
            Err(StateError::WorkstreamNotArchived(id)) if id == registered.workstream_id
        ));
    }

    #[test]
    fn state_files_are_private_on_unix() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let _registry = HostRegistry::open(&root).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(temporary.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(root.host_database_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn client_catalog_uses_its_own_schema() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let catalog = ClientCatalog::open(&root).unwrap();

        assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
    }

    #[test]
    fn client_catalog_groups_a_location_with_an_explicit_project_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let host = registry.identity().unwrap();
        let location_id = LocationId::new();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let recorded = catalog
            .register_local_project_location(
                &host,
                location_id,
                Path::new("/workspace/wsnav"),
                "wsnav",
            )
            .unwrap();
        let loaded = catalog
            .local_project_location(host.host_id, location_id)
            .unwrap()
            .unwrap();

        assert_eq!(loaded, recorded);
    }

    #[test]
    fn client_catalog_can_forget_project_locations_without_touching_host_state() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let host = registry.identity().unwrap();
        let location_id = LocationId::new();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let project = catalog
            .register_local_project_location(
                &host,
                location_id,
                Path::new("/workspace/wsnav"),
                "wsnav",
            )
            .unwrap();

        assert_eq!(
            catalog
                .ignore_project_locations(project.project_id)
                .unwrap(),
            1
        );
        assert!(
            catalog
                .project_location_is_ignored(host.host_id, location_id)
                .unwrap()
        );
        assert!(
            !catalog
                .project_location_is_ignored(host.host_id, LocationId::new())
                .unwrap()
        );
        assert!(
            catalog
                .local_project_location(host.host_id, location_id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn client_catalog_groups_matching_repository_fingerprints_across_hosts() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let local_host = registry.identity().unwrap();
        let remote_host = HostIdentity {
            host_id: HostId::new(),
            registry_generation: "remote-generation".to_owned(),
        };
        let local_location = LocationId::new();
        let remote_location = LocationId::new();
        let fingerprint = format!("git-remote-v1:{}", "a".repeat(64));
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let local = catalog
            .register_local_project_location_with_identity(
                &local_host,
                local_location,
                Path::new("/workspace/wsnav"),
                "cubey",
                Some(&fingerprint),
            )
            .unwrap();
        catalog
            .register_ssh_host(
                "snap",
                &remote_host,
                Path::new("/bin/wsnav"),
                "snap",
                Capabilities::default(),
            )
            .unwrap();

        let remote = catalog
            .register_host_project_location(
                remote_host.host_id,
                remote_location,
                "different-checkout-name",
                Some(&fingerprint),
            )
            .unwrap();

        assert_eq!(remote.project_id, local.project_id);
        assert_eq!(remote.display_name, "cubey");
    }

    #[test]
    fn client_catalog_keeps_missing_and_distinct_fingerprints_separate() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let host = registry.identity().unwrap();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let first = catalog
            .register_local_project_location_with_identity(
                &host,
                LocationId::new(),
                Path::new("/workspace/wsnav"),
                "first",
                Some(&format!("git-remote-v1:{}", "a".repeat(64))),
            )
            .unwrap();
        let second = catalog
            .register_local_project_location_with_identity(
                &host,
                LocationId::new(),
                Path::new("/workspace/wsnav"),
                "second",
                Some(&format!("git-remote-v1:{}", "b".repeat(64))),
            )
            .unwrap();
        let absent = catalog
            .register_local_project_location(
                &host,
                LocationId::new(),
                Path::new("/workspace/wsnav"),
                "absent",
            )
            .unwrap();

        assert_ne!(first.project_id, second.project_id);
        assert_ne!(first.project_id, absent.project_id);
        assert_ne!(second.project_id, absent.project_id);
    }

    #[test]
    fn repository_metadata_backfill_refreshes_a_single_location_label() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let host = registry.identity().unwrap();
        let location_id = LocationId::new();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let legacy = catalog
            .register_local_project_location(
                &host,
                location_id,
                Path::new("/bin/wsnav"),
                "cubey-worktree1",
            )
            .unwrap();
        let fingerprint = format!("git-remote-v1:{}", "e".repeat(64));

        let refreshed = catalog
            .register_local_project_location_with_identity(
                &host,
                location_id,
                Path::new("/bin/wsnav"),
                "cubey",
                Some(&fingerprint),
            )
            .unwrap();

        assert_eq!(refreshed.project_id, legacy.project_id);
        assert_eq!(refreshed.display_name, "cubey");
    }

    #[test]
    fn host_registry_migrates_repository_metadata_as_pending() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let legacy_sql = HOST_SCHEMA_SQL
            .replace("        repository_display_name TEXT NOT NULL,\n", "")
            .replace("        remote_identity_fingerprint TEXT,\n", "");
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection.execute_batch(&legacy_sql).unwrap();
        connection
            .execute(
                "INSERT INTO host_identity VALUES (1, ?1, 'generation', 4)",
                [HostId::new().to_string()],
            )
            .unwrap();
        let location_id = LocationId::new();
        connection
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_identity, repository_path,
                    default_base_ref, managed_worktree_root, revision
                 ) VALUES (?1, '/repo/.git', '/repo', ?2, '/state/worktrees/location', 1)",
                params![location_id.to_string(), "a".repeat(40)],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA user_version = 4;")
            .unwrap();
        drop(connection);

        let mut registry = HostRegistry::open(&root).unwrap();
        assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
        assert_eq!(
            registry.pending_repository_metadata().unwrap(),
            vec![PendingRepositoryMetadata {
                location_id,
                repository_path: PathBuf::from("/repo"),
            }]
        );
        let fingerprint = format!("git-remote-v1:{}", "c".repeat(64));
        registry
            .record_repository_metadata(
                location_id,
                Path::new("/primary/repo"),
                "repo",
                Some(&fingerprint),
            )
            .unwrap();
        assert!(registry.pending_repository_metadata().unwrap().is_empty());
    }

    #[test]
    fn client_catalog_migrates_legacy_local_host_records() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let connection = Connection::open(root.client_database_path()).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE hosts (
                    host_alias TEXT PRIMARY KEY,
                    host_id TEXT NOT NULL UNIQUE,
                    executable_path TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 INSERT INTO hosts VALUES ('local', '00000000-0000-0000-0000-000000000001', '/bin/wsnav', 1);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        drop(connection);

        let catalog = ClientCatalog::open(&root).unwrap();
        let migrated = catalog.host("local").unwrap().unwrap();

        assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
        assert_eq!(migrated.registry_generation, "");
        assert!(matches!(migrated.transport, ClientHostTransport::Local));
        assert_eq!(migrated.capabilities, Capabilities::default());
    }

    #[test]
    fn client_catalog_migrates_d6_projects_without_losing_associations() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let legacy_sql = CLIENT_SCHEMA_SQL
            .replace("        repository_fingerprint TEXT,\n", "")
            .replace(
                "    CREATE UNIQUE INDEX project_location_identity_idx\n        ON project_locations(host_id, location_id);\n",
                "",
            )
            .replace(
                "    CREATE UNIQUE INDEX project_repository_fingerprint_idx\n        ON projects(repository_fingerprint)\n        WHERE repository_fingerprint IS NOT NULL;\n",
                "",
            );
        let connection = Connection::open(root.client_database_path()).unwrap();
        connection.execute_batch(&legacy_sql).unwrap();
        let host_id = HostId::new();
        let location_id = LocationId::new();
        let project_id = ProjectId::new();
        connection
            .execute(
                "INSERT INTO hosts VALUES (
                    'local', ?1, 'generation', '/bin/wsnav', 'local', NULL,
                    '{\"codex\":false,\"git\":false,\"tmux\":false}', 1
                 )",
                [host_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO projects VALUES (?1, 'cubey', 1)",
                [project_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_locations VALUES (?1, ?2, ?3)",
                params![
                    project_id.to_string(),
                    host_id.to_string(),
                    location_id.to_string(),
                ],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA user_version = 2;")
            .unwrap();
        drop(connection);

        let mut catalog = ClientCatalog::open(&root).unwrap();
        let fingerprint = format!("git-remote-v1:{}", "d".repeat(64));
        let grouped = catalog
            .register_local_project_location_with_identity(
                &HostIdentity {
                    host_id,
                    registry_generation: "generation".to_owned(),
                },
                location_id,
                Path::new("/bin/wsnav"),
                "cubey",
                Some(&fingerprint),
            )
            .unwrap();

        assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
        assert_eq!(grouped.project_id, project_id);
    }

    #[test]
    fn registered_ssh_host_refuses_identity_generation_and_capability_drift() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let identity = HostIdentity {
            host_id: HostId::new(),
            registry_generation: "generation-a".to_owned(),
        };
        let capabilities = Capabilities {
            codex: true,
            git: true,
            tmux: true,
        };
        let registered = catalog
            .register_ssh_host(
                "snap",
                &identity,
                Path::new("/home/bryan/.local/bin/wsnav"),
                "snap",
                capabilities.clone(),
            )
            .unwrap();

        assert!(matches!(
            registered.transport,
            ClientHostTransport::Ssh { ref destination } if destination == "snap"
        ));
        assert_eq!(catalog.ssh_hosts().unwrap(), vec![registered.clone()]);
        assert_eq!(
            catalog
                .verify_hello(
                    "snap",
                    &HelloResponse {
                        host_id: identity.host_id,
                        registry_generation: identity.registry_generation.clone(),
                        wsnav_version: "0.1.0".to_owned(),
                        capabilities: capabilities.clone(),
                    }
                )
                .unwrap(),
            registered
        );
        assert!(matches!(
            catalog.verify_hello(
                "snap",
                &HelloResponse {
                    host_id: HostId::new(),
                    registry_generation: identity.registry_generation.clone(),
                    wsnav_version: "0.1.0".to_owned(),
                    capabilities: capabilities.clone(),
                }
            ),
            Err(StateError::ClientHostIdentityMismatch)
        ));
        assert!(matches!(
            catalog.verify_hello(
                "snap",
                &HelloResponse {
                    host_id: identity.host_id,
                    registry_generation: "generation-b".to_owned(),
                    wsnav_version: "0.1.0".to_owned(),
                    capabilities: capabilities.clone(),
                }
            ),
            Err(StateError::ClientHostGenerationMismatch)
        ));
        assert!(matches!(
            catalog.verify_hello(
                "snap",
                &HelloResponse {
                    host_id: identity.host_id,
                    registry_generation: identity.registry_generation,
                    wsnav_version: "0.1.0".to_owned(),
                    capabilities: Capabilities::default(),
                }
            ),
            Err(StateError::ClientHostCapabilitiesMismatch)
        ));
    }

    #[test]
    fn explicit_ssh_reset_removes_only_client_registration_and_associations() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let identity = HostIdentity {
            host_id: HostId::new(),
            registry_generation: "generation".to_owned(),
        };
        catalog
            .register_ssh_host(
                "snap",
                &identity,
                Path::new("/home/bryan/.local/bin/wsnav"),
                "snap",
                Capabilities::default(),
            )
            .unwrap();

        catalog.reset_ssh_host("snap").unwrap();

        assert!(catalog.host("snap").unwrap().is_none());
        assert!(catalog.ssh_hosts().unwrap().is_empty());
    }

    #[test]
    fn fresh_registry_identity_is_stable_and_uses_the_injected_source() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let ids = SequenceIds::default();
        let first = HostRegistry::open_with_id_generator(&root, &ids).unwrap();
        let first_identity = first.identity().unwrap();
        let second = HostRegistry::open_with_id_generator(&root, &ids).unwrap();

        assert_eq!(first.schema_version().unwrap(), HOST_SCHEMA_VERSION);
        assert_eq!(first_identity.host_id, HostId::from(Uuid::from_u128(1)));
        assert_eq!(
            first_identity.registry_generation,
            Uuid::from_u128(2).to_string()
        );
        assert_eq!(second.identity().unwrap(), first_identity);
    }

    #[test]
    fn future_schema_versions_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 99;")
            .unwrap();

        assert!(matches!(
            HostRegistry::open(&root),
            Err(StateError::UnsupportedSchemaVersion(99))
        ));
    }

    #[test]
    fn v1_host_schema_migrates_activity_ordering_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE host_identity (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    host_id TEXT NOT NULL UNIQUE,
                    registry_generation TEXT NOT NULL,
                    schema_version INTEGER NOT NULL
                 );
                 INSERT INTO host_identity VALUES (1, 'host', 'generation', 1);
                 CREATE TABLE workstreams (
                    workstream_id TEXT PRIMARY KEY,
                    location_id TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    source_workstream_id TEXT,
                    checkout_id TEXT NOT NULL UNIQUE,
                    lifecycle TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        drop(connection);

        let registry = HostRegistry::open(&root).unwrap();
        assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
        let connection = Connection::open(root.host_database_path()).unwrap();
        let activity_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('workstreams')
                 WHERE name = 'last_activity_sequence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let activity_time_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('workstreams')
                 WHERE name = 'last_activity_at_millis'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let recorded_version: i64 = connection
            .query_row(
                "SELECT schema_version FROM host_identity WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(activity_column, 1);
        assert_eq!(activity_time_column, 1);
        assert_eq!(recorded_version, HOST_SCHEMA_VERSION);
    }

    #[test]
    fn v2_host_schema_adds_unknown_activity_time_without_inventing_one() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE host_identity (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    host_id TEXT NOT NULL UNIQUE,
                    registry_generation TEXT NOT NULL,
                    schema_version INTEGER NOT NULL
                 );
                 INSERT INTO host_identity VALUES (1, 'host', 'generation', 2);
                 CREATE TABLE workstreams (
                    workstream_id TEXT PRIMARY KEY,
                    location_id TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    source_workstream_id TEXT,
                    checkout_id TEXT NOT NULL UNIQUE,
                    lifecycle TEXT NOT NULL,
                    last_activity_sequence INTEGER NOT NULL CHECK (last_activity_sequence >= 0),
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        drop(connection);

        let registry = HostRegistry::open(&root).unwrap();
        assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
        let connection = Connection::open(root.host_database_path()).unwrap();
        let default_value: String = connection
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('workstreams')
                 WHERE name = 'last_activity_at_millis'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(default_value, "0");
    }

    #[test]
    fn v3_host_schema_backfills_the_managed_worktree_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        drop(registry);
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection
            .execute(
                "UPDATE project_locations SET managed_worktree_root = '' WHERE location_id = ?1",
                [registered.location_id.to_string()],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA user_version = 3;")
            .unwrap();
        drop(connection);

        let mut registry = HostRegistry::open(&root).unwrap();
        let prepared = registry
            .prepare_managed_workstream(
                "migration-root".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            )
            .unwrap();

        assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
        assert_eq!(
            prepared.plan.worktree_path.parent(),
            Some(
                root.base()
                    .join("worktrees")
                    .join(registered.location_id.to_string())
                    .as_path()
            )
        );
    }

    #[test]
    fn v5_host_schema_adds_archive_visibility_without_losing_workstreams() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path()).unwrap();
        let legacy_sql = HOST_SCHEMA_SQL.replace("        archived_at_millis INTEGER,\n", "");
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection.execute_batch(&legacy_sql).unwrap();
        connection
            .execute(
                "INSERT INTO host_identity VALUES (1, ?1, 'generation', 5)",
                [HostId::new().to_string()],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA user_version = 5;")
            .unwrap();
        drop(connection);

        let registry = HostRegistry::open(&root).unwrap();
        assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
        let connection = Connection::open(root.host_database_path()).unwrap();
        let archived_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('workstreams')
                 WHERE name = 'archived_at_millis'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(archived_column, 1);
    }

    #[test]
    fn deterministic_operation_identity_is_persisted_on_first_request() {
        let (_temporary, mut registry) = registry();
        let ids = SequenceIds::default();
        let (operation, inserted) = registry
            .create_or_get_operation_with_id_generator(
                "deterministic-start".to_owned(),
                OperationKind::Start,
                "{}".to_owned(),
                &ids,
            )
            .unwrap();

        assert!(inserted);
        assert_eq!(operation.id, OperationId::from(Uuid::from_u128(1)));
    }

    #[test]
    fn external_workstream_reserves_one_runtime_until_it_is_stopped() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let first = registry.reserve_runtime(registered.workstream_id).unwrap();

        assert_eq!(first.status, RuntimeStatus::Starting);
        assert!(matches!(
            registry.reserve_runtime(registered.workstream_id),
            Err(StateError::RuntimeAlreadyLive(id)) if id == registered.workstream_id
        ));
        registry
            .mark_runtime_stopped(first.runtime_id, first.revision)
            .unwrap();
        let resumed = registry.reserve_runtime(registered.workstream_id).unwrap();

        assert_eq!(resumed.runtime_id, first.runtime_id);
        assert_ne!(resumed.tmux_generation, first.tmux_generation);
        assert_eq!(resumed.status, RuntimeStatus::Starting);
    }

    #[test]
    fn managed_workstream_plan_is_durable_and_deduplicated_before_git() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let first = registry
            .prepare_managed_workstream(
                "independent-1".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            )
            .unwrap();
        let repeated = registry
            .prepare_managed_workstream(
                "independent-1".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "b".repeat(40),
            )
            .unwrap();

        assert!(first.newly_prepared);
        assert!(!repeated.newly_prepared);
        assert_eq!(first.plan, repeated.plan);
        assert_eq!(
            first.plan.operation.phase,
            OperationPhase::ExternalEffectStarted
        );
        assert_eq!(first.plan.origin, WorkstreamOrigin::Independent);
        assert_eq!(first.plan.source_native_session_id, None);
        assert_eq!(
            first.plan.worktree_path.parent(),
            Some(registered.managed_worktree_root.as_path())
        );
    }

    #[test]
    fn managed_workstream_commit_records_one_owned_checkout_without_runtime() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let prepared = registry
            .prepare_managed_workstream(
                "independent-commit".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            )
            .unwrap();
        let committed = registry.commit_managed_workstream(&prepared.plan).unwrap();
        let replayed = registry.commit_managed_workstream(&prepared.plan).unwrap();
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == committed.workstream_id)
            .unwrap();
        let ownership: String = registry
            .connection
            .query_row(
                "SELECT ownership FROM checkouts WHERE checkout_id = ?1",
                [committed.checkout_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(committed, replayed);
        assert_eq!(committed.origin, WorkstreamOrigin::Independent);
        assert_eq!(committed.source_workstream_id, registered.workstream_id);
        assert_eq!(overview.checkout_path, prepared.plan.worktree_path);
        assert!(overview.runtime.is_none());
        assert_eq!(ownership, "managed");
    }

    #[test]
    fn fork_requires_one_live_source_binding_with_a_settled_turn() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        assert!(matches!(
            registry.prepare_managed_workstream(
                "fork-no-boundary".to_owned(),
                OperationKind::Fork,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            ),
            Err(StateError::ForkBoundaryUnavailable)
        ));

        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let cwd = runtime.cwd.to_string_lossy().into_owned();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: cwd.clone(),
                    native_session_id: "source-session".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            )
            .unwrap();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::UserPromptSubmit,
                    cwd: cwd.clone(),
                    native_session_id: "source-session".to_owned(),
                    turn_id: None,
                    source: None,
                },
            )
            .unwrap();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::Stop,
                    cwd,
                    native_session_id: "source-session".to_owned(),
                    turn_id: Some("settled-turn".to_owned()),
                    source: None,
                },
            )
            .unwrap();
        let source_revision = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == registered.workstream_id)
            .unwrap()
            .revision;
        let prepared = registry
            .prepare_managed_workstream(
                "fork-settled".to_owned(),
                OperationKind::Fork,
                registered.workstream_id,
                source_revision,
                "a".repeat(40),
            )
            .unwrap();

        assert_eq!(prepared.plan.origin, WorkstreamOrigin::Fork);
        assert_eq!(prepared.plan.source_runtime_id, Some(runtime.runtime_id));
        assert_eq!(
            prepared.plan.source_native_session_id.as_deref(),
            Some("source-session")
        );
        assert_eq!(
            prepared.plan.last_settled_turn_id.as_deref(),
            Some("settled-turn")
        );
        let created = registry
            .commit_forked_managed_workstream(&prepared.plan, "destination-session")
            .unwrap();
        let destination_runtime = registry
            .runtime_for_workstream(created.workstream_id)
            .unwrap()
            .unwrap();
        let destination_binding = registry
            .binding_for_runtime(destination_runtime.runtime_id)
            .unwrap()
            .unwrap();

        assert_eq!(destination_runtime.status, RuntimeStatus::Stopped);
        assert_eq!(destination_binding.start_source, "resume");
        assert_eq!(destination_binding.native_session_id, "destination-session");
    }

    #[test]
    fn unresolved_managed_worktree_operation_remains_recovery_required() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let prepared = registry
            .prepare_managed_workstream(
                "independent-recovery".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            )
            .unwrap();
        registry
            .mark_managed_workstream_recovery(&prepared.plan)
            .unwrap();
        let replay = registry
            .prepare_managed_workstream(
                "independent-recovery".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            )
            .unwrap();

        assert_eq!(
            replay.plan.operation.phase,
            OperationPhase::RecoveryRequired
        );
        assert!(matches!(
            registry.commit_managed_workstream(&replay.plan),
            Err(StateError::ManagedWorktreeOperationUnavailable)
        ));
    }

    #[test]
    fn explicit_recovery_exposes_and_commits_only_the_exact_start_plan() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let prepared = registry
            .prepare_managed_workstream(
                "private-request-key".to_owned(),
                OperationKind::Start,
                registered.workstream_id,
                Revision::INITIAL,
                "a".repeat(40),
            )
            .unwrap();
        registry
            .mark_managed_workstream_recovery(&prepared.plan)
            .unwrap();

        let operations = registry.unresolved_operation_overviews().unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].operation_id, prepared.plan.operation.id);
        assert_eq!(operations[0].kind, OperationKind::Start);
        assert_eq!(operations[0].phase, OperationPhase::RecoveryRequired);

        let recovered_plan = registry
            .managed_workstream_plan(prepared.plan.operation.id)
            .unwrap();
        assert_eq!(
            recovered_plan.operation.phase,
            OperationPhase::RecoveryRequired
        );
        let created = registry
            .commit_recovered_managed_workstream(&recovered_plan)
            .unwrap();

        assert_eq!(created.workstream_id, prepared.plan.workstream_id);
        assert!(
            registry
                .unresolved_operation_overviews()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn fork_attempt_marker_is_atomic_and_prevents_a_second_provider_call() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let cwd = runtime.cwd.to_string_lossy().into_owned();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: cwd.clone(),
                    native_session_id: "source-session".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            )
            .unwrap();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::Stop,
                    cwd,
                    native_session_id: "source-session".to_owned(),
                    turn_id: Some("settled-turn".to_owned()),
                    source: None,
                },
            )
            .unwrap();
        let source_revision = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == registered.workstream_id)
            .unwrap()
            .revision;
        let prepared = registry
            .prepare_managed_workstream(
                "fork-attempt".to_owned(),
                OperationKind::Fork,
                registered.workstream_id,
                source_revision,
                "a".repeat(40),
            )
            .unwrap();

        let marked = registry
            .record_managed_fork_attempt(&prepared.plan)
            .unwrap();
        assert!(marked.fork_attempted_at_millis.is_some());
        assert!(matches!(
            registry.record_managed_fork_attempt(&marked),
            Err(StateError::ManagedWorktreeOperationUnavailable)
        ));
    }

    #[test]
    fn explicit_park_is_distinct_from_an_observed_runtime_stop() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let unexpected_stop = registry.reserve_runtime(registered.workstream_id).unwrap();
        registry
            .mark_runtime_stopped(unexpected_stop.runtime_id, unexpected_stop.revision)
            .unwrap();
        assert!(
            !registry
                .runtime_is_deliberately_parked(
                    unexpected_stop.runtime_id,
                    registered.workstream_id,
                )
                .unwrap()
        );

        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        registry
            .park_runtime(runtime.runtime_id, runtime.revision)
            .unwrap();
        assert!(
            registry
                .runtime_is_deliberately_parked(runtime.runtime_id, registered.workstream_id)
                .unwrap()
        );
        assert_eq!(
            registry.workstream_overviews().unwrap()[0].lifecycle,
            WorkstreamLifecycle::Parked
        );

        registry.reserve_runtime(registered.workstream_id).unwrap();
        assert_eq!(
            registry.workstream_overviews().unwrap()[0].lifecycle,
            WorkstreamLifecycle::Open
        );
    }

    #[test]
    fn conversation_activity_time_survives_park_and_resume() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let cwd = runtime.cwd.to_string_lossy().into_owned();

        assert_eq!(
            registry.workstream_overviews().unwrap()[0].last_activity_at_millis,
            None
        );
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: cwd.clone(),
                    native_session_id: "session-a".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            )
            .unwrap();
        assert_eq!(
            registry.workstream_overviews().unwrap()[0].last_activity_at_millis,
            None
        );
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::UserPromptSubmit,
                    cwd: cwd.clone(),
                    native_session_id: "session-a".to_owned(),
                    turn_id: Some("turn-a".to_owned()),
                    source: None,
                },
            )
            .unwrap();
        let activity_at_millis = registry.workstream_overviews().unwrap()[0]
            .last_activity_at_millis
            .unwrap();
        let live = registry
            .runtime_for_workstream(registered.workstream_id)
            .unwrap()
            .unwrap();
        registry
            .park_runtime(live.runtime_id, live.revision)
            .unwrap();
        registry.reserve_runtime(registered.workstream_id).unwrap();

        assert_eq!(
            registry.workstream_overviews().unwrap()[0].last_activity_at_millis,
            Some(activity_at_millis)
        );
    }

    #[test]
    fn navigator_overview_joins_only_bounded_workstream_metadata() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: runtime.cwd.to_string_lossy().into_owned(),
                    native_session_id: "session-a".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            )
            .unwrap();
        registry
            .record_thread_metadata(runtime.runtime_id, "session-a", Some("Native title"))
            .unwrap();
        let overview = registry.workstream_overviews().unwrap();

        assert_eq!(overview.len(), 1);
        assert_eq!(overview[0].workstream_id, registered.workstream_id);
        assert_eq!(overview[0].lifecycle, WorkstreamLifecycle::Open);
        assert_eq!(
            overview[0]
                .binding
                .as_ref()
                .and_then(|binding| binding.observed_thread_name.as_deref()),
            Some("Native title")
        );
        assert_eq!(
            overview[0]
                .binding
                .as_ref()
                .map(|binding| binding.name_state),
            Some(NameState::Named)
        );
    }

    #[test]
    fn lost_runtime_requires_verified_native_resume_to_reopen() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let lost = settled_runtime(&mut registry, registered.workstream_id);
        registry
            .mark_runtime_recovery_required(lost.runtime_id, lost.revision)
            .unwrap();

        let overview = registry.workstream_overviews().unwrap().remove(0);
        assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
        assert_eq!(overview.runtime.unwrap().status, RuntimeStatus::Unknown);
        assert_eq!(
            overview.binding.unwrap().native_session_id,
            "session-a".to_owned()
        );
        assert!(
            overview
                .attention
                .as_ref()
                .and_then(|attention| attention.recovery_unseen_since_revision)
                .is_some()
        );
        assert!(
            overview
                .attention
                .as_ref()
                .and_then(|attention| attention.result_unseen_since_revision)
                .is_some()
        );

        let recovery = registry
            .reserve_runtime_recovery(registered.workstream_id)
            .unwrap();
        let cwd = recovery.cwd.to_string_lossy().into_owned();
        assert!(matches!(
            registry.apply_hook_observation(
                recovery.runtime_id,
                &recovery.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: cwd.clone(),
                    native_session_id: "session-a".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            ),
            Err(StateError::HookEvidenceMismatch)
        ));
        registry
            .apply_hook_observation(
                recovery.runtime_id,
                &recovery.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd,
                    native_session_id: "session-a".to_owned(),
                    turn_id: None,
                    source: Some("resume".to_owned()),
                },
            )
            .unwrap();

        let reopened = registry.workstream_overviews().unwrap().remove(0);
        assert_eq!(reopened.lifecycle, WorkstreamLifecycle::Open);
        assert_eq!(reopened.runtime.unwrap().status, RuntimeStatus::Idle);
        let attention = reopened.attention.unwrap();
        assert_eq!(attention.recovery_unseen_since_revision, None);
        assert!(attention.result_unseen_since_revision.is_some());
    }

    #[test]
    fn unbound_runtime_recovery_accepts_only_a_native_resume_picker_selection() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        registry
            .record_runtime_process_birth(runtime.runtime_id, runtime.revision, "birth-a")
            .unwrap();
        let launched = registry
            .runtime_for_workstream(registered.workstream_id)
            .unwrap()
            .unwrap();
        registry
            .mark_runtime_recovery_required(launched.runtime_id, launched.revision)
            .unwrap();
        let recovery = registry
            .reserve_runtime_recovery(registered.workstream_id)
            .unwrap();
        let observation = |source: &str| HookObservation {
            event: LifecycleEvent::SessionStart,
            cwd: recovery.cwd.to_string_lossy().into_owned(),
            native_session_id: "selected-session".to_owned(),
            turn_id: None,
            source: Some(source.to_owned()),
        };

        assert!(matches!(
            registry.apply_hook_observation(
                recovery.runtime_id,
                &recovery.tmux_generation,
                observation("startup"),
            ),
            Err(StateError::HookEvidenceMismatch)
        ));
        registry
            .apply_hook_observation(
                recovery.runtime_id,
                &recovery.tmux_generation,
                observation("resume"),
            )
            .unwrap();
        let overview = registry.workstream_overviews().unwrap().remove(0);
        assert_eq!(overview.lifecycle, WorkstreamLifecycle::Open);
        assert_eq!(
            overview.binding.unwrap().native_session_id,
            "selected-session".to_owned()
        );
    }

    #[test]
    fn navigator_overview_orders_latest_observed_activity_first() {
        let (_temporary, mut registry) = registry();
        let first = registry
            .register_external_workstream(
                PathBuf::from("/disposable/first"),
                "first-repository".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let second = registry
            .register_external_workstream(
                PathBuf::from("/disposable/second"),
                "second-repository".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(first.workstream_id).unwrap();
        let cwd = runtime.cwd.to_string_lossy().into_owned();
        for observation in [
            HookObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
            HookObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
            HookObservation {
                event: LifecycleEvent::Stop,
                cwd,
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
        ] {
            registry
                .apply_hook_observation(runtime.runtime_id, &runtime.tmux_generation, observation)
                .unwrap();
        }

        let overview = registry.workstream_overviews().unwrap();

        assert_eq!(
            overview
                .iter()
                .map(|entry| entry.workstream_id)
                .collect::<Vec<_>>(),
            vec![first.workstream_id, second.workstream_id]
        );
        assert!(overview[0].last_activity_sequence > overview[1].last_activity_sequence);
        assert!(overview[0].last_activity_at_millis.is_some());
        assert_eq!(overview[1].last_activity_at_millis, None);
    }

    #[test]
    fn navigator_snapshot_pages_past_its_per_page_workstream_limit() {
        let (_temporary, mut registry) = registry();
        for index in 0..=MAX_NAVIGATOR_WORKSTREAMS {
            registry
                .register_external_workstream(
                    PathBuf::from(format!("/disposable/repository-{index}")),
                    format!("common-dir-identity-{index}"),
                    "deadbeef".to_owned(),
                )
                .unwrap();
        }

        let first = registry
            .workstream_overview_page(0, MAX_NAVIGATOR_WORKSTREAMS)
            .unwrap();
        assert_eq!(first.workstreams.len(), MAX_NAVIGATOR_WORKSTREAMS);
        assert_eq!(
            first.next_cursor,
            Some(u32::try_from(MAX_NAVIGATOR_WORKSTREAMS).unwrap())
        );
        let second = registry
            .workstream_overview_page(first.next_cursor.unwrap(), MAX_NAVIGATOR_WORKSTREAMS)
            .unwrap();
        assert_eq!(second.workstreams.len(), 1);
        assert_eq!(second.next_cursor, None);
        assert_eq!(
            registry.workstream_overviews().unwrap().len(),
            MAX_NAVIGATOR_WORKSTREAMS + 1
        );
    }

    #[test]
    fn hook_process_birth_must_match_the_prepared_runtime_generation() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();

        assert!(matches!(
            registry.expected_hook_process_birth(runtime.runtime_id, &runtime.tmux_generation),
            Err(StateError::HookEvidenceMismatch)
        ));
        registry
            .record_runtime_process_birth(runtime.runtime_id, runtime.revision, "birth-1")
            .unwrap();
        assert_eq!(
            registry
                .expected_hook_process_birth(runtime.runtime_id, &runtime.tmux_generation)
                .unwrap(),
            "birth-1"
        );
        assert!(matches!(
            registry.expected_hook_process_birth(runtime.runtime_id, "stale-generation"),
            Err(StateError::HookEvidenceMismatch)
        ));
    }

    #[test]
    fn matching_hook_lifecycle_binds_and_sets_sticky_result_attention_atomically() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let start = HookObservation {
            event: LifecycleEvent::SessionStart,
            cwd: runtime.cwd.to_string_lossy().into_owned(),
            native_session_id: "session-a".to_owned(),
            turn_id: None,
            source: Some("startup".to_owned()),
        };
        registry
            .apply_hook_observation(runtime.runtime_id, &runtime.tmux_generation, start)
            .unwrap();
        let prompt = HookObservation {
            event: LifecycleEvent::UserPromptSubmit,
            cwd: runtime.cwd.to_string_lossy().into_owned(),
            native_session_id: "session-a".to_owned(),
            turn_id: Some("turn-a".to_owned()),
            source: None,
        };
        registry
            .apply_hook_observation(runtime.runtime_id, &runtime.tmux_generation, prompt)
            .unwrap();
        let stop = HookObservation {
            event: LifecycleEvent::Stop,
            cwd: runtime.cwd.to_string_lossy().into_owned(),
            native_session_id: "session-a".to_owned(),
            turn_id: Some("turn-a".to_owned()),
            source: None,
        };
        registry
            .apply_hook_observation(runtime.runtime_id, &runtime.tmux_generation, stop)
            .unwrap();

        assert_eq!(
            registry
                .binding_for_runtime(runtime.runtime_id)
                .unwrap()
                .unwrap()
                .last_settled_turn_id
                .as_deref(),
            Some("turn-a")
        );
        assert_eq!(
            registry
                .attention(registered.workstream_id)
                .unwrap()
                .unwrap()
                .latest_turn_id
                .as_deref(),
            Some("turn-a")
        );
    }

    #[test]
    fn proven_idle_clear_rotates_only_the_current_conversation_tip() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let cwd = runtime.cwd.to_string_lossy().into_owned();
        for observation in [
            HookObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
            HookObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
            HookObservation {
                event: LifecycleEvent::Stop,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
        ] {
            registry
                .apply_hook_observation(runtime.runtime_id, &runtime.tmux_generation, observation)
                .unwrap();
        }

        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd,
                    native_session_id: "session-b".to_owned(),
                    turn_id: None,
                    source: Some("clear".to_owned()),
                },
            )
            .unwrap();

        let binding = registry
            .binding_for_runtime(runtime.runtime_id)
            .unwrap()
            .unwrap();
        assert_eq!(binding.native_session_id, "session-b");
        assert_eq!(binding.start_source, "clear");
        assert_eq!(binding.last_settled_turn_id, None);
        assert_eq!(
            binding.predecessor_native_session_id.as_deref(),
            Some("session-a")
        );
        assert_eq!(
            registry
                .runtime_for_workstream(registered.workstream_id)
                .unwrap()
                .unwrap()
                .status,
            RuntimeStatus::Idle
        );
        assert_eq!(
            registry
                .attention(registered.workstream_id)
                .unwrap()
                .unwrap()
                .latest_native_session_id
                .as_deref(),
            Some("session-a")
        );
    }

    #[test]
    fn changed_session_start_rejects_unproven_sources_and_working_turns() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let cwd = runtime.cwd.to_string_lossy().into_owned();
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: cwd.clone(),
                    native_session_id: "session-a".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            )
            .unwrap();
        assert!(matches!(
            registry.apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd: cwd.clone(),
                    native_session_id: "session-b".to_owned(),
                    turn_id: None,
                    source: Some("startup".to_owned()),
                },
            ),
            Err(StateError::HookEvidenceMismatch)
        ));
        registry
            .apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::UserPromptSubmit,
                    cwd: cwd.clone(),
                    native_session_id: "session-a".to_owned(),
                    turn_id: Some("turn-a".to_owned()),
                    source: None,
                },
            )
            .unwrap();
        assert!(matches!(
            registry.apply_hook_observation(
                runtime.runtime_id,
                &runtime.tmux_generation,
                HookObservation {
                    event: LifecycleEvent::SessionStart,
                    cwd,
                    native_session_id: "session-b".to_owned(),
                    turn_id: None,
                    source: Some("clear".to_owned()),
                },
            ),
            Err(StateError::HookEvidenceMismatch)
        ));
        assert_eq!(
            registry
                .binding_for_runtime(runtime.runtime_id)
                .unwrap()
                .unwrap()
                .native_session_id,
            "session-a"
        );
    }

    #[test]
    fn stale_or_rebound_hook_cannot_replace_a_managed_session() {
        let (_temporary, mut registry) = registry();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir-identity".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        let forged = HookObservation {
            event: LifecycleEvent::SessionStart,
            cwd: runtime.cwd.to_string_lossy().into_owned(),
            native_session_id: "forged-session".to_owned(),
            turn_id: None,
            source: Some("startup".to_owned()),
        };

        assert!(matches!(
            registry.apply_hook_observation(runtime.runtime_id, "stale", forged),
            Err(StateError::HookEvidenceMismatch)
        ));
        assert_eq!(
            registry.binding_for_runtime(runtime.runtime_id).unwrap(),
            None
        );
    }

    #[test]
    fn observer_ownership_is_stable_and_lifecycle_is_explicit() {
        let (_temporary, mut registry) = registry();
        let ownership = ProfileOwnership {
            canonical_path: PathBuf::from("/private/codex/wsnav-observer.config.toml"),
            owner_id: "owner".to_owned(),
            profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
            hook_executable: PathBuf::from("/private/bin/wsnav"),
            content_hash: "hash".to_owned(),
        };
        let pending = registry
            .record_codex_integration(ownership.clone(), IntegrationLifecycle::TrustPending)
            .unwrap();
        let ready = registry
            .record_codex_integration(ownership, IntegrationLifecycle::Ready)
            .unwrap();

        assert_eq!(pending.lifecycle, IntegrationLifecycle::TrustPending);
        assert_eq!(ready.lifecycle, IntegrationLifecycle::Ready);
        assert!(ready.revision > pending.revision);
    }

    #[test]
    fn explicit_profile_update_rotates_exact_ownership_back_to_trust_pending() {
        let (_temporary, mut registry) = registry();
        let original = ProfileOwnership {
            canonical_path: PathBuf::from("/private/codex/wsnav-observer.config.toml"),
            owner_id: "owner".to_owned(),
            profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
            hook_executable: PathBuf::from("/private/bin/wsnav-old"),
            content_hash: "old-hash".to_owned(),
        };
        let ready = registry
            .record_codex_integration(original.clone(), IntegrationLifecycle::Ready)
            .unwrap();
        let replacement = ProfileOwnership {
            hook_executable: PathBuf::from("/private/bin/wsnav-new"),
            content_hash: "new-hash".to_owned(),
            ..original.clone()
        };

        let updated = registry
            .replace_codex_integration(
                &original,
                replacement.clone(),
                IntegrationLifecycle::TrustPending,
            )
            .unwrap();

        assert_eq!(updated.ownership, replacement);
        assert_eq!(updated.lifecycle, IntegrationLifecycle::TrustPending);
        assert!(updated.revision > ready.revision);
        assert!(matches!(
            registry.replace_codex_integration(
                &original,
                replacement,
                IntegrationLifecycle::TrustPending,
            ),
            Err(StateError::IntegrationOwnershipMismatch)
        ));
    }
}
