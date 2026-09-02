use super::{
    StateRoot,
    cli::{
        Cli, Commands, is_observer_command, is_observer_setup_command,
        is_presentation_mouse_command, is_provider_pane_command, is_shell_gate_command,
        is_shell_launch_helper_command,
    },
    dispatch,
    model::AppError,
};
use crate::{
    domain::RandomIdGenerator,
    provider::codex::profile::{OBSERVER_PROFILE_SCHEMA_VERSION, ProfileOwnership},
    state::IntegrationLifecycle,
};
use clap::{CommandFactory as _, Parser as _};

fn current_workstream_fixture() -> (tempfile::TempDir, StateRoot, crate::domain::WorkstreamId) {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let checkout = temporary.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let mut state = crate::state::create_current(&state_path, &RandomIdGenerator).unwrap();
    let (_, workstream_id) = state
        .seed_test_workstream(
            &checkout,
            "checkout",
            crate::domain::ProviderKind::Codex,
            &RandomIdGenerator,
        )
        .unwrap();
    drop(state);
    let root = StateRoot::select(&state_path);
    (temporary, root, workstream_id)
}

#[test]
fn retained_observer_commands_use_the_schema15_boundary() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    drop(crate::state::create_current(&state_path, &RandomIdGenerator).unwrap());

    let doctor = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        state_path.to_str().unwrap(),
        "doctor",
    ])
    .unwrap();
    assert!(dispatch::execute(doctor).is_ok());

    let remove = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        state_path.to_str().unwrap(),
        "remove-observer",
    ])
    .unwrap();
    assert!(matches!(
        dispatch::execute(remove),
        Err(AppError::ObserverNotInstalled)
    ));
}

#[test]
fn provider_owned_naming_has_no_wsnav_command() {
    assert!(
        Cli::try_parse_from(["wsnav", "rename", "workstream", "1", "provider-owned name"]).is_err()
    );
}

#[test]
fn direct_codex_start_refuses_unready_observer_without_mutating_schema15_state() {
    let (temporary, root, workstream_id) = current_workstream_fixture();
    let codex_home = temporary.path().join("codex");
    std::fs::create_dir(&codex_home).unwrap();
    let profile = codex_home.join("wsnav-observer.config.toml");
    let ownership = ProfileOwnership {
        canonical_path: profile,
        owner_id: "test-owner".to_owned(),
        profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
        hook_executable: std::env::current_exe().unwrap(),
        content_hash: "test-hash".to_owned(),
    };
    let state = crate::state::open_current(&root).unwrap();
    let mut registry = state.into_host_registry().unwrap();
    let expected_integration = registry
        .record_codex_integration(ownership, IntegrationLifecycle::TrustPending)
        .unwrap();
    drop(registry);
    let start = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        root.base().to_str().unwrap(),
        "start",
        &workstream_id.to_string(),
    ])
    .unwrap();

    assert!(matches!(
        dispatch::execute(start),
        Err(AppError::ObserverReadinessRequired)
    ));
    let state = crate::state::open_current(&root).unwrap();
    assert_eq!(
        state.codex_integration().unwrap(),
        Some(expected_integration)
    );
    assert!(
        state
            .onboarding_workstream_projections()
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .into_host_registry()
            .unwrap()
            .runtime_for_workstream(workstream_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn normal_startup_creates_schema15_for_a_fresh_root() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    let root = StateRoot::select(&state_path);

    let startup = dispatch::prepare_navigator_state(&root).unwrap();
    assert!(matches!(startup, dispatch::NavigatorStartup::Ready));

    let state = crate::state::open_current(&root).unwrap();
    assert_eq!(
        state.schema_version().unwrap(),
        crate::state::HOST_SCHEMA_VERSION
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
fn presentation_mouse_helper_is_not_success_suppressed() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        "/state",
        "_presentation_mouse",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--target-pane",
        "%1",
        "--client-name",
        "/dev/pts/1",
    ])
    .unwrap();
    assert!(is_presentation_mouse_command(parsed.command.as_ref()));
    assert!(!is_provider_pane_command(parsed.command.as_ref()));
}

#[test]
fn opencode_observer_entrypoint_is_hidden_and_typed() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "_opencode_observer",
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
        Some(Commands::OpenCodeObserver { port: 4321, .. })
    ));
    assert!(Cli::try_parse_from(["wsnav", "_opencode_observer"]).is_err());
}

#[test]
fn navigator_entrypoint_is_hidden_and_typed() {
    let parsed = Cli::try_parse_from([
        "wsnav",
        "_navigator",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Some(Commands::NavigatorPane { .. })
    ));
    assert!(Cli::try_parse_from(["wsnav", "navigator_d17"]).is_err());
}

#[test]
fn account_shell_entrypoints_are_hidden_typed_and_separate_from_provider_panes() {
    let gate = Cli::try_parse_from([
        "wsnav",
        "_shell_gate",
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
        Some(Commands::ShellGate { arguments, .. })
            if arguments == &[std::ffi::OsString::from("--version")]
    ));
    assert!(is_shell_gate_command(gate.command.as_ref()));
    assert!(!is_shell_launch_helper_command(gate.command.as_ref()));
    assert!(!is_observer_command(gate.command.as_ref()));
    assert!(!is_provider_pane_command(gate.command.as_ref()));

    let helper = Cli::try_parse_from([
        "wsnav",
        "_launch_helper",
        "--capability",
        "a1.b2",
        "--provider",
        "opencode",
        "--",
    ])
    .unwrap();
    assert!(matches!(
        helper.command,
        Some(Commands::LaunchHelper { arguments, .. }) if arguments.is_empty()
    ));
    assert!(Cli::try_parse_from(["wsnav", "shell_gate"]).is_err());
    assert!(Cli::try_parse_from(["wsnav", "launch_helper"]).is_err());
}

#[test]
fn gate_leaves_explicit_queries_unmanaged_before_opening_state() {
    let cli = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        "/missing/current-state",
        "_shell_gate",
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
        Err(AppError::ShellGateUnmanaged)
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
fn retired_provider_and_presentation_helpers_are_unparseable_but_control_is_typed() {
    assert!(Cli::try_parse_from(["wsnav", "_navigator"]).is_err());
    assert!(Cli::try_parse_from(["wsnav", "_provider_attach"]).is_err());
    assert!(Cli::try_parse_from(["wsnav", "_provider_remote_attach"]).is_err());

    let current = Cli::try_parse_from([
        "wsnav",
        "_provider_attach",
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
        current.command.as_ref(),
        Some(Commands::ProviderAttach {
            expected_workstream_revision: 1,
            expected_runtime_revision: 1,
            ..
        })
    ));
    assert!(is_provider_pane_command(current.command.as_ref()));
    assert!(!is_observer_command(current.command.as_ref()));

    assert!(Cli::try_parse_from(["wsnav", "_observer_review"]).is_err());
    assert!(Cli::try_parse_from(["wsnav", "_presentation_shell"]).is_err());
    assert!(Cli::try_parse_from(["wsnav", "_presentation_ssh_shell"]).is_err());

    let control = Cli::try_parse_from([
        "wsnav",
        "_presentation_control",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--action",
        "focus-next",
        "--source-pane",
        "%0",
        "--client-name",
        "/dev/pts/9",
    ])
    .unwrap();
    assert!(matches!(
        control.command.as_ref(),
        Some(Commands::PresentationControl { action, .. }) if action == "focus-next"
    ));
    assert!(is_provider_pane_command(control.command.as_ref()));

    let setup = Cli::try_parse_from([
        "wsnav",
        "_observer_setup",
        "--shell-leader-pid",
        "42",
        "--consent",
    ])
    .unwrap();
    assert!(is_observer_setup_command(setup.command.as_ref()));
    assert!(!is_provider_pane_command(setup.command.as_ref()));
}

#[test]
fn observer_setup_decline_returns_failure_without_opening_or_mutating_state() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("state");
    drop(crate::state::create_current(&state_path, &RandomIdGenerator).unwrap());
    let cli = Cli {
        state_root: Some(state_path.clone()),
        command: Some(Commands::ObserverSetup {
            shell_leader_pid: 42,
            consent: false,
        }),
    };
    assert!(matches!(
        dispatch::execute(cli),
        Err(AppError::ShellControlUnavailable)
    ));
    let state = crate::state::open_current(&StateRoot::select(&state_path)).unwrap();
    assert!(state.codex_integration().unwrap().is_none());
    assert!(
        state
            .onboarding_workstream_projections()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn observer_helpers_are_silent_but_return_a_failure_status_on_error() {
    let active = Cli::try_parse_from([
        "wsnav",
        "_opencode_observer",
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
