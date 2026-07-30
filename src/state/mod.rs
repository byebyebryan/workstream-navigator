use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AttentionState, CheckoutId, CompoundOperation, DomainError, HostId, IdGenerator, LocationId,
    OperationId, OperationKind, OperationPhase, ProjectId, RandomIdGenerator, Revision, RuntimeId,
    RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
};
use crate::protocol::{Capabilities, HelloResponse};
use crate::provider::codex::hooks::{HookObservation, LifecycleEvent};
use crate::provider::codex::names::NameState;
use crate::provider::codex::profile::{OBSERVER_PROFILE_NAME, ProfileOwnership};

const HOST_SCHEMA_VERSION: i64 = 2;
const CLIENT_SCHEMA_VERSION: i64 = 2;
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
        last_activity_sequence INTEGER NOT NULL CHECK (last_activity_sequence >= 0),
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

/// One V1 external checkout and its initial workstream registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalWorkstream {
    pub location_id: LocationId,
    pub checkout_id: CheckoutId,
    pub workstream_id: WorkstreamId,
    pub checkout_path: PathBuf,
    pub repository_identity: String,
    pub default_base_ref: String,
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
    pub checkout_path: PathBuf,
    pub lifecycle: WorkstreamLifecycle,
    pub last_activity_sequence: i64,
    pub revision: Revision,
    pub runtime: Option<RuntimeRecord>,
    pub binding: Option<ProviderBinding>,
    pub attention: Option<AttentionState>,
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
        migrate_host_schema(&mut connection)?;
        initialize_host_identity(&connection, id_generator)?;
        Ok(Self { connection })
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
                "SELECT canonical_profile_path, owner_id, hook_executable_path,
                    generated_content_hash, lifecycle, revision
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
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8)
             ON CONFLICT(profile_name) DO UPDATE SET
                lifecycle = excluded.lifecycle, revision = excluded.revision",
                params![
                    Uuid::new_v4().to_string(),
                    OBSERVER_PROFILE_NAME,
                    ownership.canonical_path.to_string_lossy(),
                    ownership.owner_id,
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
                hook_executable_path = ?3, generated_content_hash = ?4, lifecycle = ?5,
                revision = ?6
             WHERE profile_name = ?7 AND generated_content_hash = ?8 AND revision = ?9",
                params![
                    replacement.canonical_path.to_string_lossy(),
                    replacement.owner_id,
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
        validate_registry_text("repository identity", &repository_identity)?;
        validate_registry_text("default base ref", &default_base_ref)?;
        let registration = ExternalWorkstream {
            location_id: LocationId::new(),
            checkout_id: CheckoutId::new(),
            workstream_id: WorkstreamId::new(),
            checkout_path,
            repository_identity,
            default_base_ref,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let activity_sequence = next_activity_sequence(&transaction)?;
        let path = registration.checkout_path.to_string_lossy();
        transaction
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_identity, repository_path, default_base_ref,
                    managed_worktree_root, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                params![
                    registration.location_id.to_string(),
                    registration.repository_identity,
                    path,
                    registration.default_base_ref,
                    "",
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
                    checkout_id, lifecycle, last_activity_sequence, revision
                 ) VALUES (?1, ?2, 'external', NULL, ?3, 'open', ?4, 1)",
                params![
                    registration.workstream_id.to_string(),
                    registration.location_id.to_string(),
                    registration.checkout_id.to_string(),
                    activity_sequence,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(registration)
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
        let (checkout_path, workstream_lifecycle): (String, String) = transaction
            .query_row(
                "SELECT checkouts.path, workstreams.lifecycle FROM workstreams
                 JOIN checkouts ON checkouts.checkout_id = workstreams.checkout_id
                 WHERE workstreams.workstream_id = ?1
                   AND workstreams.lifecycle IN ('open', 'parked')",
                [workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
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
                tmux_session: format!("wsnav-{}", current.runtime_id.short()),
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
                tmux_session: format!("wsnav-{}", runtime_id.short()),
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
            touch_workstream(&transaction, &workstream_id.to_string())?;
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(record)
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

    /// Returns the bounded state needed by one local navigator snapshot.
    /// Provider content, terminal captures, and hook payloads are not queried
    /// or returned.
    ///
    /// # Errors
    ///
    /// Returns an error when a persisted identity, lifecycle, or revision is
    /// malformed, or when the registry cannot be queried.
    pub fn workstream_overviews(&self) -> Result<Vec<WorkstreamOverview>, StateError> {
        let bases = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT workstreams.workstream_id, workstreams.location_id,
                            checkouts.path, workstreams.lifecycle,
                            workstreams.last_activity_sequence, workstreams.revision
                     FROM workstreams
                     JOIN checkouts ON checkouts.checkout_id = workstreams.checkout_id
                     ORDER BY workstreams.last_activity_sequence DESC,
                              checkouts.path, workstreams.workstream_id
                     LIMIT ?1",
                )
                .map_err(StateError::Sqlite)?;
            let bases = statement
                .query_map([MAX_NAVIGATOR_WORKSTREAM_QUERY], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(StateError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StateError::Sqlite)?;
            if bases.len() > MAX_NAVIGATOR_WORKSTREAMS {
                return Err(StateError::NavigatorSnapshotTooLarge);
            }
            bases
        };

        bases
            .into_iter()
            .map(
                |(
                    workstream_id,
                    location_id,
                    checkout_path,
                    lifecycle,
                    activity_sequence,
                    revision,
                )| {
                    let workstream_id = Uuid::parse_str(&workstream_id)
                        .map(WorkstreamId::from)
                        .map_err(StateError::InvalidPersistedUuid)?;
                    let location_id = Uuid::parse_str(&location_id)
                        .map(LocationId::from)
                        .map_err(StateError::InvalidPersistedUuid)?;
                    let lifecycle = workstream_lifecycle_from_text(&lifecycle)?;
                    let revision = Revision::try_from(revision)?;
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
                        checkout_path: PathBuf::from(checkout_path),
                        lifecycle,
                        last_activity_sequence: activity_sequence,
                        revision,
                        runtime,
                        binding,
                        attention,
                    })
                },
            )
            .collect()
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let runtime = transaction
            .query_row(
                "SELECT workstream_id, tmux_generation, cwd, lifecycle, revision
                 FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
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
        touch_workstream(&transaction, &runtime.0)?;
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
        validate_project_display_name(display_name)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        ensure_local_client_host(&transaction, host, executable_path)?;
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT projects.project_id, projects.display_name
                 FROM project_locations
                 JOIN projects ON projects.project_id = project_locations.project_id
                 WHERE project_locations.host_id = ?1 AND project_locations.location_id = ?2",
                params![host.host_id.to_string(), location_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let project = if let Some((project_id, display_name)) = existing {
            ClientProjectLocation {
                project_id: Uuid::parse_str(&project_id)
                    .map(ProjectId::from)
                    .map_err(StateError::InvalidPersistedUuid)?,
                display_name,
            }
        } else {
            let project_id = ProjectId::new();
            transaction
                .execute(
                    "INSERT INTO projects (project_id, display_name, revision) VALUES (?1, ?2, 1)",
                    params![project_id.to_string(), display_name],
                )
                .map_err(StateError::Sqlite)?;
            transaction
                .execute(
                    "INSERT INTO project_locations (project_id, host_id, location_id)
                     VALUES (?1, ?2, ?3)",
                    params![
                        project_id.to_string(),
                        host.host_id.to_string(),
                        location_id.to_string()
                    ],
                )
                .map_err(StateError::Sqlite)?;
            ClientProjectLocation {
                project_id,
                display_name: display_name.to_owned(),
            }
        };
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
        if host.host_id != hello.host_id {
            return Err(StateError::ClientHostIdentityMismatch);
        }
        if host.registry_generation != hello.registry_generation {
            return Err(StateError::ClientHostGenerationMismatch);
        }
        if host.capabilities != hello.capabilities {
            return Err(StateError::ClientHostCapabilitiesMismatch);
        }
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
                "SELECT projects.project_id, projects.display_name
                 FROM project_locations
                 JOIN projects ON projects.project_id = project_locations.project_id
                 WHERE project_locations.host_id = ?1 AND project_locations.location_id = ?2",
                params![host_id.to_string(), location_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)
            .and_then(|row| {
                row.map_or(Ok(None), |(project_id, display_name)| {
                    Uuid::parse_str(&project_id)
                        .map(ProjectId::from)
                        .map(|project_id| {
                            Some(ClientProjectLocation {
                                project_id,
                                display_name,
                            })
                        })
                        .map_err(StateError::InvalidPersistedUuid)
                })
            })
    }
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

fn migrate_host_schema(connection: &mut Connection) -> Result<(), StateError> {
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
                 CHECK (last_activity_sequence >= 0);",
            )
            .map_err(StateError::Sqlite)?,
        HOST_SCHEMA_VERSION => return Ok(()),
        _ => return Err(StateError::UnsupportedSchemaVersion(current)),
    }
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
                    DEFAULT '{\"codex\":false,\"git\":false,\"tmux\":false}';",
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
    let lifecycle: String = row.get(4)?;
    let revision: i64 = row.get(5)?;
    Ok(CodexIntegration {
        ownership: ProfileOwnership {
            canonical_path: PathBuf::from(row.get::<_, String>(0)?),
            owner_id: row.get(1)?,
            hook_executable: PathBuf::from(row.get::<_, String>(2)?),
            content_hash: row.get(3)?,
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
        return update_runtime_lifecycle(
            transaction,
            context.runtime_id,
            context.runtime_revision,
            "idle",
        );
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
    update_runtime_lifecycle(
        transaction,
        context.runtime_id,
        context.runtime_revision,
        "idle",
    )
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
/// This sequence is intentionally logical rather than wall-clock time: it is
/// enough to render deterministic newest-first navigation without storing a
/// timestamp or depending on clock injection at this boundary.
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
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET last_activity_sequence = ?1, revision = revision + 1
             WHERE workstream_id = ?2",
            params![activity_sequence, workstream_id],
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

fn validate_project_display_name(value: &str) -> Result<(), StateError> {
    if value.trim().is_empty() || value.chars().count() > 128 || value.contains(['\0', '\n', '\r'])
    {
        return Err(StateError::InvalidProjectDisplayName);
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
    #[error("too many Workstreams for one bounded navigator snapshot")]
    NavigatorSnapshotTooLarge,
    #[error("client project display name is invalid")]
    InvalidProjectDisplayName,
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
    #[error("workstream {0} already has a live runtime")]
    RuntimeAlreadyLive(WorkstreamId),
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
        let recorded_version: i64 = connection
            .query_row(
                "SELECT schema_version FROM host_identity WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(activity_column, 1);
        assert_eq!(recorded_version, HOST_SCHEMA_VERSION);
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
    fn explicit_park_is_distinct_from_an_observed_runtime_stop() {
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
            .park_runtime(runtime.runtime_id, runtime.revision)
            .unwrap();
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
    }

    #[test]
    fn navigator_snapshot_fails_closed_above_its_bounded_workstream_limit() {
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

        assert!(matches!(
            registry.workstream_overviews(),
            Err(StateError::NavigatorSnapshotTooLarge)
        ));
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
