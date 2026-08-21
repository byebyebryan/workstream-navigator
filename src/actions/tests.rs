use super::{
    ActionError, IntegrationLifecycle, ObserverProfile, OsString, ProcessProbe, ProviderBinding,
    ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeProbe, WorkstreamId,
    WorkstreamLifecycle, archive,
    attachment::inspect_opencode_prior_runtime,
    cleanup::{
        attachment_runtime_matches, fail_cleanup_unknown_opencode_session_creation,
        matches_recorded_runtime, observer_identity_matches, spawned_observer_identity_matches,
    },
    codex_recovery_program,
    creation::{IndependentStartSpec, start_independent_workstream_with},
    model::reconcile_observer_trust_with_manager,
    park,
    providers::managed_codex_environment,
    reconcile_lost_runtimes, restore, start,
    start::{
        backfill_live_runtime_provider_pid, opencode_recovery_handle_matches,
        runtime_launch_program,
    },
};
use crate::provider::names::NameState;

use std::{
    cell::Cell,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

fn private_existing_root(path: &Path) -> crate::state::StateRoot {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    crate::state::StateRoot::create(path).unwrap()
}

fn registry() -> (tempfile::TempDir, crate::state::HostRegistry, WorkstreamId) {
    let temporary = tempfile::tempdir().unwrap();
    let root = private_existing_root(temporary.path());
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let registered = registry
        .register_project_root(
            Path::new("/disposable/repository"),
            crate::domain::ProviderKind::Codex,
        )
        .unwrap();
    (temporary, registry, registered.workstream_id)
}

#[test]
fn completed_native_review_promotes_pending_observer_before_a_managed_action() {
    let temporary = tempfile::tempdir().unwrap();
    let root = crate::state::StateRoot::select(temporary.path().join("state"));
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let manager = ObserverProfile::new(
        temporary.path().join("codex-home"),
        temporary.path().join("bin/wsnav"),
        root.base(),
    );
    let ownership = manager.install("owner".to_owned(), None).unwrap();
    registry
        .record_codex_integration(ownership, IntegrationLifecycle::TrustPending)
        .unwrap();

    reconcile_observer_trust_with_manager(&mut registry, &manager).unwrap();
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::TrustPending
    );

    let mut trust = String::from("\n[hooks.state]\n");
    for hook in ["session_start", "user_prompt_submit", "stop", "session_end"] {
        let key =
            serde_json::to_string(&format!("{}:{hook}:0:0", manager.path().display())).unwrap();
        writeln!(
                trust,
                "\n[hooks.state.{key}]\ntrusted_hash = \"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""
            )
            .unwrap();
    }
    fs::write(manager.path(), format!("{}{}", manager.rendered(), trust)).unwrap();

    reconcile_observer_trust_with_manager(&mut registry, &manager).unwrap();
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::Ready
    );
}

#[test]
fn archive_and_restore_without_a_runtime_never_start_codex() {
    let (temporary, mut registry, workstream_id) = registry();
    let root = crate::state::StateRoot::select(temporary.path());

    let archived_revision =
        archive(&root, &mut registry, workstream_id, Revision::INITIAL).unwrap();
    let archived = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert!(archived.archived_at_millis.is_some());
    assert!(archived.runtime.is_none());
    assert!(matches!(
        start(&root, &mut registry, workstream_id, Some(archived_revision)),
        Err(ActionError::WorkstreamArchived)
    ));
    assert!(matches!(
        park(&root, &mut registry, workstream_id, Some(archived_revision)),
        Err(ActionError::WorkstreamArchived)
    ));
    assert!(matches!(
        archive(&root, &mut registry, workstream_id, archived_revision),
        Err(ActionError::WorkstreamAlreadyArchived)
    ));

    let restored_revision = restore(&mut registry, workstream_id, archived_revision).unwrap();
    let restored = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert_eq!(restored.archived_at_millis, None);
    assert!(restored.runtime.is_none());
    assert_eq!(restored.revision, restored_revision);
}

#[test]
fn managed_codex_environment_has_only_the_explicit_utf8_locale() {
    let environment = managed_codex_environment();

    for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
        assert_eq!(
            environment.get(&OsString::from(key)),
            Some(&OsString::from("C.UTF-8"))
        );
    }
    assert_eq!(environment.len(), 3);
}

#[test]
fn opencode_helper_cleanup_failure_marks_pre_effect_creation_for_recovery() {
    let temporary = tempfile::tempdir().unwrap();
    let root = private_existing_root(temporary.path());
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();

    assert!(matches!(
        fail_cleanup_unknown_opencode_session_creation(&mut registry, &prepared),
        ActionError::OpenCodeSessionCreationExternalEffectUnknown
    ));
    assert_eq!(
        registry
            .runtime_for_workstream(registered.workstream_id)
            .unwrap()
            .unwrap()
            .status,
        crate::domain::RuntimeStatus::Unknown
    );
}

#[test]
fn abandoned_prepared_opencode_creation_on_missing_runtime_requires_recovery() {
    let temporary = tempfile::tempdir().unwrap();
    let root = private_existing_root(temporary.path());
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();

    assert!(matches!(
        inspect_opencode_prior_runtime(&root, &mut registry, registered.workstream_id),
        Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode
        ))
    ));

    let current_runtime = registry
        .runtime_for_workstream(registered.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        current_runtime.status,
        crate::domain::RuntimeStatus::Unknown
    );
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);

    let current_operation = registry
        .opencode_session_creation_for_runtime(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap()
        .unwrap();
    assert_eq!(current_operation, prepared);
    assert!(
        registry
            .binding_for_runtime(runtime.runtime_id)
            .unwrap()
            .is_none()
    );

    assert!(matches!(
        start(
            &root,
            &mut registry,
            registered.workstream_id,
            Some(overview.revision),
        ),
        Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode
        ))
    ));
    assert_eq!(
        registry
            .opencode_session_creation_for_runtime(runtime.runtime_id, &runtime.tmux_generation,)
            .unwrap()
            .unwrap(),
        prepared,
        "recovery must not attempt another native session creation"
    );
}

#[test]
fn native_recovery_uses_an_exact_binding_or_the_native_picker() {
    let cwd = Path::new("/disposable/repository");
    let binding = ProviderBinding {
        runtime_id: RuntimeId::new(),
        runtime_generation: "generation-a".to_owned(),
        provider: crate::domain::ProviderKind::Codex,
        native_session_id: crate::domain::ProviderSessionId::codex("known-session").unwrap(),
        start_source: "resume".to_owned(),
        last_settled_turn_id: Some("settled-turn".to_owned()),
        observed_thread_name: None,
        name_state: NameState::Unavailable,
        predecessor_native_session_id: None,
        predecessor_effective_name: None,
        revision: Revision::INITIAL,
    };

    assert_eq!(
        codex_recovery_program(cwd, Some(&binding)),
        vec![
            "codex".into(),
            "--profile".into(),
            "wsnav-observer".into(),
            "-C".into(),
            cwd.as_os_str().to_owned(),
            "resume".into(),
            "known-session".into(),
        ]
    );
    assert_eq!(
        codex_recovery_program(cwd, None),
        vec![
            "codex".into(),
            "--profile".into(),
            "wsnav-observer".into(),
            "-C".into(),
            cwd.as_os_str().to_owned(),
            "resume".into(),
        ]
    );
}

#[test]
fn native_provider_is_wrapped_by_the_private_launch_barrier() {
    let runtime_id = RuntimeId::new();
    let wrapped = runtime_launch_program(
        Path::new("/state"),
        runtime_id,
        vec!["codex".into(), "--profile".into(), "wsnav-observer".into()],
    )
    .unwrap();

    assert_eq!(
        &wrapped[1..],
        &[
            OsString::from("--state-root"),
            OsString::from("/state"),
            OsString::from("_runtime_launch"),
            OsString::from(runtime_id.to_string()),
            OsString::from("--"),
            OsString::from("codex"),
            OsString::from("--profile"),
            OsString::from("wsnav-observer"),
        ]
    );
}

#[test]
fn conclusive_private_runtime_loss_becomes_recovery_required_before_snapshot() {
    let temporary = tempfile::tempdir().unwrap();
    let root = private_existing_root(temporary.path());
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let registered = registry
        .register_project_root(
            Path::new("/disposable/repository"),
            crate::domain::ProviderKind::Codex,
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 42, "birth-a")
        .unwrap();

    reconcile_lost_runtimes(&root, &mut registry).unwrap();

    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
    assert_eq!(
        overview.runtime.as_ref().map(|runtime| runtime.status),
        Some(crate::domain::RuntimeStatus::Unknown)
    );
    assert!(
        overview
            .attention
            .as_ref()
            .and_then(|attention| attention.recovery_unseen_since_revision)
            .is_some()
    );
}

#[test]
fn live_runtime_is_accepted_only_when_its_recorded_identity_matches() {
    let record = crate::state::RuntimeRecord {
        runtime_id: RuntimeId::new(),
        workstream_id: WorkstreamId::new(),
        provider: crate::domain::ProviderKind::Codex,
        tmux_generation: "generation".to_owned(),
        tmux_session: "session".to_owned(),
        cwd: PathBuf::from("/disposable/repository"),
        provider_pid: Some(1),
        process_birth: Some("birth-a".to_owned()),
        status: crate::domain::RuntimeStatus::Idle,
        revision: Revision::INITIAL,
    };
    let exact = RuntimeProbe::Live {
        pane_id: "%1".to_owned(),
        pane_pid: 1,
        cwd: record.cwd.clone(),
        process_birth: Some("birth-a".to_owned()),
    };

    assert!(attachment_runtime_matches(&record, &exact));
    assert!(matches_recorded_runtime(&record, &exact, false));
    assert!(!matches_recorded_runtime(&record, &exact, true));
    assert!(!matches_recorded_runtime(
        &record,
        &RuntimeProbe::Live {
            pane_id: "%1".to_owned(),
            pane_pid: 2,
            cwd: record.cwd.clone(),
            process_birth: Some("birth-a".to_owned()),
        },
        false,
    ));
    assert!(!matches_recorded_runtime(
        &record,
        &RuntimeProbe::Live {
            pane_id: "%1".to_owned(),
            pane_pid: 1,
            cwd: PathBuf::from("/another/checkout"),
            process_birth: Some("birth-a".to_owned()),
        },
        false,
    ));
    assert!(!matches_recorded_runtime(
        &record,
        &RuntimeProbe::Live {
            pane_id: "%1".to_owned(),
            pane_pid: 1,
            cwd: record.cwd.clone(),
            process_birth: Some("birth-b".to_owned()),
        },
        false,
    ));
    assert!(!attachment_runtime_matches(&record, &RuntimeProbe::Missing));
    assert!(!attachment_runtime_matches(
        &record,
        &RuntimeProbe::Unknown {
            diagnostic: "probe unavailable".to_owned(),
        },
    ));
}

#[test]
fn codex_attachment_requires_a_complete_recorded_process_identity() {
    let record = crate::state::RuntimeRecord {
        runtime_id: RuntimeId::new(),
        workstream_id: WorkstreamId::new(),
        provider: crate::domain::ProviderKind::Codex,
        tmux_generation: "generation".to_owned(),
        tmux_session: "session".to_owned(),
        cwd: PathBuf::from("/disposable/repository"),
        provider_pid: None,
        process_birth: None,
        status: crate::domain::RuntimeStatus::Idle,
        revision: Revision::INITIAL,
    };
    let live = RuntimeProbe::Live {
        pane_id: "%1".to_owned(),
        pane_pid: 1,
        cwd: record.cwd.clone(),
        process_birth: Some("birth-a".to_owned()),
    };

    assert!(!attachment_runtime_matches(&record, &live));
}

#[test]
fn exact_live_probe_backfills_a_missing_provider_pid() {
    let temporary = tempfile::tempdir().unwrap();
    let root = private_existing_root(temporary.path());
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let registered = registry
        .register_project_root(
            Path::new("/disposable/repository"),
            crate::domain::ProviderKind::Codex,
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    drop(registry);
    let connection = rusqlite::Connection::open(root.host_database_path()).unwrap();
    connection
        .execute(
            "UPDATE runtimes SET process_birth = 'birth-a' WHERE runtime_id = ?1",
            [runtime.runtime_id.to_string()],
        )
        .unwrap();
    drop(connection);
    let mut registry = crate::state::open_current_only(&root)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let legacy = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    let probe = RuntimeProbe::Live {
        pane_id: "%1".to_owned(),
        pane_pid: 77,
        cwd: legacy.cwd.clone(),
        process_birth: Some("birth-a".to_owned()),
    };

    let repaired = backfill_live_runtime_provider_pid(&mut registry, &legacy, &probe)
        .unwrap()
        .unwrap();

    assert_eq!(repaired.provider_pid, Some(77));
    assert!(matches_recorded_runtime(&repaired, &probe, false));
}

#[test]
fn independent_creation_reuses_its_request_without_a_git_effect() {
    let (_temporary, mut registry, source) = registry();
    let first = registry
        .create_independent_workstream(
            "independent-action",
            source,
            Revision::INITIAL,
            crate::domain::ProviderKind::Codex,
        )
        .unwrap();
    let replay = registry
        .create_independent_workstream(
            "independent-action",
            source,
            Revision::INITIAL,
            crate::domain::ProviderKind::Codex,
        )
        .unwrap();

    assert_eq!(first, replay);
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == first.workstream_id)
        .unwrap();
    assert_eq!(
        overview.project_repository_path,
        PathBuf::from("/disposable/repository")
    );
}

#[test]
fn independent_creation_keeps_the_project_root_without_touching_files() {
    let temporary = tempfile::tempdir().unwrap();
    let repository = temporary.path().join("repository");
    fs::create_dir(&repository).unwrap();
    fs::write(repository.join("source-only.txt"), "do not copy\n").unwrap();

    let root = crate::state::StateRoot::select(temporary.path().join("state"));
    let mut registry = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let registered = registry
        .register_project_root(&repository, crate::domain::ProviderKind::Codex)
        .unwrap();
    let created = registry
        .create_independent_workstream(
            "independent-system-git",
            registered.workstream_id,
            Revision::INITIAL,
            crate::domain::ProviderKind::Codex,
        )
        .unwrap();
    let destination_root = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == created.workstream_id)
        .unwrap()
        .project_repository_path;

    assert_eq!(destination_root, repository);
    assert!(repository.join("source-only.txt").is_file());
    assert_eq!(created.origin, crate::domain::WorkstreamOrigin::Independent);
}

#[test]
fn independent_creation_survives_one_provider_start_failure_without_fallback() {
    let (temporary, mut registry, source_workstream_id) = registry();
    let root = crate::state::StateRoot::select(temporary.path());
    let readiness_calls = Cell::new(0);
    let starter_calls = Cell::new(0);
    let starter_provider = Cell::new(None);
    let selected_provider = ProviderKind::Codex;

    let result = start_independent_workstream_with(
        &root,
        &mut registry,
        IndependentStartSpec {
            source_workstream_id,
            expected_revision: Some(Revision::INITIAL),
            request_key: "independent-start-failure",
            provider: selected_provider,
        },
        |registry, provider| {
            readiness_calls.set(readiness_calls.get() + 1);
            assert_eq!(provider, selected_provider);
            assert_eq!(
                registry
                    .workstream_overviews()
                    .unwrap()
                    .iter()
                    .filter(|overview| overview.provider == selected_provider)
                    .count(),
                1
            );
            Ok(())
        },
        |_root, registry, workstream_id, expected_revision, provider| {
            starter_calls.set(starter_calls.get() + 1);
            starter_provider.set(Some(provider));
            let created = registry
                .workstream_overviews()
                .unwrap()
                .into_iter()
                .find(|overview| overview.workstream_id == workstream_id)
                .unwrap();
            assert_eq!(created.provider, selected_provider);
            assert_eq!(expected_revision, Some(created.revision));
            let reserved = registry
                .reserve_runtime_with_provider(workstream_id, provider)
                .unwrap();
            registry
                .mark_runtime_recovery_required(reserved.runtime_id, reserved.revision)
                .unwrap();
            Err(ActionError::Runtime(
                crate::runtime::RuntimeError::TmuxRejected(
                    "fixture provider launch failed".to_owned(),
                ),
            ))
        },
    );

    assert!(matches!(
        result,
        Err(ActionError::Runtime(
            crate::runtime::RuntimeError::TmuxRejected(ref diagnostic)
        )) if diagnostic == "fixture provider launch failed"
    ));
    assert_eq!(readiness_calls.get(), 1);
    assert_eq!(starter_calls.get(), 1);
    assert_eq!(starter_provider.get(), Some(selected_provider));

    let independent = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id != source_workstream_id)
        .expect("durable independent Workstream remains visible");
    assert_eq!(independent.provider, selected_provider);
    assert_eq!(independent.archived_at_millis, None);
    assert_eq!(independent.lifecycle, WorkstreamLifecycle::RecoveryRequired);
    let runtime = independent
        .runtime
        .expect("failed launch retains its Runtime record");
    assert_eq!(runtime.provider, selected_provider);
    assert_eq!(runtime.status, crate::domain::RuntimeStatus::Unknown);
}

struct FixedBirth(Option<String>);

impl ProcessProbe for FixedBirth {
    fn process_birth(&self, _pid: u32) -> Option<String> {
        self.0.clone()
    }
}

#[test]
fn observer_cleanup_refuses_missing_or_reused_birth_without_signalling() {
    assert!(!observer_identity_matches(&FixedBirth(None), 77, "birth-a"));
    assert!(!observer_identity_matches(
        &FixedBirth(Some("birth-b".to_owned())),
        77,
        "birth-a"
    ));
    assert!(!observer_identity_matches(
        &FixedBirth(Some("birth-a".to_owned())),
        77,
        ""
    ));
    assert!(observer_identity_matches(
        &FixedBirth(Some("birth-a".to_owned())),
        77,
        "birth-a"
    ));
}

#[test]
fn spawned_observer_ready_requires_the_exact_live_pid_and_birth() {
    let handle = crate::state::OpenCodeRuntimeHandle {
        runtime_id: RuntimeId::new(),
        runtime_generation: "generation".to_owned(),
        endpoint_host: crate::provider::opencode::LOOPBACK_HOST.to_owned(),
        endpoint_port: 4321,
        version: "contract-build-a".to_owned(),
        native_session_id: ProviderSessionId::new(ProviderKind::OpenCode, "session").unwrap(),
        observer_pid: Some(77),
        observer_birth: Some("birth-a".to_owned()),
        observer_status: crate::state::OpenCodeObserverStatus::Ready,
        revision: Revision::INITIAL,
    };
    assert!(spawned_observer_identity_matches(
        &handle,
        77,
        "birth-a",
        &FixedBirth(Some("birth-a".to_owned())),
    ));
    assert!(!spawned_observer_identity_matches(
        &handle,
        77,
        "birth-a",
        &FixedBirth(None),
    ));
    assert!(!spawned_observer_identity_matches(
        &handle,
        78,
        "birth-a",
        &FixedBirth(Some("birth-a".to_owned())),
    ));
}

#[test]
fn opencode_recovery_handle_match_is_exact_and_provider_namespaced() {
    let runtime_id = RuntimeId::new();
    let session = ProviderSessionId::new(ProviderKind::OpenCode, "root-session").unwrap();
    let runtime = crate::state::RuntimeRecord {
        runtime_id,
        workstream_id: WorkstreamId::new(),
        provider: ProviderKind::OpenCode,
        tmux_generation: "generation-a".to_owned(),
        tmux_session: "wsnav-runtime".to_owned(),
        cwd: PathBuf::from("/disposable/repository"),
        provider_pid: Some(42),
        process_birth: Some("pane-birth".to_owned()),
        status: crate::domain::RuntimeStatus::Unknown,
        revision: Revision::INITIAL,
    };
    let binding = crate::state::ProviderBinding {
        runtime_id,
        runtime_generation: "generation-a".to_owned(),
        provider: ProviderKind::OpenCode,
        native_session_id: session.clone(),
        start_source: "resume".to_owned(),
        last_settled_turn_id: Some("settled-message".to_owned()),
        observed_thread_name: None,
        name_state: NameState::Unavailable,
        predecessor_native_session_id: None,
        predecessor_effective_name: None,
        revision: Revision::INITIAL,
    };
    let handle = crate::state::OpenCodeRuntimeHandle {
        runtime_id,
        runtime_generation: "generation-a".to_owned(),
        endpoint_host: crate::provider::opencode::LOOPBACK_HOST.to_owned(),
        endpoint_port: 4321,
        version: "contract-build-b".to_owned(),
        native_session_id: session,
        observer_pid: Some(77),
        observer_birth: Some("observer-birth".to_owned()),
        observer_status: crate::state::OpenCodeObserverStatus::Unknown,
        revision: Revision::INITIAL,
    };
    assert!(opencode_recovery_handle_matches(
        &runtime, &binding, &handle
    ));

    let mut mismatched = handle.clone();
    mismatched.runtime_generation = "generation-b".to_owned();
    assert!(!opencode_recovery_handle_matches(
        &runtime,
        &binding,
        &mismatched
    ));
    mismatched = handle.clone();
    mismatched.native_session_id = ProviderSessionId::new(ProviderKind::OpenCode, "other").unwrap();
    assert!(!opencode_recovery_handle_matches(
        &runtime,
        &binding,
        &mismatched
    ));
    mismatched = handle.clone();
    mismatched.endpoint_host = "0.0.0.0".to_owned();
    assert!(!opencode_recovery_handle_matches(
        &runtime,
        &binding,
        &mismatched
    ));
    mismatched = handle;
    let mut codex_binding = binding;
    codex_binding.provider = ProviderKind::Codex;
    assert!(!opencode_recovery_handle_matches(
        &runtime,
        &codex_binding,
        &mismatched
    ));
}
