use super::{
    ActionError, IntegrationLifecycle, ObserverProfile, OsString, ProcessProbe, ProviderBinding,
    ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeProbe, StartOutcome, WorkstreamId,
    WorkstreamLifecycle, archive,
    attachment::inspect_opencode_prior_runtime,
    cleanup::{
        attachment_runtime_matches, fail_cleanup_unknown_opencode_session_creation,
        matches_recorded_runtime, observer_identity_matches, spawned_observer_identity_matches,
    },
    codex_recovery_program,
    creation::{IndependentStartSpec, start_independent_workstream_with},
    model::reconcile_observer_trust_with_manager,
    park, preflight_attachment_read_only,
    providers::managed_codex_environment,
    reconcile_lost_runtimes, restore, start,
    start::{
        CodexRecoveryEvidence, CodexThreadReader, backfill_live_runtime_provider_pid,
        codex_recovery_process_matches, opencode_recovery_handle_matches,
        reconcile_live_codex_recovery, runtime_launch_program,
    },
};
use crate::provider::codex::app_server::{AppServerError, ThreadMetadata};
use crate::provider::lifecycle::{LifecycleEvent, LifecycleObservation};
use crate::provider::names::NameState;
use crate::runtime::{
    LinuxProcessProbe, NativeLaunch, PrivateRuntime, ProcessCommand, RuntimePaths, SystemTmux,
};

use std::{
    cell::{Cell, RefCell},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

use rusqlite::{Connection, params};
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
    let mut state =
        crate::state::create_current(root.base(), &crate::domain::RandomIdGenerator).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            Path::new("/disposable/repository"),
            "repository",
            crate::domain::ProviderKind::Codex,
            &crate::domain::RandomIdGenerator,
        )
        .unwrap();
    let registry = state.into_host_registry().unwrap();
    (temporary, registry, workstream_id)
}

fn registry_for_provider(
    provider: ProviderKind,
) -> (
    tempfile::TempDir,
    crate::state::StateRoot,
    crate::state::HostRegistry,
    WorkstreamId,
) {
    let temporary = tempfile::tempdir().unwrap();
    let root = crate::state::StateRoot::select(temporary.path().join("state"));
    let mut state =
        crate::state::create_current(root.base(), &crate::domain::RandomIdGenerator).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            Path::new("/disposable/repository"),
            "repository",
            provider,
            &crate::domain::RandomIdGenerator,
        )
        .unwrap();
    let registry = state.into_host_registry().unwrap();
    (temporary, root, registry, workstream_id)
}

fn codex_recovery_fixture() -> (
    tempfile::TempDir,
    crate::state::StateRoot,
    crate::state::HostRegistry,
    WorkstreamId,
    crate::state::RuntimeRecord,
    crate::state::ProviderBinding,
    RuntimeProbe,
    Revision,
) {
    let (temporary, root, mut registry, workstream_id) = registry_for_provider(ProviderKind::Codex);
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 42, "birth-a")
        .unwrap();
    let runtime = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: runtime.cwd.to_string_lossy().into_owned(),
                native_session_id: "retained-session".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    let runtime = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    registry
        .mark_runtime_recovery_required(runtime.runtime_id, runtime.revision)
        .unwrap();
    let retained_binding = registry
        .retained_codex_binding_for_runtime(runtime.runtime_id)
        .unwrap()
        .unwrap();
    let runtime = registry
        .reserve_runtime_recovery_with_provider(workstream_id, ProviderKind::Codex)
        .unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 42, "birth-b")
        .unwrap();
    let runtime = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    let recovery_revision = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap()
        .revision;
    let probe = RuntimeProbe::Live {
        pane_id: "%1".to_owned(),
        pane_pid: 42,
        cwd: runtime.cwd.clone(),
        process_birth: Some("birth-b".to_owned()),
    };
    (
        temporary,
        root,
        registry,
        workstream_id,
        runtime,
        retained_binding,
        probe,
        recovery_revision,
    )
}

struct RecoveryProcessProbe {
    birth: Option<String>,
    command: Option<ProcessCommand>,
    events: Option<Rc<RefCell<Vec<&'static str>>>>,
}

impl ProcessProbe for RecoveryProcessProbe {
    fn process_birth(&self, _pid: u32) -> Option<String> {
        if let Some(events) = &self.events {
            events.borrow_mut().push("process_birth");
        }
        self.birth.clone()
    }

    fn process_command_checked(
        &self,
        _pid: u32,
    ) -> Result<Option<ProcessCommand>, crate::runtime::ProcessProbeError> {
        if let Some(events) = &self.events {
            events.borrow_mut().push("process_command");
        }
        Ok(self.command.clone())
    }
}

struct RecoveryThreadReader {
    expected_session: String,
    accept: bool,
    calls: Cell<usize>,
    events: Option<Rc<RefCell<Vec<&'static str>>>>,
}

impl CodexThreadReader for RecoveryThreadReader {
    fn read_thread(&self, thread_id: &str) -> Result<ThreadMetadata, AppServerError> {
        self.calls.set(self.calls.get() + 1);
        if let Some(events) = &self.events {
            events.borrow_mut().push("thread_read");
        }
        if self.accept && thread_id == self.expected_session {
            Ok(ThreadMetadata {
                name: Some("provider-owned name".to_owned()),
            })
        } else {
            Err(AppServerError::ThreadIdentityMismatch)
        }
    }
}

struct DisposableRuntimeGuard {
    socket: PathBuf,
    directory: PathBuf,
}

impl Drop for DisposableRuntimeGuard {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .env_remove("TMUX")
            .args(["-f", "/dev/null", "-S"])
            .arg(&self.socket)
            .arg("kill-server")
            .spawn()
            .and_then(std::process::Child::wait_with_output);
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn live_runtime_for_read_only_test(
    provider: ProviderKind,
) -> (
    tempfile::TempDir,
    crate::state::StateRoot,
    crate::state::RuntimeRecord,
    WorkstreamId,
    DisposableRuntimeGuard,
) {
    let (temporary, root, mut registry, workstream_id) = registry_for_provider(provider);
    let repository = temporary.path().join("repository");
    fs::create_dir(&repository).unwrap();
    let runtime_record = registry.reserve_runtime(workstream_id).unwrap();
    drop(registry);

    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime_paths = RuntimePaths::for_record(
        root.base(),
        runtime_record.runtime_id,
        &runtime_record.tmux_session,
    )
    .unwrap();
    let runtime = PrivateRuntime::new(&tmux, &process_probe, runtime_paths.clone());
    runtime
        .start(&NativeLaunch {
            cwd: repository.clone(),
            program: vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("exec sleep 60"),
            ],
            environment: std::collections::BTreeMap::new(),
        })
        .unwrap();
    let guard = DisposableRuntimeGuard {
        socket: runtime_paths.socket.clone(),
        directory: runtime_paths.directory.clone(),
    };

    let deadline = Instant::now() + Duration::from_secs(2);
    let (provider_pid, process_birth) = loop {
        if let Ok(RuntimeProbe::Live {
            pane_pid,
            process_birth: Some(process_birth),
            ..
        }) = runtime.probe()
        {
            break (pane_pid, process_birth);
        }
        assert!(
            Instant::now() < deadline,
            "fake Runtime did not become live"
        );
        thread::sleep(Duration::from_millis(10));
    };

    let mut registry = crate::state::open_current(&root)
        .unwrap()
        .into_host_registry()
        .unwrap();
    registry
        .record_runtime_process_identity(
            runtime_record.runtime_id,
            runtime_record.revision,
            provider_pid,
            &process_birth,
        )
        .unwrap();
    drop(registry);
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute(
            "UPDATE runtimes SET cwd = ?1, lifecycle = 'idle' WHERE runtime_id = ?2",
            params![
                repository.to_string_lossy().to_string(),
                runtime_record.runtime_id.to_string(),
            ],
        )
        .unwrap();
    drop(connection);
    let record = crate::state::open_current(&root)
        .unwrap()
        .into_host_registry()
        .unwrap()
        .runtime_by_id(runtime_record.runtime_id)
        .unwrap()
        .unwrap();
    (temporary, root, record, workstream_id, guard)
}

fn host_database_bytes(root: &crate::state::StateRoot) -> [Option<Vec<u8>>; 2] {
    ["host.sqlite", "host.sqlite-wal"].map(|name| fs::read(root.base().join(name)).ok())
}

fn workstream_revisions(
    root: &crate::state::StateRoot,
    workstream_id: WorkstreamId,
) -> (Revision, Revision) {
    let registry = crate::state::open_current(root)
        .unwrap()
        .into_host_registry()
        .unwrap();
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    (overview.revision, overview.runtime.unwrap().revision)
}

#[test]
fn completed_native_review_promotes_pending_observer_before_a_managed_action() {
    let temporary = tempfile::tempdir().unwrap();
    let root = crate::state::StateRoot::select(temporary.path().join("state"));
    let mut registry = crate::state::create_current(root.base(), &crate::domain::RandomIdGenerator)
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
    let (_temporary, _root, mut registry, workstream_id) =
        registry_for_provider(ProviderKind::OpenCode);
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();

    assert!(matches!(
        fail_cleanup_unknown_opencode_session_creation(&mut registry, &prepared),
        ActionError::OpenCodeSessionCreationExternalEffectUnknown
    ));
    assert_eq!(
        registry
            .runtime_for_workstream(workstream_id)
            .unwrap()
            .unwrap()
            .status,
        crate::domain::RuntimeStatus::Unknown
    );
}

#[test]
fn abandoned_prepared_opencode_creation_on_missing_runtime_requires_recovery() {
    let (_temporary, root, mut registry, workstream_id) =
        registry_for_provider(ProviderKind::OpenCode);
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();

    assert!(matches!(
        inspect_opencode_prior_runtime(&root, &mut registry, workstream_id),
        Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode
        ))
    ));

    let current_runtime = registry
        .runtime_for_workstream(workstream_id)
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
        .find(|overview| overview.workstream_id == workstream_id)
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
        start(&root, &mut registry, workstream_id, Some(overview.revision),),
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
fn exact_live_codex_recovery_reconciles_the_retained_binding_only() {
    let (
        _temporary,
        _root,
        mut registry,
        workstream_id,
        runtime,
        binding,
        probe,
        workstream_revision,
    ) = codex_recovery_fixture();
    let events = Rc::new(RefCell::new(Vec::new()));
    let command = ProcessCommand {
        executable: PathBuf::from("/usr/local/bin/codex"),
        argv: codex_recovery_program(&runtime.cwd, Some(&binding)),
    };
    let process_probe = RecoveryProcessProbe {
        birth: Some("birth-b".to_owned()),
        command: Some(command),
        events: Some(events.clone()),
    };
    let reader = RecoveryThreadReader {
        expected_session: binding.native_session_id.native_id().to_owned(),
        accept: true,
        calls: Cell::new(0),
        events: Some(events.clone()),
    };
    let overview_before = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    let outcome = reconcile_live_codex_recovery(
        &mut registry,
        workstream_id,
        workstream_revision,
        &runtime,
        &probe,
        CodexRecoveryEvidence {
            process_probe: &process_probe,
            thread_reader: &reader,
            revalidate: |_: &crate::state::RuntimeRecord,
                         probe: &RuntimeProbe,
                         _: &crate::state::ProviderBinding| {
                events.borrow_mut().push("revalidate");
                Ok(probe.clone())
            },
        },
    )
    .unwrap();

    assert_eq!(outcome, StartOutcome::Reconciled);
    assert_eq!(reader.calls.get(), 1);
    assert_eq!(
        events.borrow().as_slice(),
        [
            "process_birth",
            "process_command",
            "thread_read",
            "revalidate"
        ]
    );
    let runtime_after = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    assert_eq!(runtime_after, runtime);
    let overview_after = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .unwrap();
    assert_eq!(overview_after.lifecycle, WorkstreamLifecycle::Open);
    assert_eq!(overview_after.revision, workstream_revision.next());
    assert_eq!(
        overview_after.last_activity_sequence,
        overview_before.last_activity_sequence + 1
    );
    let binding_after = registry
        .binding_for_runtime(runtime.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(binding_after.runtime_generation, runtime.tmux_generation);
    assert_eq!(binding_after.revision, binding.revision.next());
    assert_eq!(binding_after.native_session_id, binding.native_session_id);
    assert_eq!(binding_after.start_source, binding.start_source);
    assert_eq!(
        binding_after.last_settled_turn_id,
        binding.last_settled_turn_id
    );
    assert_eq!(
        binding_after.observed_thread_name,
        binding.observed_thread_name
    );
    assert_eq!(binding_after.name_state, binding.name_state);
}

#[test]
fn codex_recovery_executable_accepts_only_the_live_or_linux_deleted_name() {
    let (
        _temporary,
        _root,
        _registry,
        _workstream_id,
        runtime,
        binding,
        _probe,
        _workstream_revision,
    ) = codex_recovery_fixture();
    let argv = codex_recovery_program(&runtime.cwd, Some(&binding));

    for executable in ["/usr/bin/codex", "/usr/bin/codex (deleted)"] {
        let command = ProcessCommand {
            executable: PathBuf::from(executable),
            argv: argv.clone(),
        };
        let expected = executable.ends_with("/codex") || cfg!(target_os = "linux");
        assert_eq!(
            codex_recovery_process_matches(&command, &runtime.cwd, &binding),
            expected,
            "unexpected executable classification: {executable}"
        );
    }

    for executable in [
        "codex",
        "/usr/bin/other",
        "/usr/bin/codex.old",
        "/usr/bin/codex (deleted) (deleted)",
    ] {
        let command = ProcessCommand {
            executable: PathBuf::from(executable),
            argv: argv.clone(),
        };
        assert!(
            !codex_recovery_process_matches(&command, &runtime.cwd, &binding),
            "unexpected executable acceptance: {executable}"
        );
    }

    let command = ProcessCommand {
        executable: PathBuf::from("/usr/bin/codex"),
        argv: codex_recovery_program(Path::new("/different/cwd"), Some(&binding)),
    };
    assert!(!codex_recovery_process_matches(
        &command,
        &runtime.cwd,
        &binding
    ));
}

#[test]
fn codex_live_recovery_refuses_process_or_thread_mismatch_without_state_change() {
    for mismatch in ["pid", "birth", "cwd", "executable", "argv", "thread"] {
        let (_temporary, _root, mut registry, workstream_id, runtime, binding, probe, revision) =
            codex_recovery_fixture();
        let mut probe = probe;
        match mismatch {
            "pid" => {
                if let RuntimeProbe::Live { pane_pid, .. } = &mut probe {
                    *pane_pid = 43;
                }
            }
            "cwd" => {
                if let RuntimeProbe::Live { cwd, .. } = &mut probe {
                    *cwd = PathBuf::from("/disposable/other-repository");
                }
            }
            _ => {}
        }
        let mut argv = codex_recovery_program(&runtime.cwd, Some(&binding));
        if mismatch == "argv" {
            argv.push("unexpected".into());
        }
        let executable = if mismatch == "executable" {
            PathBuf::from("/usr/local/bin/other")
        } else {
            PathBuf::from("/usr/local/bin/codex")
        };
        let process_probe = RecoveryProcessProbe {
            birth: Some(if mismatch == "birth" {
                "different-birth".to_owned()
            } else {
                "birth-b".to_owned()
            }),
            command: Some(ProcessCommand { executable, argv }),
            events: None,
        };
        let reader = RecoveryThreadReader {
            expected_session: binding.native_session_id.native_id().to_owned(),
            accept: mismatch != "thread",
            calls: Cell::new(0),
            events: None,
        };
        let runtime_before = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
        let error = reconcile_live_codex_recovery(
            &mut registry,
            workstream_id,
            revision,
            &runtime,
            &probe,
            CodexRecoveryEvidence {
                process_probe: &process_probe,
                thread_reader: &reader,
                revalidate: |_: &crate::state::RuntimeRecord,
                             probe: &RuntimeProbe,
                             _: &crate::state::ProviderBinding| {
                    Ok(probe.clone())
                },
            },
        )
        .unwrap_err();
        assert!(
            matches!(error, ActionError::RuntimeProbeAmbiguous)
                || matches!(
                    error,
                    ActionError::AppServer(AppServerError::ThreadIdentityMismatch)
                )
        );
        assert_eq!(
            registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap(),
            runtime_before
        );
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
        assert_eq!(overview.revision, revision);
        assert!(
            registry
                .codex_recovery_binding(
                    workstream_id,
                    revision,
                    runtime.runtime_id,
                    runtime.revision,
                    &runtime.tmux_generation,
                )
                .is_ok()
        );
        assert_eq!(reader.calls.get(), usize::from(mismatch == "thread"));
    }
}

#[test]
fn codex_live_recovery_reprobes_after_thread_read_and_rejects_changed_evidence() {
    let (_temporary, root, mut registry, workstream_id, runtime, binding, probe, revision) =
        codex_recovery_fixture();
    let events = Rc::new(RefCell::new(Vec::new()));
    let process_probe = RecoveryProcessProbe {
        birth: Some("birth-b".to_owned()),
        command: Some(ProcessCommand {
            executable: PathBuf::from("/usr/local/bin/codex"),
            argv: codex_recovery_program(&runtime.cwd, Some(&binding)),
        }),
        events: Some(events.clone()),
    };
    let reader = RecoveryThreadReader {
        expected_session: binding.native_session_id.native_id().to_owned(),
        accept: true,
        calls: Cell::new(0),
        events: Some(events.clone()),
    };
    let runtime_before = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    let state_before = host_database_bytes(&root);
    let error = reconcile_live_codex_recovery(
        &mut registry,
        workstream_id,
        revision,
        &runtime,
        &probe,
        CodexRecoveryEvidence {
            process_probe: &process_probe,
            thread_reader: &reader,
            revalidate: |_: &crate::state::RuntimeRecord,
                         initial_probe: &RuntimeProbe,
                         _: &crate::state::ProviderBinding| {
                events.borrow_mut().push("revalidate");
                let RuntimeProbe::Live {
                    pane_id,
                    pane_pid,
                    cwd,
                    process_birth,
                } = initial_probe
                else {
                    panic!("fixture must be live");
                };
                Ok(RuntimeProbe::Live {
                    pane_id: format!("{pane_id}-changed"),
                    pane_pid: *pane_pid,
                    cwd: cwd.clone(),
                    process_birth: process_birth.clone(),
                })
            },
        },
    )
    .unwrap_err();

    assert!(matches!(error, ActionError::RuntimeProbeAmbiguous));
    assert_eq!(reader.calls.get(), 1);
    assert_eq!(
        events.borrow().as_slice(),
        [
            "process_birth",
            "process_command",
            "thread_read",
            "revalidate"
        ]
    );
    assert_eq!(
        registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap(),
        runtime_before
    );
    assert_eq!(
        registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap()
            .lifecycle,
        WorkstreamLifecycle::RecoveryRequired
    );
    assert_eq!(host_database_bytes(&root), state_before);
}

fn mutate_codex_recovery_fence(root: &crate::state::StateRoot, runtime_id: RuntimeId, case: &str) {
    let connection = Connection::open(root.host_database_path()).unwrap();
    let runtime_id = runtime_id.to_string();
    match case {
        "unbound" => connection
            .execute(
                "DELETE FROM provider_bindings WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "provider" => connection
            .execute(
                "UPDATE runtimes SET provider = 'opencode' WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "session" => connection
            .execute(
                "UPDATE provider_bindings SET native_session_id = 'other-session'
                 WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "generation" => connection
            .execute(
                "UPDATE runtimes SET tmux_generation = 'other-generation'
                 WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "binding-generation" => connection
            .execute(
                "UPDATE provider_bindings SET runtime_generation = 'current-generation'
                 WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "runtime-status" => connection
            .execute(
                "UPDATE runtimes SET lifecycle = 'idle' WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "runtime-revision" => connection
            .execute(
                "UPDATE runtimes SET revision = revision + 1 WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        "workstream-status" => connection
            .execute(
                "UPDATE workstreams SET lifecycle = 'open'
                 WHERE workstream_id = (SELECT workstream_id FROM runtimes WHERE runtime_id = ?1)",
                [&runtime_id],
            )
            .unwrap(),
        "archived" => connection
            .execute(
                "UPDATE workstreams SET archived_at_millis = 1
                 WHERE workstream_id = (SELECT workstream_id FROM runtimes WHERE runtime_id = ?1)",
                [&runtime_id],
            )
            .unwrap(),
        "workstream-revision" => connection
            .execute(
                "UPDATE workstreams SET revision = revision + 1
                 WHERE workstream_id = (SELECT workstream_id FROM runtimes WHERE runtime_id = ?1)",
                [&runtime_id],
            )
            .unwrap(),
        "binding-revision" => connection
            .execute(
                "UPDATE provider_bindings SET revision = revision + 1
                 WHERE runtime_id = ?1",
                [&runtime_id],
            )
            .unwrap(),
        _ => panic!("unknown recovery fence case: {case}"),
    };
}

#[test]
fn codex_recovery_state_fences_fail_closed_without_partial_mutation() {
    for case in [
        "unbound",
        "provider",
        "session",
        "generation",
        "binding-generation",
        "runtime-status",
        "runtime-revision",
        "workstream-status",
        "archived",
        "workstream-revision",
        "binding-revision",
    ] {
        let (
            _temporary,
            root,
            mut registry,
            workstream_id,
            runtime,
            binding,
            _probe,
            workstream_revision,
        ) = codex_recovery_fixture();
        mutate_codex_recovery_fence(&root, runtime.runtime_id, case);
        let before = host_database_bytes(&root);
        let result = registry.reconcile_codex_recovery(
            workstream_id,
            workstream_revision,
            runtime.runtime_id,
            runtime.revision,
            &runtime.tmux_generation,
            &binding,
        );
        assert!(result.is_err(), "fence unexpectedly accepted: {case}");
        assert_eq!(host_database_bytes(&root), before, "mutated state: {case}");
    }
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
    let (_temporary, root, mut registry, workstream_id) =
        registry_for_provider(ProviderKind::Codex);
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
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
#[cfg(unix)]
fn read_only_codex_preflight_preserves_state_on_success_and_topology_refusal() {
    if Command::new("tmux")
        .arg("-V")
        .spawn()
        .and_then(std::process::Child::wait_with_output)
        .is_err()
    {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let (_temporary, root, record, workstream_id, _runtime_guard) =
        live_runtime_for_read_only_test(ProviderKind::Codex);
    let before_success_bytes = host_database_bytes(&root);
    let before_success_revisions = workstream_revisions(&root, workstream_id);
    let registry = crate::state::open_current(&root)
        .unwrap()
        .into_host_registry()
        .unwrap();
    assert_eq!(
        preflight_attachment_read_only(&root, &registry, workstream_id)
            .unwrap()
            .runtime_id,
        record.runtime_id
    );
    drop(registry);
    assert_eq!(host_database_bytes(&root), before_success_bytes);
    let after_success_revisions = workstream_revisions(&root, workstream_id);
    assert_eq!(after_success_revisions, before_success_revisions);

    let paths =
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session).unwrap();
    let extra = Command::new("tmux")
        .env_remove("TMUX")
        .args(["-f", "/dev/null", "-S"])
        .arg(&paths.socket)
        .args(["split-window", "-d", "-t"])
        .arg(format!("{}:provider", record.tmux_session))
        .args(["/bin/sleep", "60"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(std::process::Child::wait_with_output)
        .unwrap();
    assert!(extra.status.success(), "tmux failed: {:?}", extra.stderr);

    let before_refusal_bytes = host_database_bytes(&root);
    let before_refusal_revisions = workstream_revisions(&root, workstream_id);
    let registry = crate::state::open_current(&root)
        .unwrap()
        .into_host_registry()
        .unwrap();
    assert!(preflight_attachment_read_only(&root, &registry, workstream_id).is_err());
    drop(registry);
    assert_eq!(host_database_bytes(&root), before_refusal_bytes);
    let after_refusal_revisions = workstream_revisions(&root, workstream_id);
    assert_eq!(after_refusal_revisions, before_refusal_revisions);
}

#[test]
#[cfg(unix)]
fn read_only_opencode_refusal_preserves_missing_provider_handle() {
    if Command::new("tmux")
        .arg("-V")
        .spawn()
        .and_then(std::process::Child::wait_with_output)
        .is_err()
    {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let (_temporary, root, record, workstream_id, _runtime_guard) =
        live_runtime_for_read_only_test(ProviderKind::OpenCode);
    let paths =
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session).unwrap();
    assert!(paths.socket.exists(), "fake OpenCode Runtime did not start");

    let before_bytes = host_database_bytes(&root);
    let (before_handle, before_revisions) = {
        let registry = crate::state::open_current(&root)
            .unwrap()
            .into_host_registry()
            .unwrap();
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        (
            registry.opencode_runtime_handle(record.runtime_id).unwrap(),
            (overview.revision, overview.runtime.unwrap().revision),
        )
    };
    assert!(before_handle.is_none());
    let registry = crate::state::open_current(&root)
        .unwrap()
        .into_host_registry()
        .unwrap();
    assert!(preflight_attachment_read_only(&root, &registry, workstream_id).is_err());
    drop(registry);
    let (after_handle, after_revisions) = {
        let registry = crate::state::open_current(&root)
            .unwrap()
            .into_host_registry()
            .unwrap();
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        (
            registry.opencode_runtime_handle(record.runtime_id).unwrap(),
            (overview.revision, overview.runtime.unwrap().revision),
        )
    };
    assert_eq!(after_handle, before_handle);
    assert_eq!(after_revisions, before_revisions);
    assert_eq!(host_database_bytes(&root), before_bytes);
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
    let (_temporary, root, mut registry, workstream_id) =
        registry_for_provider(ProviderKind::Codex);
    let runtime = registry.reserve_runtime(workstream_id).unwrap();
    drop(registry);
    let connection = rusqlite::Connection::open(root.host_database_path()).unwrap();
    connection
        .execute(
            "UPDATE runtimes SET process_birth = 'birth-a' WHERE runtime_id = ?1",
            [runtime.runtime_id.to_string()],
        )
        .unwrap();
    drop(connection);
    let mut registry = crate::state::open_current(&root)
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
    let mut state =
        crate::state::create_current(root.base(), &crate::domain::RandomIdGenerator).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &repository,
            "repository",
            crate::domain::ProviderKind::Codex,
            &crate::domain::RandomIdGenerator,
        )
        .unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let created = registry
        .create_independent_workstream(
            "independent-system-git",
            workstream_id,
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
