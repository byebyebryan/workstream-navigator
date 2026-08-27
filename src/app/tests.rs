use std::fmt::Write as _;

use super::{
    HostRegistry, IntegrationLifecycle, ObserverProfile, Path, Presentation, StateRoot,
    cli::{
        Cli, Commands, is_d17_shell_gate_command, is_d17_shell_launch_helper_command,
        is_observer_command, is_provider_pane_command,
    },
    dispatch,
    model::AppError,
    observer::{finalize_native_trust, prepare_observer_activation_with_manager},
};
use crate::domain::RandomIdGenerator;
use clap::{CommandFactory as _, Parser as _};

fn fresh_root(temporary: &tempfile::TempDir) -> StateRoot {
    let path = temporary.path().join("state");
    let root = StateRoot::create(&path).unwrap();
    crate::state::fresh_create(&path, &RandomIdGenerator).unwrap();
    root
}

fn open_registry(root: &StateRoot) -> HostRegistry {
    crate::state::open_current_only(root)
        .unwrap()
        .into_host_registry()
        .unwrap()
}

#[test]
fn normal_d17_startup_creates_schema14_for_a_fresh_root() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let root = StateRoot::select(&state_path);

    let startup = dispatch::prepare_d17_navigator_state(&root).unwrap();
    assert!(matches!(startup, dispatch::D17NavigatorStartup::Ready));

    let state = crate::state::open_d17_current_only(&root).unwrap();
    assert_eq!(
        state.schema_version().unwrap(),
        crate::state::D17_HOST_SCHEMA_VERSION
    );
}

#[test]
fn normal_d17_startup_migrates_idle_schema13_before_presentation_creation() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let root = StateRoot::create(&state_path).unwrap();
    drop(crate::state::fresh_create(&state_path, &RandomIdGenerator).unwrap());

    let startup = dispatch::prepare_d17_navigator_state(&root).unwrap();
    assert!(matches!(startup, dispatch::D17NavigatorStartup::Ready));

    let state = crate::state::open_d17_current_only(&root).unwrap();
    assert_eq!(
        state.schema_version().unwrap(),
        crate::state::D17_HOST_SCHEMA_VERSION
    );
}

#[test]
fn normal_d17_startup_resumes_a_schema14_transition_before_opening_a_presentation() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let root = StateRoot::create(&state_path).unwrap();
    drop(crate::state::fresh_create(&state_path, &RandomIdGenerator).unwrap());
    let transition_path = state_path.join(crate::state::TRANSITION_LOCK_FILE);
    let transition_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&transition_path)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&transition_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    drop(transition_file);
    let lease = crate::state::acquire_transition_lease(&state_path).unwrap();
    let mut state = crate::state::open_cutover_transition(&root, &lease).unwrap();
    state.migrate_schema13_to14(&lease).unwrap();
    drop(state);
    drop(lease);

    let startup = dispatch::prepare_d17_navigator_state(&root).unwrap();
    assert!(matches!(startup, dispatch::D17NavigatorStartup::Ready));

    assert!(!transition_path.exists());
    let state = crate::state::open_d17_current_only(&root).unwrap();
    assert_eq!(
        state.schema_version().unwrap(),
        crate::state::D17_HOST_SCHEMA_VERSION
    );
}

#[test]
fn normal_d17_startup_resumes_a_schema13_transition_before_opening_a_presentation() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let root = StateRoot::create(&state_path).unwrap();
    drop(crate::state::fresh_create(&state_path, &RandomIdGenerator).unwrap());
    let transition_path = state_path.join(crate::state::TRANSITION_LOCK_FILE);
    let transition_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&transition_path)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&transition_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    drop(transition_file);

    let startup = dispatch::prepare_d17_navigator_state(&root).unwrap();
    assert!(matches!(startup, dispatch::D17NavigatorStartup::Ready));

    assert!(!transition_path.exists());
    let state = crate::state::open_d17_current_only(&root).unwrap();
    assert_eq!(
        state.schema_version().unwrap(),
        crate::state::D17_HOST_SCHEMA_VERSION
    );
}

#[test]
fn normal_d17_startup_refuses_to_migrate_beneath_a_live_d16_presentation() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let root = StateRoot::create(&state_path).unwrap();
    drop(crate::state::fresh_create(&state_path, &RandomIdGenerator).unwrap());
    let navigator = temporary.path().join("navigator-fixture");
    std::fs::write(&navigator, "#!/bin/sh\nexec sleep 60\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&navigator, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let presentation = Presentation::fresh_with_executable(&state_path, navigator);
    presentation.start().unwrap();

    let result = dispatch::prepare_d17_navigator_state(&root);
    presentation.close().unwrap();

    assert!(matches!(
        result,
        Err(AppError::D17CutoverNeedsPresentationClosed)
    ));
    let state = crate::state::open_current_only(&root).unwrap();
    assert_eq!(
        state.schema_version().unwrap(),
        crate::state::D16_HOST_SCHEMA_VERSION
    );
}

#[test]
fn owned_profile_hook_entrypoint_is_parseable_but_hidden() {
    let parsed = Cli::try_parse_from(["wsnav", "_hook"]);
    assert!(matches!(parsed.unwrap().command, Some(Commands::Hook)));
    assert!(Cli::try_parse_from(["wsnav", "hook"]).is_err());
}

#[test]
fn runtime_launch_barrier_is_parseable_but_hidden() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        "/state",
        "_runtime_launch",
        "00000000-0000-0000-0000-000000000001",
        "--",
        "codex",
        "--profile",
        "wsnav-observer",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Commands::RuntimeLaunch { program, .. })
            if program == ["codex", "--profile", "wsnav-observer"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
    ));
    assert!(
        Cli::try_parse_from([
            "wsnav",
            "_runtime_launch",
            "00000000-0000-0000-0000-000000000001"
        ])
        .is_err()
    );
}

#[test]
fn opencode_observer_entrypoint_is_hidden_and_typed() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "_opencode_observer_d16",
        "00000000-0000-0000-0000-000000000001",
        "generation",
        "4321",
        "root-session",
        "4242",
        "/project",
        "birth",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Commands::OpenCodeObserverD16 { port: 4321, .. })
    ));
    assert!(Cli::try_parse_from(["wsnav", "_opencode_observer"]).is_err());
}

#[test]
fn d17_navigator_entrypoint_is_hidden_and_typed() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "_navigator_d17",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Commands::NavigatorPaneD17 { .. })
    ));
    assert!(Cli::try_parse_from(["wsnav", "navigator_d17"]).is_err());
}

#[test]
fn d17_account_shell_entrypoints_are_hidden_typed_and_separate_from_provider_panes() {
    let gate = Cli::try_parse_from([
        "wsnav",
        "_d17_shell_gate",
        "--provider",
        "codex",
        "--shell-leader-pid",
        "42",
        "--",
        "--version",
    ])
    .unwrap();
    assert!(matches!(
        gate.command.as_ref(),
        Some(Commands::D17ShellGate { arguments, .. })
            if arguments == &[std::ffi::OsString::from("--version")]
    ));
    assert!(is_d17_shell_gate_command(gate.command.as_ref()));
    assert!(!is_d17_shell_launch_helper_command(gate.command.as_ref()));
    assert!(!is_observer_command(gate.command.as_ref()));
    assert!(!is_provider_pane_command(gate.command.as_ref()));

    let helper = Cli::try_parse_from([
        "wsnav",
        "_d17_launch_helper",
        "--capability",
        "a1.b2",
        "--provider",
        "opencode",
        "--",
    ])
    .unwrap();
    assert!(matches!(
        helper.command,
        Some(Commands::D17LaunchHelper { arguments, .. }) if arguments.is_empty()
    ));
    assert!(Cli::try_parse_from(["wsnav", "d17_shell_gate"]).is_err());
    assert!(Cli::try_parse_from(["wsnav", "d17_launch_helper"]).is_err());
}

#[test]
fn d17_gate_leaves_explicit_queries_unmanaged_before_opening_state() {
    let cli = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        "/missing/d17-state",
        "_d17_shell_gate",
        "--provider",
        "codex",
        "--shell-leader-pid",
        "42",
        "--",
        "--version",
    ])
    .unwrap();
    assert!(matches!(
        dispatch::execute(cli),
        Err(AppError::D17ShellGateUnmanaged)
    ));
}

#[test]
fn state_free_opencode_helpers_are_hidden_and_not_observer_or_provider_pane_commands() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "_opencode_serve_guardian",
        "opencode",
        "/project",
        "4321",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command.as_ref(),
        Some(Commands::OpenCodeServeGuardian { port: 4321, .. })
    ));
    assert!(!is_observer_command(parsed.command.as_ref()));
    assert!(!is_provider_pane_command(parsed.command.as_ref()));
    assert!(Cli::try_parse_from(["wsnav", "opencode_serve_guardian"]).is_err());
}

#[test]
fn provider_pane_helpers_are_local_and_silent() {
    let local = Cli::try_parse_from([
        "wsnav",
        "_provider_attach",
        "00000000-0000-0000-0000-000000000001",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--attempt-id",
        "00000000-0000-0000-0000-000000000002",
    ])
    .unwrap();
    assert!(is_provider_pane_command(local.command.as_ref()));
    assert!(!is_observer_command(local.command.as_ref()));
    assert!(Cli::try_parse_from(["wsnav", "_provider_remote_attach"]).is_err());

    let d17 = Cli::try_parse_from([
        "wsnav",
        "_provider_attach_d17",
        "00000000-0000-0000-0000-000000000001",
        "--expected-workstream-revision",
        "1",
        "--expected-runtime-id",
        "00000000-0000-0000-0000-000000000002",
        "--expected-runtime-revision",
        "1",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--attempt-id",
        "00000000-0000-0000-0000-000000000003",
    ])
    .unwrap();
    assert!(matches!(
        d17.command.as_ref(),
        Some(Commands::ProviderAttachD17 {
            expected_workstream_revision: 1,
            expected_runtime_revision: 1,
            ..
        })
    ));
    assert!(is_provider_pane_command(d17.command.as_ref()));
    assert!(!is_observer_command(d17.command.as_ref()));

    let review = Cli::try_parse_from(["wsnav", "_observer_review"]).unwrap();
    assert!(is_provider_pane_command(review.command.as_ref()));
    assert!(!is_observer_command(review.command.as_ref()));
    let shell = Cli::try_parse_from([
        "wsnav",
        "_presentation_shell",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--shell",
        "/bin/sh",
        "--cwd",
        "/tmp/project",
    ])
    .unwrap();
    assert!(is_provider_pane_command(shell.command.as_ref()));
    assert!(!is_observer_command(shell.command.as_ref()));
    assert!(Cli::try_parse_from(["wsnav", "_presentation_ssh_shell"]).is_err());
}

#[test]
fn observer_helpers_are_silent_but_return_a_failure_status_on_error() {
    let active = Cli::try_parse_from([
        "wsnav",
        "_opencode_observer_d16",
        "00000000-0000-0000-0000-000000000001",
        "generation",
        "4321",
        "root-session",
        "4242",
        "/project",
        "birth",
    ])
    .unwrap();
    assert!(is_observer_command(active.command.as_ref()));
    assert!(!is_provider_pane_command(active.command.as_ref()));

    let standby = Cli::try_parse_from([
        "wsnav",
        "_opencode_observer_standby",
        "00000000-0000-0000-0000-000000000001",
        "generation",
        "4321",
        "contract-build-a",
        "root-session",
        "4242",
        "/project",
        "birth",
    ])
    .unwrap();
    assert!(is_observer_command(standby.command.as_ref()));
    assert!(!is_provider_pane_command(standby.command.as_ref()));
}

#[test]
fn normal_help_and_parser_exclude_retired_surfaces() {
    let help = Cli::command().render_help().to_string();
    assert!(!help.contains("setup"));
    assert!(!help.contains("update-observer"));
    assert!(!help.contains("trust-observer"));
    assert!(!help.contains("remote"));
    assert!(help.contains("Start the Workstream's native provider"));
    assert!(help.contains("Recover a lost private Runtime through its exact native"));
    assert!(help.contains("durable host-registry record"));
    assert!(!help.contains("live private-tmux probe"));
    assert!(!help.contains("Register one existing Git project"));
    assert!(!help.contains("Create and start an independent Workstream"));
    for retired in [
        "register",
        "new-workstream",
        "register-remote",
        "host",
        "_remote",
        "_probe",
        "_attach",
    ] {
        assert!(
            Cli::try_parse_from(["wsnav", retired]).is_err(),
            "{retired}"
        );
    }
}

#[test]
fn native_trust_is_recorded_only_after_codex_completes_the_exact_review() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fresh_root(&temporary);
    let mut registry = open_registry(&root);
    let manager = ObserverProfile::new(
        temporary.path().join("codex-home"),
        temporary.path().join("bin/wsnav"),
        root.base(),
    );
    let ownership = manager.install("owner".to_owned(), None).unwrap();
    registry
        .record_codex_integration(ownership.clone(), IntegrationLifecycle::TrustPending)
        .unwrap();

    assert!(!finalize_native_trust(&mut registry, &manager, &ownership).unwrap());

    std::fs::write(
        manager.path(),
        format!(
            "{}{}",
            manager.rendered(),
            complete_native_trust_suffix(&manager)
        ),
    )
    .unwrap();

    assert!(finalize_native_trust(&mut registry, &manager, &ownership).unwrap());
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::Ready
    );
}

#[test]
fn navigator_activation_creates_one_owned_profile_and_requires_native_review() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fresh_root(&temporary);
    let mut registry = open_registry(&root);
    let manager = test_observer_profile(temporary.path(), &root);

    let activation =
        prepare_observer_activation_with_manager(&root, &mut registry, &manager).unwrap();

    assert_eq!(
        activation,
        super::observer::ObserverActivation::ReviewRequired
    );
    assert!(manager.path().is_file());
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::TrustPending
    );
}

#[test]
fn navigator_activation_reopens_missing_native_trust_without_setup_command() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fresh_root(&temporary);
    let mut registry = open_registry(&root);
    let manager = test_observer_profile(temporary.path(), &root);
    let ownership = manager.install("owner".to_owned(), None).unwrap();
    registry
        .record_codex_integration(ownership, IntegrationLifecycle::Ready)
        .unwrap();

    let activation =
        prepare_observer_activation_with_manager(&root, &mut registry, &manager).unwrap();

    assert_eq!(
        activation,
        super::observer::ObserverActivation::ReviewRequired
    );
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::TrustPending
    );
}

#[test]
fn navigator_activation_migrates_an_exact_prior_executable_before_review() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fresh_root(&temporary);
    let mut registry = open_registry(&root);
    let previous = ObserverProfile::new(
        temporary.path().join("codex-home"),
        temporary.path().join("bin/wsnav-old"),
        root.base(),
    );
    let ownership = previous.install("owner".to_owned(), None).unwrap();
    registry
        .record_codex_integration(ownership, IntegrationLifecycle::Ready)
        .unwrap();
    let manager = test_observer_profile(temporary.path(), &root);

    let activation =
        prepare_observer_activation_with_manager(&root, &mut registry, &manager).unwrap();

    assert_eq!(
        activation,
        super::observer::ObserverActivation::ReviewRequired
    );
    let integration = registry.codex_integration().unwrap().unwrap();
    assert_eq!(integration.lifecycle, IntegrationLifecycle::TrustPending);
    assert_eq!(
        integration.ownership.hook_executable,
        temporary.path().join("bin/wsnav")
    );
    assert_eq!(
        std::fs::read_to_string(manager.path()).unwrap(),
        manager.rendered()
    );
}

fn test_observer_profile(root: &Path, state_root: &StateRoot) -> ObserverProfile {
    ObserverProfile::new(
        root.join("codex-home"),
        root.join("bin/wsnav"),
        state_root.base(),
    )
}

fn complete_native_trust_suffix(manager: &ObserverProfile) -> String {
    let mut suffix = String::from("\n[hooks.state]\n");
    for hook in ["session_start", "user_prompt_submit", "stop", "session_end"] {
        let key =
            serde_json::to_string(&format!("{}:{hook}:0:0", manager.path().display())).unwrap();
        writeln!(
            suffix,
            "\n[hooks.state.{key}]\ntrusted_hash = \"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""
        )
        .unwrap();
    }
    suffix
}
