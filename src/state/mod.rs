use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AttentionState, CheckoutId, CompoundOperation, DomainError, HostId, IdGenerator, LocationId,
    OperationId, OperationKind, OperationPhase, RandomIdGenerator, Revision, RuntimeId,
    RuntimeStatus, WorkstreamId,
};
use crate::provider::codex::hooks::{HookObservation, LifecycleEvent};
use crate::provider::codex::profile::{OBSERVER_PROFILE_NAME, ProfileOwnership};

const HOST_SCHEMA_VERSION: i64 = 1;
const CLIENT_SCHEMA_VERSION: i64 = 1;

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
        executable_path TEXT NOT NULL,
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
    pub status: RuntimeStatus,
    pub revision: Revision,
}

/// The exact native Codex session currently bound to a managed runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderBinding {
    pub runtime_id: RuntimeId,
    pub native_session_id: String,
    pub last_settled_turn_id: Option<String>,
    pub revision: Revision,
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
                    checkout_id, lifecycle, revision
                 ) VALUES (?1, ?2, 'external', NULL, ?3, 'open', 1)",
                params![
                    registration.workstream_id.to_string(),
                    registration.location_id.to_string(),
                    registration.checkout_id.to_string(),
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
        let checkout_path: String = transaction
            .query_row(
                "SELECT checkouts.path FROM workstreams
                 JOIN checkouts ON checkouts.checkout_id = workstreams.checkout_id
                 WHERE workstreams.workstream_id = ?1 AND workstreams.lifecycle = 'open'",
                [workstream_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
        let current: Option<RuntimeRecord> = transaction
            .query_row(
                "SELECT runtime_id, tmux_generation, tmux_session, cwd, lifecycle, revision
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
                "SELECT runtime_id, tmux_generation, tmux_session, cwd, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)
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
        validate_registry_text("thread name", name)?;
        let changed = self
            .connection
            .execute(
                "UPDATE provider_bindings SET observed_thread_name = ?1, name_state = 'named',
             revision = revision + 1 WHERE runtime_id = ?2 AND native_session_id = ?3",
                params![name, runtime_id.to_string(), native_session_id],
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

    /// Applies one already-authorized lifecycle observation to its exact runtime.
    ///
    /// Hooks supply evidence only: a new session can bind solely while the
    /// runtime is `starting`; subsequent observations must match that exact
    /// binding and generation. A settled result and its sticky attention state
    /// commit in the same `SQLite` transaction.
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
            LifecycleEvent::SessionStart => {
                if let Some(binding) = existing {
                    if binding.native_session_id != observation.native_session_id {
                        return Err(StateError::HookEvidenceMismatch);
                    }
                } else if runtime.3 != "starting" {
                    return Err(StateError::HookEvidenceMismatch);
                } else {
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
                                runtime_id.to_string(),
                                observation.native_session_id,
                                observation.source.unwrap_or_else(|| "startup".to_owned()),
                                generation
                            ],
                        )
                        .map_err(StateError::Sqlite)?;
                }
                update_runtime_lifecycle(&transaction, runtime_id, revision, "idle")?;
            }
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
    if current == HOST_SCHEMA_VERSION {
        return Ok(());
    }

    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    transaction
        .execute_batch(HOST_SCHEMA_SQL)
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(&format!("PRAGMA user_version = {HOST_SCHEMA_VERSION}"), [])
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
    if current == CLIENT_SCHEMA_VERSION {
        return Ok(());
    }

    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    transaction
        .execute_batch(CLIENT_SCHEMA_SQL)
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            &format!("PRAGMA user_version = {CLIENT_SCHEMA_VERSION}"),
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
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
    let lifecycle: String = row.get(4)?;
    let revision: i64 = row.get(5)?;
    Ok(RuntimeRecord {
        runtime_id: Uuid::parse_str(&runtime_id)
            .map(RuntimeId::from)
            .map_err(to_from_sql_error)?,
        workstream_id,
        tmux_generation: generation,
        tmux_session: session,
        cwd: PathBuf::from(cwd),
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
            "SELECT native_session_id, last_settled_turn_id, revision
             FROM provider_bindings WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| {
                Ok(ProviderBinding {
                    runtime_id,
                    native_session_id: row.get(0)?,
                    last_settled_turn_id: row.get(1)?,
                    revision: Revision::try_from(row.get::<_, i64>(2)?)
                        .map_err(to_from_sql_error)?,
                })
            },
        )
        .optional()
        .map_err(StateError::Sqlite)
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
}
