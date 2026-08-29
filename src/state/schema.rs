/// The active public production state boundary. Schema 13 remains available
/// only as the explicit D17 migration source; schema 12 remains the exact
/// fixture consumed by the D16 cutover bridge.
pub const HOST_SCHEMA_VERSION: i64 = 14;
pub(in crate::state) const MAX_NAVIGATOR_WORKSTREAMS: usize = 128;
pub(in crate::state) const MAX_PROJECT_BROWSER_ROOT_BYTES: usize = 4096;
pub(in crate::state) const MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES: usize = 1024;

/// Exact schema-12 host SQL retained for the explicit D16 fixture and
/// 12-to-13 migration. No production open path executes this SQL directly.
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
