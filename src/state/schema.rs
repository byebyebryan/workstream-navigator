use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use super::models::StateError;

/// The only current host schema accepted by the current state boundary.
pub const HOST_SCHEMA_VERSION: i64 = 15;
/// Fixed `SQLite` application identifier for a Workstream Navigator host file.
/// `WSNV` is intentionally independent of the schema number so a raw header
/// read can reject foreign `SQLite` files before opening/recovering `SQLite`.
pub const HOST_APPLICATION_ID: u32 = 0x5753_4e56;
pub(in crate::state) const MAX_NAVIGATOR_WORKSTREAMS: usize = 128;

pub(super) fn validate_foreign_keys(connection: &Connection) -> Result<(), StateError> {
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(StateError::Sqlite)?;
    if foreign_keys
        .query([])
        .map_err(StateError::Sqlite)?
        .next()
        .map_err(StateError::Sqlite)?
        .is_some()
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

pub(super) fn validate_host_identity(
    connection: &Connection,
    version: i64,
) -> Result<(), StateError> {
    let row: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT host_id, registry_generation, schema_version
             FROM host_identity WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((host_id, generation, recorded_version)) = row else {
        return Err(StateError::MalformedHostSchema);
    };
    if Uuid::parse_str(&host_id).is_err()
        || generation.is_empty()
        || generation.len() > 256
        || generation.contains(['\n', '\r'])
        || recorded_version != version
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

pub(super) fn validate_table_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
) -> Result<(), StateError> {
    if table == "sqlite_master" {
        return Ok(());
    }
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    if required
        .iter()
        .any(|column| !columns.iter().any(|value| value == column))
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

pub(super) fn table_exists(connection: &Connection, table: &str) -> Result<bool, StateError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

pub(super) fn table_has_column_readonly(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, StateError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?
        .iter()
        .any(|value| value == column))
}

/// The sole authoritative current schema.  This definition is deliberately
/// self-contained: a new current root is never created by replaying the retired
/// schema-12/13/14 SQL or by running a migration.  Keep this statement as a
/// single direct definition so the raw header identity and the durable shape
/// are published together at the bootstrap boundary.
pub(in crate::state) const HOST_SCHEMA_15_SQL: &str = "
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
        repository_path TEXT NOT NULL,
        repository_display_name TEXT NOT NULL,
        remote_identity_fingerprint TEXT,
        remote_identity_display TEXT,
        revision INTEGER NOT NULL CHECK (revision > 0),
        project_id TEXT REFERENCES projects(project_id)
    );
    CREATE TABLE workstreams (
        workstream_id TEXT PRIMARY KEY,
        location_id TEXT NOT NULL REFERENCES project_locations(location_id),
        provider TEXT NOT NULL,
        origin TEXT NOT NULL,
        source_workstream_id TEXT REFERENCES workstreams(workstream_id),
        lifecycle TEXT NOT NULL,
        archived_at_millis INTEGER,
        last_activity_sequence INTEGER NOT NULL CHECK (last_activity_sequence >= 0),
        last_activity_at_millis INTEGER NOT NULL CHECK (last_activity_at_millis >= 0),
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE independent_creation_requests (
        request_key TEXT PRIMARY KEY,
        source_workstream_id TEXT NOT NULL REFERENCES workstreams(workstream_id),
        source_revision INTEGER NOT NULL CHECK (source_revision > 0),
        workstream_id TEXT NOT NULL UNIQUE REFERENCES workstreams(workstream_id)
    );
    CREATE TABLE runtimes (
        runtime_id TEXT PRIMARY KEY,
        workstream_id TEXT NOT NULL UNIQUE REFERENCES workstreams(workstream_id),
        provider TEXT NOT NULL,
        tmux_generation TEXT NOT NULL,
        tmux_session TEXT NOT NULL,
        cwd TEXT NOT NULL,
        provider_pid INTEGER,
        process_birth TEXT,
        lifecycle TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE TABLE opencode_runtime_handles (
        runtime_id TEXT PRIMARY KEY REFERENCES runtimes(runtime_id),
        runtime_generation TEXT NOT NULL,
        endpoint_host TEXT NOT NULL CHECK (endpoint_host = '127.0.0.1'),
        endpoint_port INTEGER NOT NULL CHECK (endpoint_port BETWEEN 1 AND 65535),
        version TEXT NOT NULL,
        native_session_id TEXT NOT NULL,
        observer_pid INTEGER,
        observer_birth TEXT,
        observer_status TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0),
        UNIQUE(runtime_id, runtime_generation)
    );
    CREATE TABLE provider_bindings (
        binding_id TEXT PRIMARY KEY,
        runtime_id TEXT NOT NULL UNIQUE REFERENCES runtimes(runtime_id),
        provider TEXT NOT NULL,
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
        latest_native_session_provider TEXT,
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
        revision INTEGER NOT NULL CHECK (revision > 0),
        launch_token_id TEXT,
        launch_token_verifier TEXT,
        launch_token_expiry_monotonic INTEGER,
        launch_claims_digest TEXT
    );
    CREATE INDEX compound_operations_phase_idx ON compound_operations(phase);
    CREATE UNIQUE INDEX compound_operations_launch_token_id_idx
        ON compound_operations(launch_token_id)
        WHERE launch_token_id IS NOT NULL;
    CREATE TABLE projects (
        project_id TEXT PRIMARY KEY,
        label_location_id TEXT NOT NULL REFERENCES project_locations(location_id),
        display_name TEXT NOT NULL,
        repository_fingerprint TEXT,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE UNIQUE INDEX project_repository_fingerprint_idx
        ON projects(repository_fingerprint)
        WHERE repository_fingerprint IS NOT NULL AND repository_fingerprint != '';
    CREATE TABLE opencode_settled_messages (
        settled_message_id INTEGER PRIMARY KEY AUTOINCREMENT,
        runtime_id TEXT NOT NULL REFERENCES runtimes(runtime_id),
        runtime_generation TEXT NOT NULL,
        native_session_id TEXT NOT NULL,
        message_id TEXT NOT NULL,
        UNIQUE(runtime_id, runtime_generation, native_session_id, message_id)
    );
    CREATE INDEX opencode_settled_messages_runtime_idx
        ON opencode_settled_messages(runtime_id, runtime_generation,
                                      native_session_id, settled_message_id);
    CREATE TABLE host_operational_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        bootstrap_host_id TEXT NOT NULL,
        bootstrap_generation TEXT NOT NULL,
        provisional_lease_generation INTEGER NOT NULL CHECK (provisional_lease_generation > 0),
        provisional_lock_phase TEXT NOT NULL
            CHECK (provisional_lock_phase IN ('pending', 'ready')),
        provisional_lock_device INTEGER,
        provisional_lock_inode INTEGER,
        CHECK (
            (provisional_lock_phase = 'pending'
                AND provisional_lock_device IS NULL
                AND provisional_lock_inode IS NULL)
            OR
            (provisional_lock_phase = 'ready'
                AND provisional_lock_device IS NOT NULL
                AND provisional_lock_inode IS NOT NULL)
        )
    );
    CREATE TABLE onboarding_exec_targets (
        operation_id TEXT PRIMARY KEY REFERENCES compound_operations(operation_id),
        provider TEXT NOT NULL CHECK (provider IN ('codex', 'opencode')),
        executable_device INTEGER NOT NULL CHECK (executable_device >= 0),
        executable_inode INTEGER NOT NULL CHECK (executable_inode > 0)
    );
";
