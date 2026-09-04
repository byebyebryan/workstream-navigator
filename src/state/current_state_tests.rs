use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use rusqlite::{Connection, params};
use serde_json::Value;
use tempfile::tempdir;
use uuid::Uuid;

use crate::domain::ProviderKind;
use crate::domain::{IdGenerator, RandomIdGenerator, WorkstreamId};
use crate::provider::lifecycle::{LifecycleEvent, LifecycleObservation};

use super::current::{BootstrapCheckpoint, create_current_with_checkpoint_hook};
use super::{
    BOOTSTRAP_LOCK_FILE, HOST_APPLICATION_ID, HOST_SCHEMA_VERSION, HostRegistry, ProviderBinding,
    RuntimeRecord, StateError, StateRoot, create_current, open_current,
};

fn raw_identity(path: &std::path::Path) -> (u32, u32) {
    let bytes = fs::read(path).expect("database bytes");
    assert!(bytes.len() >= 72);
    (
        u32::from_be_bytes(bytes[60..64].try_into().unwrap()),
        u32::from_be_bytes(bytes[68..72].try_into().unwrap()),
    )
}

struct CodexLifecycleFixture {
    _temporary: tempfile::TempDir,
    path: PathBuf,
    workstream_id: WorkstreamId,
    cwd: String,
}

type LegacyAttentionRow = (
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

fn codex_lifecycle_fixture() -> CodexLifecycleFixture {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let mut state = create_current(&path, &RandomIdGenerator).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &checkout,
            "checkout",
            ProviderKind::Codex,
            &RandomIdGenerator,
        )
        .unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "native-thread".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    drop(registry);
    CodexLifecycleFixture {
        _temporary: temporary,
        path,
        workstream_id,
        cwd,
    }
}

fn observe_codex(
    registry: &mut HostRegistry,
    fixture: &CodexLifecycleFixture,
    runtime: &RuntimeRecord,
    event: LifecycleEvent,
    turn_id: Option<String>,
    source: Option<&str>,
) -> Result<(), StateError> {
    registry.apply_lifecycle_observation(
        runtime.runtime_id,
        &runtime.tmux_generation,
        LifecycleObservation {
            event,
            cwd: fixture.cwd.clone(),
            native_session_id: "native-thread".to_owned(),
            turn_id,
            source: source.map(str::to_owned),
        },
    )
}

fn insert_legacy_attention(path: &Path, workstream_id: WorkstreamId) -> LegacyAttentionRow {
    let legacy = (
        Some(17_i64),
        Some(18_i64),
        Some("legacy-session".to_owned()),
        Some("codex".to_owned()),
        Some("legacy-turn".to_owned()),
        19_i64,
    );
    let connection = Connection::open(path.join("host.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO attention_states (
                workstream_id, result_unseen_since_revision,
                recovery_unseen_since_revision, latest_native_session_id,
                latest_native_session_provider, latest_turn_id, revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                workstream_id.to_string(),
                legacy.0,
                legacy.1,
                legacy.2,
                legacy.3,
                legacy.4,
                legacy.5,
            ],
        )
        .unwrap();
    legacy
}

fn read_legacy_attention(path: &Path, workstream_id: WorkstreamId) -> LegacyAttentionRow {
    let connection = Connection::open(path.join("host.sqlite")).unwrap();
    connection
        .query_row(
            "SELECT result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_native_session_provider, latest_turn_id, revision
             FROM attention_states WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .unwrap()
}

#[test]
fn direct_schema15_bootstrap_publishes_current_identity_and_lock() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let state = create_current(&path, &RandomIdGenerator).expect("fresh current schema-15 state");
    assert_eq!(state.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    drop(state);
    assert_eq!(
        raw_identity(&path.join("host.sqlite")),
        (15, HOST_APPLICATION_ID)
    );
    let connection = Connection::open(path.join("host.sqlite")).unwrap();
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "delete");
    drop(connection);
    assert!(path.join(BOOTSTRAP_LOCK_FILE).is_file());
    assert!(path.join("provisional.lock").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(path.join(BOOTSTRAP_LOCK_FILE)).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.join("provisional.lock")).unwrap().mode() & 0o777,
            0o600
        );
    }
    assert!(!fs::read_dir(&path).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("bootstrap-")
    }));
    let reopened = open_current(&StateRoot::select(&path)).expect("reopen current schema-15 state");
    assert_eq!(reopened.schema_version().unwrap(), 15);
}

#[test]
fn direct_schema15_has_semantic_exec_target_table_only() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let connection = Connection::open(path.join("host.sqlite")).unwrap();
    let names = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(names.iter().any(|name| name == "onboarding_exec_targets"));
    assert!(
        !names
            .iter()
            .any(|name| name == "d17_onboarding_exec_targets")
    );
    assert!(!names.iter().any(|name| name == "project_browser_settings"));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the fixture enumerates the complete owned graph and refusal invariants"
)]
fn forgetting_archived_workstream_removes_only_its_owned_graph() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let mut state = create_current(&path, &RandomIdGenerator).unwrap();
    let (selected_location, selected_id) = state
        .seed_test_workstream(
            &checkout,
            "selected",
            ProviderKind::OpenCode,
            &RandomIdGenerator,
        )
        .unwrap();
    let child_checkout = temporary.path().join("child");
    fs::create_dir(&child_checkout).unwrap();
    let (child_location, child_id) = state
        .seed_test_workstream(
            &child_checkout,
            "child",
            ProviderKind::OpenCode,
            &RandomIdGenerator,
        )
        .unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let runtime = registry.reserve_runtime(selected_id).unwrap();
    registry
        .mark_runtime_stopped(runtime.runtime_id, runtime.revision)
        .unwrap();
    let runtime = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    registry
        .park_runtime(runtime.runtime_id, runtime.revision)
        .unwrap();
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == selected_id)
        .unwrap();
    let archived_revision = registry
        .archive_workstream(selected_id, overview.revision, 42)
        .unwrap();
    let runtime_generation = runtime.tmux_generation;
    let operation_id = Uuid::from_u128(0xD240).to_string();
    let database = path.join("host.sqlite");
    let connection = Connection::open(&database).unwrap();
    let selected_project_id: String = connection
        .query_row(
            "SELECT project_id FROM project_locations WHERE location_id = ?1",
            [selected_location.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "UPDATE workstreams SET source_workstream_id = ?1
             WHERE workstream_id = ?2",
            params![selected_id.to_string(), child_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO independent_creation_requests (
                request_key, source_workstream_id, source_revision, workstream_id
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                "forget-child-request",
                selected_id.to_string(),
                archived_revision.value(),
                child_id.to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_bindings (
                binding_id, runtime_id, provider, native_session_id, start_source,
                last_settled_turn_id, observed_thread_name, name_state,
                name_observed_at, predecessor_native_session_id,
                predecessor_effective_name, runtime_generation, revision
             ) VALUES (?1, ?2, 'opencode', 'native-session', 'startup', NULL, NULL,
                       'unavailable', NULL, NULL, NULL, ?3, 1)",
            params![
                Uuid::from_u128(0xD241).to_string(),
                runtime.runtime_id.to_string(),
                runtime_generation,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO opencode_runtime_handles (
                runtime_id, runtime_generation, endpoint_host, endpoint_port,
                version, native_session_id, observer_pid, observer_birth,
                observer_status, revision
             ) VALUES (?1, ?2, '127.0.0.1', 43123, 'test', 'native-session',
                       NULL, NULL, 'stopped', 1)",
            params![runtime.runtime_id.to_string(), runtime_generation],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO opencode_settled_messages (
                runtime_id, runtime_generation, native_session_id, message_id
             ) VALUES (?1, ?2, 'native-session', 'message-1')",
            params![runtime.runtime_id.to_string(), runtime_generation],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO attention_states (
                workstream_id, result_unseen_since_revision,
                recovery_unseen_since_revision, latest_native_session_id,
                latest_native_session_provider, latest_turn_id, revision
             ) VALUES (?1, 1, NULL, 'native-session', 'opencode', 'turn-1', 1)",
            [selected_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO compound_operations (
                operation_id, request_key, kind, phase, expected_revisions_json,
                effect_watermark, outcome_json, revision
             ) VALUES (?1, 'forget-onboard', 'onboard', 'provider_exec_proven', ?2, NULL, NULL, 1)",
            params![
                operation_id,
                serde_json::json!({
                    "workstream_id": selected_id,
                    "candidate_runtime_id": runtime.runtime_id,
                })
                .to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO onboarding_exec_targets (
                operation_id, provider, executable_device, executable_inode
             ) VALUES (?1, 'opencode', 1, 1)",
            [&operation_id],
        )
        .unwrap();
    let retired_fork_id = Uuid::from_u128(0xD242).to_string();
    connection
        .execute(
            "INSERT INTO compound_operations (
                operation_id, request_key, kind, phase, expected_revisions_json,
                effect_watermark, outcome_json, revision
             ) VALUES (?1, 'retired-fork-history', 'fork', 'committed', '{}',
                       'legacy-effect', NULL, 1)",
            [&retired_fork_id],
        )
        .unwrap();

    assert!(matches!(
        registry.forget_workstream(selected_id, archived_revision.next()),
        Err(StateError::Domain(_))
    ));
    let selected_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM workstreams WHERE workstream_id = ?1",
            [selected_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(selected_count, 1);
    connection
        .execute(
            "UPDATE compound_operations SET phase = 'external_effect_started'
             WHERE operation_id = ?1",
            [&operation_id],
        )
        .unwrap();
    assert!(matches!(
        registry.forget_workstream(selected_id, archived_revision),
        Err(StateError::WorkstreamForgetRefused)
    ));
    connection
        .execute(
            "UPDATE compound_operations SET phase = 'provider_exec_proven'
             WHERE operation_id = ?1",
            [&operation_id],
        )
        .unwrap();
    drop(connection);

    registry
        .forget_workstream(selected_id, archived_revision)
        .unwrap();
    drop(registry);

    let connection = Connection::open(&database).unwrap();
    for table in [
        "runtimes",
        "provider_bindings",
        "opencode_runtime_handles",
        "opencode_settled_messages",
        "attention_states",
        "independent_creation_requests",
        "onboarding_exec_targets",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} should be deleted for the selected graph");
    }
    let selected_operation_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM compound_operations WHERE operation_id = ?1",
            [operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(selected_operation_count, 0);
    let retired_fork_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM compound_operations WHERE operation_id = ?1",
            [&retired_fork_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired_fork_count, 1);
    let selected_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM workstreams WHERE workstream_id = ?1",
            [selected_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(selected_count, 0);
    let child_source: Option<String> = connection
        .query_row(
            "SELECT source_workstream_id FROM workstreams WHERE workstream_id = ?1",
            [child_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(child_source, None);
    let child_location_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE location_id = ?1",
            [child_location.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(child_location_count, 1);
    let selected_location_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE location_id = ?1",
            [selected_location.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(selected_location_count, 1);
    let selected_project_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE project_id = ?1",
            [&selected_project_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(selected_project_count, 1);
    let workstream_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM workstreams", [], |row| row.get(0))
        .unwrap();
    assert_eq!(workstream_count, 1);
}

#[test]
fn current_open_refuses_extra_schema_objects() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let database = path.join("host.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch("ALTER TABLE host_identity ADD COLUMN retired_shape TEXT;")
        .unwrap();
    drop(connection);
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::MalformedHostSchema));
}

#[test]
fn current_open_refuses_old_header_without_sqlite_open() {
    for version in [12_i64, 13, 14] {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        drop(create_current(&path, &RandomIdGenerator).unwrap());
        let database = path.join("host.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA application_id = 0x57534e56; PRAGMA user_version = {version};"
            ))
            .unwrap();
        drop(connection);
        let before = fs::read(&database).unwrap();
        #[cfg(unix)]
        let inode_before = {
            let metadata = fs::metadata(&database).unwrap();
            (metadata.dev(), metadata.ino())
        };
        let error = open_current(&StateRoot::select(&path)).unwrap_err();
        assert!(matches!(
            error,
            StateError::BootstrapArtifactMismatch
                | StateError::MalformedHostSchema
                | StateError::HostStateResetRequired(_)
        ));
        let after = fs::read(&database).unwrap();
        assert_eq!(before, after);
        #[cfg(unix)]
        {
            let metadata = fs::metadata(&database).unwrap();
            assert_eq!((metadata.dev(), metadata.ino()), inode_before);
        }
        assert!(path.join(BOOTSTRAP_LOCK_FILE).is_file());
        assert!(path.join("provisional.lock").is_file());
    }
}

#[test]
fn current_open_refuses_unresolved_retired_fork_without_mutation() {
    for (index, phase) in [
        "external_effect_started",
        "awaiting_reconciliation",
        "recovery_required",
    ]
    .into_iter()
    .enumerate()
    {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        drop(create_current(&path, &RandomIdGenerator).unwrap());
        let operation_id = Uuid::from_u128(0x5000 + index as u128).to_string();
        let database = path.join("host.sqlite");
        {
            let connection = Connection::open(&database).unwrap();
            connection
                .execute(
                    "INSERT INTO compound_operations (
                        operation_id, request_key, kind, phase,
                        expected_revisions_json, effect_watermark, outcome_json, revision
                     ) VALUES (?1, ?2, 'fork', ?3, '{}', 'legacy-effect', NULL, 1)",
                    params![operation_id, format!("legacy-fork-{index}"), phase],
                )
                .unwrap();
        }
        let before_bytes = fs::read(&database).unwrap();
        let before_rows = {
            let connection = Connection::open(&database).unwrap();
            connection
                .prepare(
                    "SELECT operation_id, request_key, kind, phase,
                            expected_revisions_json, effect_watermark,
                            outcome_json, revision
                     FROM compound_operations ORDER BY operation_id",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        let error = open_current(&StateRoot::select(&path)).unwrap_err();
        assert!(matches!(error, StateError::RetiredForkRecoveryRequired));
        assert!(error.to_string().contains("previous accepted build"));
        assert_eq!(before_bytes, fs::read(&database).unwrap());
        let after_rows = {
            let connection = Connection::open(&database).unwrap();
            connection
                .prepare(
                    "SELECT operation_id, request_key, kind, phase,
                            expected_revisions_json, effect_watermark,
                            outcome_json, revision
                     FROM compound_operations ORDER BY operation_id",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(before_rows, after_rows);
    }
}

#[test]
fn current_open_keeps_completed_fork_history_and_fork_origin_inert() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut state = create_current(&path, &RandomIdGenerator).unwrap();
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &checkout,
            "checkout",
            ProviderKind::Codex,
            &RandomIdGenerator,
        )
        .unwrap();
    drop(state);

    let database = path.join("host.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE workstreams SET origin = 'fork' WHERE workstream_id = ?1",
            [workstream_id.to_string()],
        )
        .unwrap();
    for (index, phase) in ["committed", "failed"].into_iter().enumerate() {
        connection
            .execute(
                "INSERT INTO compound_operations (
                    operation_id, request_key, kind, phase,
                    expected_revisions_json, effect_watermark, outcome_json, revision
                 ) VALUES (?1, ?2, 'fork', ?3, '{}', 'legacy-effect', NULL, 1)",
                params![
                    Uuid::from_u128(0x6000 + index as u128).to_string(),
                    format!("completed-legacy-fork-{index}"),
                    phase,
                ],
            )
            .unwrap();
    }
    drop(connection);

    let state = open_current(&StateRoot::select(&path)).unwrap();
    let registry = state.into_host_registry().unwrap();
    assert_eq!(registry.workstream_overviews().unwrap().len(), 1);
    assert!(
        registry
            .unresolved_operation_overviews()
            .unwrap()
            .is_empty()
    );
    let connection = Connection::open(&database).unwrap();
    let origin: String = connection
        .query_row(
            "SELECT origin FROM workstreams WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(origin, "fork");
    let fork_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM compound_operations WHERE kind = 'fork'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fork_rows, 2);
}

#[test]
fn native_session_rotation_reuses_one_workstream_and_rotates_its_binding() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut state = create_current(&path, &RandomIdGenerator).unwrap();
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &checkout,
            "checkout",
            ProviderKind::Codex,
            &RandomIdGenerator,
        )
        .unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();

    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "native-thread-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd,
                native_session_id: "native-thread-b".to_owned(),
                turn_id: None,
                source: Some("clear".to_owned()),
            },
        )
        .unwrap();

    let workstreams = registry.workstream_overviews().unwrap();
    assert_eq!(workstreams.len(), 1);
    assert_eq!(workstreams[0].workstream_id, workstream_id);
    let runtime_after = registry
        .runtime_for_workstream(workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime_after.runtime_id, runtime.runtime_id);
    assert_eq!(runtime_after.tmux_generation, runtime.tmux_generation);
    let binding = registry
        .binding_for_runtime(runtime.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(binding.native_session_id.native_id(), "native-thread-b");
    assert_eq!(binding.start_source, "clear");
    assert_eq!(
        binding
            .predecessor_native_session_id
            .as_ref()
            .map(crate::domain::ProviderSessionId::native_id),
        Some("native-thread-a")
    );
}

#[test]
fn legacy_attention_rows_are_ignored_by_snapshots_and_lifecycle_observations() {
    let fixture = codex_lifecycle_fixture();
    let legacy = insert_legacy_attention(&fixture.path, fixture.workstream_id);

    let root = StateRoot::select(&fixture.path);
    let initial = crate::snapshot::read_snapshot(&root).unwrap();
    assert_eq!(
        initial.workstreams[0]
            .runtime
            .as_ref()
            .map(|runtime| runtime.status),
        Some(crate::domain::RuntimeStatus::Idle)
    );

    let state = open_current(&root).unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let runtime = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    observe_codex(
        &mut registry,
        &fixture,
        &runtime,
        LifecycleEvent::Stop,
        Some("observed-turn".to_owned()),
        None,
    )
    .unwrap();
    let runtime = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.status, crate::domain::RuntimeStatus::Attention);
    observe_codex(
        &mut registry,
        &fixture,
        &runtime,
        LifecycleEvent::UserPromptSubmit,
        None,
        None,
    )
    .unwrap();
    let runtime = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.status, crate::domain::RuntimeStatus::Working);
    registry
        .mark_runtime_recovery_required(runtime.runtime_id, runtime.revision)
        .unwrap();
    drop(registry);

    let after = crate::snapshot::read_snapshot(&root).unwrap();
    assert_eq!(
        after.workstreams[0].lifecycle,
        crate::domain::WorkstreamLifecycle::RecoveryRequired
    );
    assert_eq!(
        after.workstreams[0]
            .runtime
            .as_ref()
            .map(|runtime| runtime.status),
        Some(crate::domain::RuntimeStatus::Unknown)
    );

    assert_eq!(
        read_legacy_attention(&fixture.path, fixture.workstream_id),
        legacy
    );
}

#[test]
fn malformed_codex_stop_turn_id_preserves_all_durable_state() {
    let fixture = codex_lifecycle_fixture();
    insert_legacy_attention(&fixture.path, fixture.workstream_id);
    let root = StateRoot::select(&fixture.path);
    let state = open_current(&root).unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let runtime = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    observe_codex(
        &mut registry,
        &fixture,
        &runtime,
        LifecycleEvent::Stop,
        Some("prior-turn".to_owned()),
        None,
    )
    .unwrap();
    let runtime = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    observe_codex(
        &mut registry,
        &fixture,
        &runtime,
        LifecycleEvent::UserPromptSubmit,
        None,
        None,
    )
    .unwrap();

    let runtime_before = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime_before.status, crate::domain::RuntimeStatus::Working);
    let overview_before = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == fixture.workstream_id)
        .unwrap();
    let binding_before = registry
        .binding_for_runtime(runtime_before.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        binding_before.last_settled_turn_id.as_deref(),
        Some("prior-turn")
    );
    let attention_before = read_legacy_attention(&fixture.path, fixture.workstream_id);

    for turn_id in ["malformed\nturn".to_owned(), "x".repeat(257)] {
        let error = observe_codex(
            &mut registry,
            &fixture,
            &runtime_before,
            LifecycleEvent::Stop,
            Some(turn_id),
            None,
        );
        assert!(matches!(error, Err(StateError::InvalidProviderMetadata)));
        assert_eq!(
            registry
                .runtime_for_workstream(fixture.workstream_id)
                .unwrap()
                .unwrap(),
            runtime_before
        );
        assert_eq!(
            registry
                .workstream_overviews()
                .unwrap()
                .into_iter()
                .find(|overview| overview.workstream_id == fixture.workstream_id)
                .unwrap(),
            overview_before
        );
        assert_eq!(
            registry
                .binding_for_runtime(runtime_before.runtime_id)
                .unwrap()
                .unwrap(),
            binding_before
        );
        assert_eq!(
            read_legacy_attention(&fixture.path, fixture.workstream_id),
            attention_before
        );
    }
}

#[test]
fn current_open_refuses_missing_lock_for_nonempty_root() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let connection = Connection::open(path.join("host.sqlite")).unwrap();
    connection
        .execute_batch("CREATE TABLE marker (value TEXT); PRAGMA application_id = 0x57534e56; PRAGMA user_version = 14;")
        .unwrap();
    drop(connection);
    fs::set_permissions(path.join("host.sqlite"), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        raw_identity(&path.join("host.sqlite")),
        (14, HOST_APPLICATION_ID)
    );
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(
        matches!(error, StateError::HostStateResetRequired(14)),
        "unexpected: {error:?}"
    );
}

#[test]
fn current_open_never_adopts_an_exact_database_without_the_lock() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    fs::remove_file(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::MissingBootstrapLock));
    assert!(!path.join(BOOTSTRAP_LOCK_FILE).exists());
}

#[test]
fn current_open_classifies_future_and_foreign_headers_before_open() {
    for (application_id, user_version, expected) in [
        (HOST_APPLICATION_ID, 16_u32, "future"),
        (0x1234_5678, 15_u32, "foreign"),
    ] {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let database = path.join("host.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE marker (value TEXT);")
            .unwrap();
        connection
            .pragma_update(None, "application_id", application_id)
            .unwrap();
        connection
            .pragma_update(None, "user_version", user_version)
            .unwrap();
        drop(connection);
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
        let error = open_current(&StateRoot::select(&path)).unwrap_err();
        match expected {
            "future" => assert!(matches!(error, StateError::UnsupportedFutureHostSchema(16))),
            "foreign" => assert!(matches!(error, StateError::MalformedHostSchema)),
            _ => unreachable!(),
        }
    }
}

#[test]
fn sequence_ids_are_not_required_for_direct_bootstrap() {
    #[derive(Default)]
    struct FixedIds;
    impl IdGenerator for FixedIds {
        fn uuid(&self) -> Uuid {
            Uuid::from_u128(1)
        }
    }
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &FixedIds).unwrap());
    assert!(path.join(BOOTSTRAP_LOCK_FILE).is_file());
}

#[test]
fn direct_schema15_retains_current_registry_operations() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut state = create_current(&path, &RandomIdGenerator).unwrap();
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &checkout,
            "checkout",
            ProviderKind::Codex,
            &RandomIdGenerator,
        )
        .unwrap();
    drop(state);
    let state = open_current(&StateRoot::select(&path)).unwrap();
    assert_eq!(state.mode(), super::StateMode::Current);
    let registry = state.into_host_registry().unwrap();
    let workstreams = registry.workstream_overviews().unwrap();
    assert_eq!(workstreams.len(), 1);
    assert_eq!(workstreams[0].workstream_id, workstream_id);
}

fn parked_runtime_fixture() -> (
    tempfile::TempDir,
    HostRegistry,
    WorkstreamId,
    crate::domain::RuntimeId,
    RuntimeRecord,
    ProviderBinding,
    crate::domain::Revision,
) {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let checkout = temporary.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let mut state = create_current(&path, &RandomIdGenerator).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &checkout,
            "checkout",
            ProviderKind::Codex,
            &RandomIdGenerator,
        )
        .unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "retained-session".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    let runtime = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::Stop,
                cwd,
                native_session_id: "retained-session".to_owned(),
                turn_id: Some("settled-turn".to_owned()),
                source: None,
            },
        )
        .unwrap();
    let runtime = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    let runtime_id = runtime.runtime_id;
    let binding = registry.binding_for_runtime(runtime_id).unwrap().unwrap();
    registry
        .park_runtime(runtime.runtime_id, runtime.revision)
        .unwrap();
    let parked_runtime = registry.runtime_by_id(runtime_id).unwrap().unwrap();
    let parked = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert_eq!(parked.lifecycle, crate::domain::WorkstreamLifecycle::Parked);
    assert_eq!(
        parked.runtime.as_ref().map(|runtime| runtime.status),
        Some(crate::domain::RuntimeStatus::Stopped)
    );

    (
        temporary,
        registry,
        workstream_id,
        runtime_id,
        parked_runtime,
        binding,
        parked.revision,
    )
}

#[test]
fn restore_normalizes_only_parked_workstreams_and_preserves_runtime_state() {
    let (
        _temporary,
        mut registry,
        workstream_id,
        runtime_id,
        parked_runtime,
        binding,
        parked_revision,
    ) = parked_runtime_fixture();
    let archived_revision = registry
        .archive_workstream(workstream_id, parked_revision, 1)
        .unwrap();
    let restored_revision = registry
        .restore_workstream(workstream_id, archived_revision)
        .unwrap();
    let restored = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert_eq!(restored.lifecycle, crate::domain::WorkstreamLifecycle::Open);
    assert_eq!(restored.revision, restored_revision);
    assert_eq!(
        registry.runtime_by_id(runtime_id).unwrap().unwrap(),
        parked_runtime
    );
    assert_eq!(
        registry.binding_for_runtime(runtime_id).unwrap(),
        Some(binding)
    );
}

#[test]
fn restore_preserves_live_runtime_byte_for_byte_and_only_changes_visibility() {
    let fixture = codex_lifecycle_fixture();
    let mut registry = open_current(&StateRoot::select(&fixture.path))
        .unwrap()
        .into_host_registry()
        .unwrap();
    let runtime = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    observe_codex(
        &mut registry,
        &fixture,
        &runtime,
        LifecycleEvent::UserPromptSubmit,
        None,
        None,
    )
    .unwrap();
    let runtime_before = registry
        .runtime_for_workstream(fixture.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime_before.status, crate::domain::RuntimeStatus::Working);
    let active = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == fixture.workstream_id)
        .unwrap();
    let archived_revision = registry
        .archive_workstream(fixture.workstream_id, active.revision, 3)
        .unwrap();
    let archived = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == fixture.workstream_id)
        .unwrap();
    assert_eq!(archived.archived_at_millis, Some(3));
    assert_eq!(
        archived.runtime.as_ref(),
        Some(&runtime_before),
        "the low-level catalog transition must retain the live Runtime unchanged"
    );

    let runtime_at_restore = registry
        .runtime_by_id(runtime_before.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime_at_restore, runtime_before);
    let restored_revision = registry
        .restore_workstream(fixture.workstream_id, archived_revision)
        .unwrap();
    let runtime_after = registry
        .runtime_by_id(runtime_before.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime_after, runtime_at_restore);
    let restored = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == fixture.workstream_id)
        .unwrap();
    let mut expected = archived;
    expected.archived_at_millis = None;
    expected.revision = restored_revision;
    assert_eq!(restored, expected);
}

#[test]
fn restore_preserves_recovery_required_lifecycle() {
    let (
        _temporary,
        mut registry,
        workstream_id,
        runtime_id,
        _parked_runtime,
        _binding,
        _parked_revision,
    ) = parked_runtime_fixture();
    let stopped_runtime = registry.runtime_by_id(runtime_id).unwrap().unwrap();
    registry
        .mark_runtime_recovery_required(stopped_runtime.runtime_id, stopped_runtime.revision)
        .unwrap();
    let recovery = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert_eq!(
        recovery.lifecycle,
        crate::domain::WorkstreamLifecycle::RecoveryRequired
    );
    let archived_recovery_revision = registry
        .archive_workstream(workstream_id, recovery.revision, 2)
        .unwrap();
    let restored_recovery_revision = registry
        .restore_workstream(workstream_id, archived_recovery_revision)
        .unwrap();
    let restored_recovery = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert_eq!(
        restored_recovery.lifecycle,
        crate::domain::WorkstreamLifecycle::RecoveryRequired
    );
    assert_eq!(restored_recovery.revision, restored_recovery_revision);
}

#[test]
fn current_startup_assessment_opens_only_the_schema15_epoch() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let root = StateRoot::select(&path);
    let assessment = crate::startup::assess_current_startup(&root).unwrap();
    assert!(matches!(
        assessment,
        crate::startup::StartupAssessment::Fresh(_)
    ));
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let assessment = crate::startup::assess_current_startup(&root).unwrap();
    assert!(matches!(
        assessment,
        crate::startup::StartupAssessment::Current(_)
    ));
}

#[test]
fn bootstrap_restarts_at_each_durable_phase_boundary() {
    const PHASES: &[BootstrapCheckpoint] = &[
        BootstrapCheckpoint::RootReserved,
        BootstrapCheckpoint::DatabaseCreateReserved,
        BootstrapCheckpoint::DatabaseOwned,
        BootstrapCheckpoint::DatabaseInitialized,
        BootstrapCheckpoint::DatabaseReady,
        BootstrapCheckpoint::DatabasePublished,
        BootstrapCheckpoint::ProvisionalPending,
        BootstrapCheckpoint::ProvisionalCreated,
        BootstrapCheckpoint::ProvisionalReady,
        BootstrapCheckpoint::Ready,
    ];

    for (index, checkpoint) in PHASES.iter().copied().enumerate() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join(format!("state-{index}"));
        let mut hook = move |observed| {
            if observed == checkpoint {
                Err(StateError::BootstrapEffectUnknown)
            } else {
                Ok(())
            }
        };
        let error =
            create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
        assert!(matches!(error, StateError::BootstrapEffectUnknown));
        let reopened = open_current(&StateRoot::select(&path))
            .unwrap_or_else(|error| panic!("{checkpoint:?} did not recover: {error:?}"));
        assert_eq!(reopened.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    }
}

#[test]
fn root_reservation_defers_staging_name_until_database_reservation() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut saw_root_reserved = false;
    let mut saw_database_create_reserved = false;
    let mut hook = |checkpoint| {
        let lock = path.join(BOOTSTRAP_LOCK_FILE);
        let wire: Value = serde_json::from_slice(&fs::read(lock).unwrap()).unwrap();
        match checkpoint {
            BootstrapCheckpoint::RootReserved => {
                saw_root_reserved = true;
                assert_eq!(wire["body"]["phase"], "root_reserved");
                assert!(wire["body"]["database_name"].is_null());
                assert!(wire["body"]["database_device"].is_null());
                assert!(wire["body"]["database_inode"].is_null());
                assert!(wire["body"]["provisional_device"].is_null());
                assert!(wire["body"]["provisional_inode"].is_null());
            }
            BootstrapCheckpoint::DatabaseCreateReserved => {
                saw_database_create_reserved = true;
                assert_eq!(wire["body"]["phase"], "database_create_reserved");
                let generation = wire["body"]["bootstrap_generation"]
                    .as_str()
                    .unwrap()
                    .replace('-', "");
                assert_eq!(
                    wire["body"]["database_name"],
                    format!("host.sqlite.bootstrap-{generation}")
                );
                return Err(StateError::BootstrapEffectUnknown);
            }
            _ => {}
        }
        Ok(())
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    assert!(saw_root_reserved);
    assert!(saw_database_create_reserved);
    let resumed = open_current(&StateRoot::select(&path)).unwrap();
    assert_eq!(resumed.schema_version().unwrap(), HOST_SCHEMA_VERSION);
}

#[test]
fn bootstrap_refuses_an_uncommitted_staging_identity_after_restart() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::DatabaseCreated {
            Err(StateError::BootstrapEffectUnknown)
        } else {
            Ok(())
        }
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    assert!(fs::read_dir(&path).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("host.sqlite.bootstrap-")
    }));
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );
}

#[test]
fn bootstrap_refuses_a_foreign_owned_database_before_publication() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::DatabaseInitialized {
            let stage = fs::read_dir(&path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|candidate| {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("host.sqlite.bootstrap-"))
                })
                .expect("staging database");
            let connection = Connection::open(stage).unwrap();
            connection
                .execute(
                    "UPDATE host_identity SET registry_generation = ?1 WHERE singleton = 1",
                    [Uuid::new_v4().to_string()],
                )
                .unwrap();
            Err(StateError::BootstrapEffectUnknown)
        } else {
            Ok(())
        }
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapArtifactMismatch));
    assert!(!path.join("host.sqlite").exists());
    assert!(fs::read_dir(&path).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("host.sqlite.bootstrap-")
    }));
}

#[test]
fn bootstrap_refuses_a_foreign_host_identity_before_publication() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::DatabaseInitialized {
            let stage = fs::read_dir(&path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|candidate| {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("host.sqlite.bootstrap-"))
                })
                .expect("staging database");
            let connection = Connection::open(stage).unwrap();
            connection
                .execute(
                    "UPDATE host_identity SET host_id = ?1 WHERE singleton = 1",
                    [Uuid::new_v4().to_string()],
                )
                .unwrap();
            Err(StateError::BootstrapEffectUnknown)
        } else {
            Ok(())
        }
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapArtifactMismatch));
}

#[test]
fn ready_open_refuses_host_identity_or_generation_tampering_without_mutation() {
    for (table, column) in [
        ("host_identity", "host_id"),
        ("host_identity", "registry_generation"),
        ("host_operational_metadata", "bootstrap_host_id"),
        ("host_operational_metadata", "bootstrap_generation"),
    ] {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        drop(create_current(&path, &RandomIdGenerator).unwrap());
        let database = path.join("host.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                &format!("UPDATE {table} SET {column} = ?1 WHERE singleton = 1"),
                [Uuid::new_v4().to_string()],
            )
            .unwrap();
        drop(connection);
        let database_before = fs::read(&database).unwrap();
        let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
        let provisional_before = fs::read(path.join("provisional.lock")).unwrap();
        let error = open_current(&StateRoot::select(&path)).unwrap_err();
        assert!(matches!(error, StateError::BootstrapArtifactMismatch));
        assert_eq!(fs::read(&database).unwrap(), database_before);
        assert_eq!(
            fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
            lock_before
        );
        assert_eq!(
            fs::read(path.join("provisional.lock")).unwrap(),
            provisional_before
        );
    }
}

#[test]
fn ready_open_validates_provisional_contents_and_inode() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let provisional = path.join("provisional.lock");
    let original = fs::read(&provisional).unwrap();
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let mut changed = original.clone();
    changed[0] ^= 1;
    fs::write(&provisional, changed).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::InvalidProvisionalLease));
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );

    fs::write(&provisional, original.clone()).unwrap();
    let moved = temporary.path().join("provisional.lock.moved");
    fs::rename(&provisional, &moved).unwrap();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut replacement = options.open(&provisional).unwrap();
    replacement.write_all(&original).unwrap();
    replacement.sync_all().unwrap();
    drop(replacement);
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapArtifactMismatch));
    assert!(moved.exists());
    assert_eq!(fs::read(&provisional).unwrap(), original);
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );
}

#[test]
fn ready_open_refuses_replaced_or_nonprivate_database_without_cleanup() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("replaced-db-state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let database = path.join("host.sqlite");
    let moved = temporary.path().join("host.sqlite.moved");
    fs::rename(&database, &moved).unwrap();
    fs::copy(&moved, &database).unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let replacement_before = fs::read(&database).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapArtifactMismatch));
    assert!(moved.exists());
    assert_eq!(fs::read(&database).unwrap(), replacement_before);
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );

    let path = temporary.path().join("mode-db-state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let database = path.join("host.sqlite");
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o644)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapArtifactMismatch));
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );
}

#[test]
fn swapped_root_is_rejected_before_any_replacement_mutation() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let replacement = temporary.path().join("replacement");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::RootReserved {
            fs::rename(&path, &replacement).unwrap();
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            return Ok(());
        }
        Ok(())
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::InvalidBootstrapLock));
    assert!(path.is_dir());
    assert!(fs::read_dir(&path).unwrap().next().is_none());
    assert!(replacement.join(BOOTSTRAP_LOCK_FILE).is_file());
}

#[test]
fn replaced_lock_is_rejected_while_the_original_lease_is_held() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let moved = temporary.path().join("bootstrap.lock.moved");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::RootReserved {
            let lock = path.join(BOOTSTRAP_LOCK_FILE);
            let bytes = fs::read(&lock).unwrap();
            fs::rename(&lock, &moved).unwrap();
            fs::write(&lock, bytes).unwrap();
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
        }
        Ok(())
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::InvalidBootstrapLock));
    assert!(moved.exists());
    assert!(path.join(BOOTSTRAP_LOCK_FILE).exists());
}

#[test]
fn replaced_staging_database_is_not_adopted_after_restart() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::DatabaseInitialized {
            let stage = fs::read_dir(&path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|candidate| {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("host.sqlite.bootstrap-"))
                })
                .unwrap();
            let moved = temporary.path().join("stage.moved");
            fs::rename(&stage, &moved).unwrap();
            fs::copy(&moved, &stage).unwrap();
            fs::set_permissions(&stage, fs::Permissions::from_mode(0o600)).unwrap();
            return Err(StateError::BootstrapEffectUnknown);
        }
        Ok(())
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapArtifactMismatch));
}

#[test]
fn staging_sidecar_and_stage_final_coexistence_are_effect_unknown() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("sidecar-state");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::DatabaseInitialized {
            return Err(StateError::BootstrapEffectUnknown);
        }
        Ok(())
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    let stage = fs::read_dir(&path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("host.sqlite.bootstrap-"))
        })
        .unwrap();
    let sidecar = stage.with_file_name(format!(
        "{}-journal",
        stage.file_name().unwrap().to_string_lossy()
    ));
    fs::write(&sidecar, b"unexpected sidecar").unwrap();
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );
    assert!(sidecar.exists());

    let path = temporary.path().join("both-state");
    let mut hook = |checkpoint| {
        if checkpoint == BootstrapCheckpoint::DatabaseReady {
            return Err(StateError::BootstrapEffectUnknown);
        }
        Ok(())
    };
    let error =
        create_current_with_checkpoint_hook(&path, &RandomIdGenerator, &mut hook).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    let stage = fs::read_dir(&path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("host.sqlite.bootstrap-"))
        })
        .unwrap();
    let destination = path.join("host.sqlite");
    fs::copy(&stage, &destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o600)).unwrap();
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapEffectUnknown));
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );
    assert!(stage.exists());
    assert!(destination.exists());
}

#[test]
fn malformed_oversized_and_checksum_lock_records_refuse_without_rewrite() {
    for mutation in ["oversized", "checksum"] {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        drop(create_current(&path, &RandomIdGenerator).unwrap());
        let lock = path.join(BOOTSTRAP_LOCK_FILE);
        let original = fs::read(&lock).unwrap();
        let changed = if mutation == "oversized" {
            vec![b'x'; 4097]
        } else {
            let mut bytes = original.clone();
            let index = bytes.iter().position(|byte| *byte == b'0').unwrap_or(0);
            bytes[index] = b'1';
            bytes
        };
        fs::write(&lock, &changed).unwrap();
        let error = open_current(&StateRoot::select(&path)).unwrap_err();
        assert!(matches!(error, StateError::InvalidBootstrapLock));
        assert_eq!(fs::read(&lock).unwrap(), changed);
    }
}

#[test]
fn nonprivate_and_symlink_current_artifacts_refuse_without_cleanup() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("mode-state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let lock = path.join(BOOTSTRAP_LOCK_FILE);
    let lock_before = fs::read(&lock).unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::InvalidBootstrapLock));
    assert_eq!(fs::read(&lock).unwrap(), lock_before);

    let path = temporary.path().join("symlink-state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let provisional = path.join("provisional.lock");
    let moved = temporary.path().join("provisional.moved");
    fs::rename(&provisional, &moved).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&moved, &provisional).unwrap();
    let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::InvalidProvisionalLease));
    assert_eq!(
        fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
        lock_before
    );
    assert!(moved.exists());
}

#[test]
fn unknown_top_level_artifacts_refuse_without_mutation() {
    for name in ["client.sqlite", "transition.lock", "retired-marker"] {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        drop(create_current(&path, &RandomIdGenerator).unwrap());
        let lock_before = fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap();
        fs::write(path.join(name), b"retired evidence").unwrap();
        let error = open_current(&StateRoot::select(&path)).unwrap_err();
        assert!(matches!(
            error,
            StateError::StateRecoveryRequired(_) | StateError::FreshRootRejected(_)
        ));
        assert_eq!(
            fs::read(path.join(BOOTSTRAP_LOCK_FILE)).unwrap(),
            lock_before
        );
        assert!(path.join(name).exists());
    }
}

#[test]
fn bootstrap_refuses_torn_lock_without_rewriting_it() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let lock = path.join(BOOTSTRAP_LOCK_FILE);
    fs::write(&lock, b"{\"torn\"").unwrap();
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::InvalidBootstrapLock));
    assert_eq!(fs::read(lock).unwrap(), b"{\"torn\"");
}

#[test]
fn bootstrap_refuses_a_busy_lock_without_opening_state() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("state");
    drop(create_current(&path, &RandomIdGenerator).unwrap());
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join(BOOTSTRAP_LOCK_FILE))
        .unwrap();
    let lock = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .expect("test lock");
    let error = open_current(&StateRoot::select(&path)).unwrap_err();
    assert!(matches!(error, StateError::BootstrapLockBusy));
    drop(lock);
    let reopened = open_current(&StateRoot::select(&path)).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), HOST_SCHEMA_VERSION);
}
