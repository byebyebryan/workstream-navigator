//! Focused coverage for the D16 production cutover adapters.

use std::fs;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use uuid::Uuid;
use wsnav::{
    cutover::{
        CutoverError, CutoverProcessAuthority, CutoverStateFactory,
        LinuxOpenCodeCutoverProcessAuthority, LiveCutoverStateFactory, LivePresentationAuthority,
        ObserverProcessState, PresentationProofSource,
    },
    domain::{ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeStatus, WorkstreamId},
    presentation::LegacyPresentationState,
    provider::names::NameState,
    runtime::{LinuxProcessProbe, ProcessProbe},
    state::{
        ObserverProcessIdentity, OpenCodeObserverProjection, OpenCodeObserverStatus,
        OpenCodeRuntimeHandle, ProviderBinding, RuntimeRecord, StateRoot, TransitionLease,
        acquire_transition_lease, exact_schema_12_fixture_sql,
    },
};

fn private_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary root");
    #[cfg(unix)]
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("root mode");
    root
}

fn transition_lease(root: &std::path::Path) -> TransitionLease {
    let lock = root.join("transition.lock");
    fs::write(&lock, b"").expect("transition lock");
    #[cfg(unix)]
    fs::set_permissions(lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
    acquire_transition_lease(root).expect("exclusive lease")
}

fn handle() -> OpenCodeRuntimeHandle {
    OpenCodeRuntimeHandle {
        runtime_id: RuntimeId::from(Uuid::from_u128(1)),
        runtime_generation: "generation-a".to_owned(),
        endpoint_host: "127.0.0.1".to_owned(),
        endpoint_port: 4312,
        version: "1.0".to_owned(),
        native_session_id: ProviderSessionId::new(ProviderKind::OpenCode, "session-a")
            .expect("session identity"),
        observer_pid: Some(std::process::id()),
        observer_birth: Some("not-used-by-standby-test".to_owned()),
        observer_status: OpenCodeObserverStatus::Ready,
        revision: Revision::INITIAL,
    }
}

fn projection() -> OpenCodeObserverProjection {
    let handle = handle();
    OpenCodeObserverProjection {
        runtime: RuntimeRecord {
            runtime_id: handle.runtime_id,
            workstream_id: WorkstreamId::new(),
            provider: ProviderKind::OpenCode,
            tmux_generation: handle.runtime_generation.clone(),
            tmux_session: "private-runtime".to_owned(),
            cwd: std::path::PathBuf::from("/workspace/project"),
            provider_pid: Some(1),
            process_birth: Some("provider-birth".to_owned()),
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        },
        binding: ProviderBinding {
            runtime_id: handle.runtime_id,
            provider: ProviderKind::OpenCode,
            native_session_id: handle.native_session_id.clone(),
            start_source: "test".to_owned(),
            last_settled_turn_id: None,
            observed_thread_name: None,
            name_state: NameState::Unavailable,
            predecessor_native_session_id: None,
            predecessor_effective_name: None,
            runtime_generation: handle.runtime_generation.clone(),
            revision: Revision::INITIAL,
        },
        handle,
    }
}

#[test]
fn live_presentation_authority_uses_the_real_read_only_classifier() {
    let root = private_root();
    let mut authority = LivePresentationAuthority::new(root.path());
    let assessment = authority.prove(root.path()).expect("presentation proof");
    assert_eq!(assessment.state(), LegacyPresentationState::None);
    assert!(assessment.proof().is_none());
}

#[test]
fn live_state_factory_opens_only_after_the_transition_lease() {
    let root = private_root();
    let state_root = StateRoot::select(root.path());
    let database =
        rusqlite::Connection::open(state_root.host_database_path()).expect("schema-12 host state");
    database
        .execute_batch(exact_schema_12_fixture_sql())
        .expect("schema-12 fixture");
    database
        .execute(
            "INSERT INTO host_identity (
                singleton, host_id, registry_generation, schema_version
             ) VALUES (1, ?1, 'fixture-generation', 12)",
            [Uuid::new_v4().to_string()],
        )
        .expect("schema-12 host identity");
    database
        .execute_batch("PRAGMA user_version = 12")
        .expect("schema-12 version");
    drop(database);
    #[cfg(unix)]
    fs::set_permissions(
        state_root.host_database_path(),
        fs::Permissions::from_mode(0o600),
    )
    .expect("database mode");
    let lease = transition_lease(root.path());
    let mut factory = LiveCutoverStateFactory::new(state_root);
    assert!(!factory.is_open());
    let authority = factory.open_under_lease(&lease).expect("lease-bound state");
    assert_eq!(authority.schema_version().expect("schema version"), 12);
    assert!(factory.is_open());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_process_authority_observes_exact_birth_and_executable_only() {
    let root = private_root();
    let mut authority = LinuxOpenCodeCutoverProcessAuthority::new(root.path());
    let pid = std::process::id();
    let birth = LinuxProcessProbe
        .process_birth(pid)
        .expect("current process birth");
    let executable = std::env::current_exe()
        .expect("current executable")
        .to_string_lossy()
        .into_owned();
    let expected = ObserverProcessIdentity {
        pid,
        birth: birth.clone(),
        executable,
    };
    assert!(matches!(
        authority.observe(&expected),
        Ok(ObserverProcessState::Running(actual)) if actual == expected
    ));

    let mut changed = expected.clone();
    changed.birth = "reused-pid".to_owned();
    assert_eq!(
        authority.observe(&changed).expect("identity observation"),
        ObserverProcessState::IdentityMismatch
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_process_authority_refuses_an_incomplete_runtime_projection() {
    let root = private_root();
    let mut authority = LinuxOpenCodeCutoverProcessAuthority::new(root.path());
    let mut target = projection();
    target.runtime.provider_pid = None;
    assert!(matches!(
        authority.corroborate_observer(&target),
        Err(CutoverError::RuntimeProjectionUnavailable)
    ));
}

#[test]
fn standby_creation_refuses_an_inexact_runtime_before_spawn() {
    let root = private_root();
    let mut authority = LinuxOpenCodeCutoverProcessAuthority::new(root.path());
    let error = authority
        .start_standby(&projection())
        .expect_err("inexact fixture identity must fail closed");
    assert!(!matches!(error, CutoverError::StandbyObserverUnavailable));
}
