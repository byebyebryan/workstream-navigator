use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use rusqlite::Connection;
use uuid::Uuid;

use crate::domain::{
    IdGenerator, ProviderKind, Revision, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
};
use crate::provider::lifecycle::{LifecycleEvent, LifecycleObservation};

use super::*;

#[derive(Default)]
struct SequenceIds(AtomicU64);

impl IdGenerator for SequenceIds {
    fn uuid(&self) -> Uuid {
        Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed) + 1))
    }
}

fn registered_registry(
    provider: ProviderKind,
) -> (
    tempfile::TempDir,
    HostRegistry,
    ProjectLocationWorkstreamRegistration,
) {
    let temporary = tempfile::tempdir().expect("temporary state root");
    let state_root = temporary.path().join("state");
    let mut state = fresh_create(&state_root, &SequenceIds::default()).expect("fresh state");
    let registration = state
        .register_project_location_with_initial_workstream(
            Path::new("/fixture/project"),
            "fixture project",
            None,
            None,
            provider,
            &SequenceIds::default(),
        )
        .expect("registered project location");
    let registry = state.into_host_registry().expect("current registry");
    (temporary, registry, registration)
}

fn lifecycle(registry: &mut HostRegistry, workstream_id: WorkstreamId) -> RuntimeRecord {
    let runtime = registry
        .reserve_runtime(workstream_id)
        .expect("reserve runtime");
    let runtime_id = runtime.runtime_id;
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 101, "birth-a")
        .expect("process identity");
    registry
        .apply_lifecycle_observation(
            runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .expect("session start");
    registry
        .apply_lifecycle_observation(
            runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: None,
            },
        )
        .expect("prompt submit");
    registry
        .apply_lifecycle_observation(
            runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::Stop,
                cwd,
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
        )
        .expect("stop");
    registry
        .runtime_by_id(runtime_id)
        .expect("runtime read")
        .expect("runtime retained")
}

#[test]
fn fresh_schema13_identity_is_stable_and_private() {
    let (temporary, registry, _registration) = registered_registry(ProviderKind::Codex);
    let identity = registry.identity().expect("identity");
    assert_ne!(identity.host_id.as_uuid(), Uuid::nil());
    assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    let database = temporary.path().join("state/host.sqlite");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(database).unwrap().mode() & 0o777, 0o600);
    }
}

#[test]
fn browser_listing_is_bounded_and_keeps_paths_private() {
    let (temporary, mut registry, _registration) = registered_registry(ProviderKind::Codex);
    let browser_root = temporary.path().join("browser");
    fs::create_dir(&browser_root).unwrap();
    fs::create_dir(browser_root.join("repository")).unwrap();
    fs::create_dir(browser_root.join("repository/.git")).unwrap();
    fs::create_dir(browser_root.join("scratch")).unwrap();
    registry
        .set_project_browser_root(&browser_root.to_string_lossy())
        .unwrap();
    let listing = registry.project_directories("", false).unwrap();
    assert_eq!(listing.root_label, "custom root · browser");
    assert_eq!(listing.entries.len(), 2);
    assert!(listing.entries[0].is_git_repository);
    assert!(
        !listing
            .root_label
            .contains(&temporary.path().to_string_lossy().to_string())
    );
}

#[test]
fn codex_lifecycle_binds_exact_generation_and_sticky_attention() {
    let (_temporary, mut registry, registration) = registered_registry(ProviderKind::Codex);
    let settled = lifecycle(&mut registry, registration.workstream.workstream_id);
    assert_eq!(settled.status, RuntimeStatus::Attention);
    let attention = registry
        .attention(registration.workstream.workstream_id)
        .unwrap()
        .unwrap();
    assert!(attention.result_unseen_since_revision.is_some());
    assert_eq!(attention.latest_turn_id.as_deref(), Some("turn-a"));
    assert!(matches!(
        registry.apply_lifecycle_observation(
            settled.runtime_id,
            "stale-generation",
            LifecycleObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: settled.cwd.to_string_lossy().into_owned(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: None,
            },
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
}

#[test]
fn result_attention_requires_the_current_revision_to_acknowledge() {
    let (_temporary, mut registry, registration) = registered_registry(ProviderKind::Codex);
    let settled = lifecycle(&mut registry, registration.workstream.workstream_id);
    let attention = registry
        .attention(registration.workstream.workstream_id)
        .unwrap()
        .unwrap();
    assert!(matches!(
        registry.acknowledge_result_attention(
            registration.workstream.workstream_id,
            Revision::INITIAL,
        ),
        Err(StateError::Domain(crate::domain::DomainError::RevisionConflict { .. }))
    ));
    let cleared = registry
        .acknowledge_result_attention(registration.workstream.workstream_id, attention.revision)
        .unwrap();
    assert!(cleared.result_unseen_since_revision.is_none());
    assert_eq!(settled.status, RuntimeStatus::Attention);
}

#[test]
fn independent_workstream_dedup_preserves_provider_identity() {
    let (_temporary, mut registry, registration) = registered_registry(ProviderKind::Codex);
    let source = registration.workstream.workstream_id;
    let first = registry
        .create_independent_workstream(
            "request-a",
            source,
            Revision::INITIAL,
            ProviderKind::OpenCode,
        )
        .unwrap();
    let replay = registry
        .create_independent_workstream(
            "request-a",
            source,
            Revision::INITIAL,
            ProviderKind::OpenCode,
        )
        .unwrap();
    assert_eq!(first, replay);
    assert!(matches!(
        registry.create_independent_workstream(
            "request-a",
            source,
            Revision::INITIAL,
            ProviderKind::Codex,
        ),
        Err(StateError::OperationRequestMismatch)
    ));
}

#[test]
fn archive_and_restore_are_visibility_transitions_with_revision_guards() {
    let (_temporary, mut registry, registration) = registered_registry(ProviderKind::Codex);
    let workstream_id = registration.workstream.workstream_id;
    let archived = registry
        .archive_workstream(workstream_id, Revision::INITIAL, 123)
        .unwrap();
    assert!(
        registry
            .workstream_overviews()
            .unwrap()
            .iter()
            .find(|workstream| workstream.workstream_id == workstream_id)
            .unwrap()
            .archived_at_millis
            .is_some()
    );
    assert!(matches!(
        registry.restore_workstream(workstream_id, Revision::INITIAL),
        Err(StateError::Domain(
            crate::domain::DomainError::RevisionConflict { .. }
        ))
    ));
    let restored = registry
        .restore_workstream(workstream_id, archived)
        .unwrap();
    assert_eq!(restored, Revision::INITIAL.next().next());
    assert!(
        registry
            .workstream_overviews()
            .unwrap()
            .iter()
            .find(|workstream| workstream.workstream_id == workstream_id)
            .unwrap()
            .archived_at_millis
            .is_none()
    );
}

#[test]
fn runtime_process_identity_requires_exact_revision_and_birth_pair() {
    let (_temporary, mut registry, registration) = registered_registry(ProviderKind::Codex);
    let runtime = registry
        .reserve_runtime(registration.workstream.workstream_id)
        .unwrap();
    assert!(matches!(
        registry.record_runtime_process_identity(
            runtime.runtime_id,
            runtime.revision.next(),
            101,
            "birth-a",
        ),
        Err(StateError::ConcurrentWrite)
    ));
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 101, "birth-a")
        .unwrap();
    let recorded = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    assert_eq!(recorded.provider_pid, Some(101));
    assert_eq!(recorded.process_birth.as_deref(), Some("birth-a"));
}

#[test]
fn deliberate_park_resolves_recovery_required_runtime_to_parked_and_stopped() {
    let (_temporary, mut registry, registration) = registered_registry(ProviderKind::Codex);
    let runtime = registry
        .reserve_runtime(registration.workstream.workstream_id)
        .unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 101, "birth-a")
        .unwrap();
    let recorded = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    registry
        .mark_runtime_recovery_required(recorded.runtime_id, recorded.revision)
        .unwrap();
    let recovery = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    assert_eq!(recovery.status, RuntimeStatus::Unknown);

    registry
        .park_runtime(recovery.runtime_id, recovery.revision)
        .unwrap();

    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registration.workstream.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::Parked);
    assert_eq!(overview.runtime.unwrap().status, RuntimeStatus::Stopped);
}

#[test]
fn current_only_open_refuses_future_schema_without_mutation() {
    let (temporary, _registry, _registration) = registered_registry(ProviderKind::Codex);
    let path = temporary.path().join("state/host.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute("PRAGMA user_version = 14", []).unwrap();
    drop(connection);
    let root = StateRoot::select(temporary.path().join("state"));
    assert!(matches!(
        open_current_only(&root),
        Err(StateError::UnsupportedFutureHostSchema(14))
    ));
}
