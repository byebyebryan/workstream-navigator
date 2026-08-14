use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

use crate::domain::{HostId, IdGenerator, ProviderKind};
use crate::protocol::Capabilities;

use super::client::serialize_capabilities;
use super::models::StateError;
use super::utils::provider_kind_from_text;

pub const HOST_SCHEMA_VERSION: i64 = 12;
pub(in crate::state) const CLIENT_SCHEMA_VERSION: i64 = 5;
pub(in crate::state) const MAX_NAVIGATOR_WORKSTREAMS: usize = 128;
pub(in crate::state) const MAX_NAVIGATOR_WORKSTREAM_QUERY: i64 = 129;
pub(in crate::state) const MAX_PROJECT_BROWSER_ROOT_BYTES: usize = 4096;
pub(in crate::state) const MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES: usize = 1024;

pub(in crate::state) const HOST_SCHEMA_SQL: &str = "
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
        revision INTEGER NOT NULL CHECK (revision > 0)
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
    CREATE TABLE project_browser_settings (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        root_path TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0)
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
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE INDEX compound_operations_phase_idx ON compound_operations(phase);
";

pub(in crate::state) const CLIENT_SCHEMA_SQL: &str = "
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

pub(in crate::state) fn configure_connection(connection: &Connection) -> Result<(), StateError> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
        )
        .map_err(StateError::Sqlite)
}

pub(in crate::state) fn initialize_host_identity(
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

pub(in crate::state) fn migrate_host_schema(
    connection: &mut Connection,
    _state_root: &Path,
) -> Result<(), StateError> {
    let current: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StateError::Sqlite)?;
    if current > HOST_SCHEMA_VERSION {
        return Err(StateError::UnsupportedSchemaVersion(current));
    }
    if current == HOST_SCHEMA_VERSION {
        return Ok(());
    }
    if current == 8 {
        let transaction = connection.transaction().map_err(StateError::Sqlite)?;
        transaction
            .execute_batch(
                "CREATE TABLE project_browser_settings (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    root_path TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0)
                );",
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute("PRAGMA user_version = 9", [])
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "UPDATE host_identity SET schema_version = 9 WHERE singleton = 1",
                [],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        migrate_host_schema_9_to_10(connection)?;
        migrate_host_schema_10_to_11(connection)?;
        migrate_host_schema_11_to_12(connection)?;
        return Ok(());
    }
    if current == 9 {
        migrate_host_schema_9_to_10(connection)?;
        migrate_host_schema_10_to_11(connection)?;
        return migrate_host_schema_11_to_12(connection);
    }
    if current == 10 {
        migrate_host_schema_10_to_11(connection)?;
        return migrate_host_schema_11_to_12(connection);
    }
    if current == 11 {
        return migrate_host_schema_11_to_12(connection);
    }
    if current != 0 {
        return Err(StateError::HostStateResetRequired(current));
    }
    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    transaction
        .execute_batch(HOST_SCHEMA_SQL)
        .map_err(StateError::Sqlite)?;
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

/// Adds the first-class provider identity to the schema-9 host registry.
///
/// The migration deliberately adds nullable columns without a SQL default,
/// validates every existing Runtime provider before writing any assignment,
/// then fills all legacy Workstreams and `ProviderBindings` with Codex in one
/// transaction. Any unknown Runtime provider or cross-record mismatch aborts
/// the transaction, leaving the schema and all rows at version 9.
pub(in crate::state) fn migrate_host_schema_9_to_10(
    connection: &mut Connection,
) -> Result<(), StateError> {
    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    if !table_has_column(&transaction, "workstreams", "provider")? {
        transaction
            .execute("ALTER TABLE workstreams ADD COLUMN provider TEXT", [])
            .map_err(StateError::Sqlite)?;
    }
    if !table_has_column(&transaction, "provider_bindings", "provider")? {
        transaction
            .execute("ALTER TABLE provider_bindings ADD COLUMN provider TEXT", [])
            .map_err(StateError::Sqlite)?;
    }
    if !table_has_column(
        &transaction,
        "attention_states",
        "latest_native_session_provider",
    )? {
        transaction
            .execute(
                "ALTER TABLE attention_states ADD COLUMN latest_native_session_provider TEXT",
                [],
            )
            .map_err(StateError::Sqlite)?;
    }

    let mut runtime_providers = transaction
        .prepare("SELECT provider FROM runtimes")
        .map_err(StateError::Sqlite)?;
    let providers = runtime_providers
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    for provider in providers {
        let parsed = provider_kind_from_text(&provider)?;
        if parsed != ProviderKind::Codex {
            return Err(StateError::ProviderIdentityMismatch);
        }
    }
    drop(runtime_providers);

    transaction
        .execute(
            "UPDATE workstreams SET provider = ?1 WHERE provider IS NULL",
            [ProviderKind::Codex.as_str()],
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE provider_bindings SET provider = ?1 WHERE provider IS NULL",
            [ProviderKind::Codex.as_str()],
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE attention_states SET latest_native_session_provider = ?1
             WHERE latest_native_session_id IS NOT NULL
               AND latest_native_session_provider IS NULL",
            [ProviderKind::Codex.as_str()],
        )
        .map_err(StateError::Sqlite)?;

    let mismatch: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM runtimes
             JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
             WHERE runtimes.provider != workstreams.provider
                OR runtimes.provider != 'codex'",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if mismatch != 0 {
        return Err(StateError::ProviderIdentityMismatch);
    }
    let binding_mismatch: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM provider_bindings
             JOIN runtimes ON runtimes.runtime_id = provider_bindings.runtime_id
             WHERE provider_bindings.provider != runtimes.provider",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if binding_mismatch != 0 {
        return Err(StateError::ProviderIdentityMismatch);
    }

    transaction
        .execute("PRAGMA user_version = 10", [])
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE host_identity SET schema_version = 10 WHERE singleton = 1",
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

pub(in crate::state) fn migrate_host_schema_10_to_11(
    connection: &mut Connection,
) -> Result<(), StateError> {
    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    let existing: Option<String> = transaction
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'opencode_runtime_handles'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    if existing.is_some() {
        // Schema 10 predates this table.  Any pre-existing table is therefore
        // ambiguous (including one that happens to have the same columns) and
        // must abort the transaction without advancing user_version.
        return Err(StateError::InvalidPersistedValue(
            "preexisting OpenCode handle table".to_owned(),
        ));
    }
    transaction
        .execute_batch(
            "CREATE TABLE opencode_runtime_handles (
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
            );",
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute("PRAGMA user_version = 11", [])
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE host_identity SET schema_version = 11 WHERE singleton = 1",
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

pub(in crate::state) fn migrate_host_schema_11_to_12(
    connection: &mut Connection,
) -> Result<(), StateError> {
    let transaction = connection.transaction().map_err(StateError::Sqlite)?;
    if let Some(existing_type) = table_column_type(&transaction, "runtimes", "provider_pid")? {
        if !existing_type.eq_ignore_ascii_case("INTEGER") {
            return Err(StateError::InvalidPersistedValue(
                "Runtime provider PID column type".to_owned(),
            ));
        }
    } else {
        transaction
            .execute("ALTER TABLE runtimes ADD COLUMN provider_pid INTEGER", [])
            .map_err(StateError::Sqlite)?;
    }
    transaction
        .execute("PRAGMA user_version = 12", [])
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE host_identity SET schema_version = 12 WHERE singleton = 1",
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

pub(in crate::state) fn table_has_column(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
) -> Result<bool, StateError> {
    let mut statement = transaction
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    Ok(columns.iter().any(|value| value == column))
}

pub(in crate::state) fn table_column_type(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
) -> Result<Option<String>, StateError> {
    let mut statement = transaction
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let name: String = row.get(1).map_err(StateError::Sqlite)?;
        if name == column {
            return row
                .get::<_, String>(2)
                .map(Some)
                .map_err(StateError::Sqlite);
        }
    }
    Ok(None)
}

pub(in crate::state) fn migrate_client_schema(
    connection: &mut Connection,
) -> Result<(), StateError> {
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
                    DEFAULT '{\"git\":false,\"tmux\":false}';
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
        4 => {}
        CLIENT_SCHEMA_VERSION => return Ok(()),
        _ => return Err(StateError::UnsupportedSchemaVersion(current)),
    }
    migrate_client_capabilities_4_to_5(&transaction)?;
    transaction
        .execute(
            &format!("PRAGMA user_version = {CLIENT_SCHEMA_VERSION}"),
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

pub(in crate::state) fn migrate_client_capabilities_4_to_5(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StateError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyCapabilities {
        #[serde(rename = "codex")]
        _codex: bool,
        git: bool,
        tmux: bool,
    }

    let legacy = {
        let mut statement = transaction
            .prepare("SELECT host_alias, capabilities_json FROM hosts")
            .map_err(StateError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?
    };
    for (alias, capabilities_json) in legacy {
        let capabilities = match serde_json::from_str::<LegacyCapabilities>(&capabilities_json) {
            Ok(legacy) => Capabilities {
                git: legacy.git,
                tmux: legacy.tmux,
            },
            Err(_) => serde_json::from_str::<Capabilities>(&capabilities_json)
                .map_err(|_| StateError::InvalidPersistedCapabilities)?,
        };
        transaction
            .execute(
                "UPDATE hosts SET capabilities_json = ?1 WHERE host_alias = ?2",
                params![serialize_capabilities(&capabilities)?, alias],
            )
            .map_err(StateError::Sqlite)?;
    }
    Ok(())
}
