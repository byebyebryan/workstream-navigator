use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::{
    AttentionState, CompoundOperation, DomainError, HostId, IdGenerator, OperationId,
    OperationKind, OperationPhase, RandomIdGenerator, Revision, WorkstreamId,
};

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
}
