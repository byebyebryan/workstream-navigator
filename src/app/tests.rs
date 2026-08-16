use std::fmt::Write as _;

use super::{
    HostRegistry, IntegrationLifecycle, ObserverProfile, Path, RuntimeId, StateRoot,
    cli::{Cli, Commands, HostCommands, is_provider_surface_command},
    dispatch::{self, should_prepare_codex_observer},
    local::codex_launch_program,
    model::AppError,
    observer::{
        ObserverActivation, finalize_native_trust, prepare_observer_activation_with_manager,
    },
};
use crate::provider::names::NameState;
use clap::{CommandFactory as _, Parser as _};

#[test]
fn resuming_uses_the_exact_bound_native_session() {
    let binding = crate::state::ProviderBinding {
        runtime_id: RuntimeId::new(),
        runtime_generation: "generation-a".to_owned(),
        provider: crate::domain::ProviderKind::Codex,
        native_session_id: crate::domain::ProviderSessionId::codex("exact-session").unwrap(),
        start_source: "startup".to_owned(),
        last_settled_turn_id: Some("settled-turn".to_owned()),
        observed_thread_name: None,
        name_state: NameState::Unavailable,
        predecessor_native_session_id: None,
        predecessor_effective_name: None,
        revision: crate::domain::Revision::INITIAL,
    };
    let program = codex_launch_program(Path::new("/checkout"), Some(&binding));

    assert!(program.ends_with(&["resume".into(), "exact-session".into()]));
}

#[test]
fn fresh_runtime_does_not_invent_a_session_id() {
    let program = codex_launch_program(Path::new("/checkout"), None);

    assert!(!program.iter().any(|argument| argument == "resume"));
}

#[test]
fn owned_profile_hook_entrypoint_is_parseable_but_hidden() {
    let parsed = Cli::try_parse_from(["wsnav", "_hook"]);
    assert!(matches!(parsed.unwrap().command, Some(Commands::Hook)));
    assert!(Cli::try_parse_from(["wsnav", "hook"]).is_err());
}

#[test]
fn release_probe_entrypoint_is_parseable_but_hidden() {
    let parsed = Cli::try_parse_from(["wsnav", "_probe"]);
    assert!(matches!(parsed.unwrap().command, Some(Commands::Probe)));
    assert!(Cli::try_parse_from(["wsnav", "probe"]).is_err());
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
    assert!(Cli::try_parse_from(["wsnav", "opencode-observer"]).is_err());
}

#[test]
fn opencode_guardian_is_state_free_hidden_and_not_provider_surface() {
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
    assert!(!is_provider_surface_command(parsed.command.as_ref()));
    assert!(Cli::try_parse_from(["wsnav", "opencode_serve_guardian"]).is_err());
    let barrier = Cli::try_parse_from([
        "wsnav",
        "_opencode_serve_barrier",
        "opencode",
        "/project",
        "4321",
    ])
    .unwrap();
    assert!(matches!(
        barrier.command.as_ref(),
        Some(Commands::OpenCodeServeBarrier { port: 4321, .. })
    ));
    assert!(!is_provider_surface_command(barrier.command.as_ref()));
}

#[test]
#[allow(clippy::too_many_lines)]
fn provider_surface_helpers_are_silent_cli_commands() {
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
    assert!(is_provider_surface_command(local.command.as_ref()));

    let remote = Cli::try_parse_from([
        "wsnav",
        "_provider_remote_attach",
        "snap",
        "00000000-0000-0000-0000-000000000001",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--attempt-id",
        "00000000-0000-0000-0000-000000000002",
    ])
    .unwrap();
    assert!(is_provider_surface_command(remote.command.as_ref()));

    let launch = Cli::try_parse_from([
        "wsnav",
        "_runtime_launch",
        "00000000-0000-0000-0000-000000000001",
        "--",
        "codex",
    ])
    .unwrap();
    assert!(is_provider_surface_command(launch.command.as_ref()));

    let review = Cli::try_parse_from(["wsnav", "_observer_review"]).unwrap();
    assert!(is_provider_surface_command(review.command.as_ref()));

    let control = Cli::try_parse_from([
        "wsnav",
        "_presentation_control",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--action",
        "literal-c-b",
        "--source-pane",
        "%1",
        "--client-name",
        "/dev/pts/9",
    ])
    .unwrap();
    assert!(matches!(
        control.command.as_ref(),
        Some(Commands::PresentationControl { action, source_pane, client_name, .. })
            if action == "literal-c-b" && source_pane == "%1" && client_name == "/dev/pts/9"
    ));
    assert!(is_provider_surface_command(control.command.as_ref()));

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
    assert!(matches!(
        shell.command.as_ref(),
        Some(Commands::PresentationShell { shell, cwd, .. })
            if shell == &std::path::PathBuf::from("/bin/sh")
                && cwd == &std::path::PathBuf::from("/tmp/project")
    ));
    assert!(is_provider_surface_command(shell.command.as_ref()));

    let remote_shell = Cli::try_parse_from([
        "wsnav",
        "_presentation_ssh_shell",
        "--presentation-socket",
        "/state/presentation/presentation-0123456789ab/tmux.sock",
        "--presentation-session",
        "wsnav-presentation-0123456789ab",
        "--destination",
        "snap",
        "--executable",
        "/home/user/.local/bin/wsnav",
        "--workstream-id",
        "00000000-0000-0000-0000-000000000001",
    ])
    .unwrap();
    assert!(matches!(
        remote_shell.command.as_ref(),
        Some(Commands::PresentationRemoteShell {
            destination,
            executable,
            workstream_id,
            ..
        }) if destination == "snap"
            && executable == &std::path::PathBuf::from("/home/user/.local/bin/wsnav")
            && workstream_id == "00000000-0000-0000-0000-000000000001"
    ));
    assert!(is_provider_surface_command(remote_shell.command.as_ref()));

    let host_shell = Cli::try_parse_from([
        "wsnav",
        "_presentation_remote_shell",
        "00000000-0000-0000-0000-000000000001",
    ])
    .unwrap();
    assert!(matches!(
        host_shell.command.as_ref(),
        Some(Commands::RemotePresentationShell { workstream_id })
            if workstream_id == "00000000-0000-0000-0000-000000000001"
    ));
    assert!(!is_provider_surface_command(host_shell.command.as_ref()));

    let host_literal = Cli::try_parse_from([
        "wsnav",
        "_presentation_remote_literal",
        "00000000-0000-0000-0000-000000000001",
    ])
    .unwrap();
    assert!(matches!(
        host_literal.command.as_ref(),
        Some(Commands::RemotePresentationLiteral { workstream_id })
            if workstream_id == "00000000-0000-0000-0000-000000000001"
    ));
    assert!(!is_provider_surface_command(host_literal.command.as_ref()));

    let temporary = tempfile::tempdir().unwrap();
    let invalid_remote = Cli::try_parse_from([
        "wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_presentation_remote_literal",
        "00000000-0000-0000-0000-000000000001",
    ])
    .unwrap();
    assert!(dispatch::execute(invalid_remote).is_err());

    let user =
        Cli::try_parse_from(["wsnav", "attach", "00000000-0000-0000-0000-000000000001"]).unwrap();
    assert!(!is_provider_surface_command(user.command.as_ref()));
}

#[test]
fn eligible_opencode_does_not_require_codex_observer_review() {
    let capabilities = vec![
        crate::protocol::ProviderCapability {
            kind: crate::domain::ProviderKind::Codex,
            status: crate::protocol::ProviderCapabilityStatus::Unavailable,
            reason: crate::protocol::ProviderCapabilityReason::ObserverNotReady,
            fresh_launch: false,
            exact_resume: false,
            observe: false,
            metadata_read: false,
            rename: false,
            fork: false,
        },
        crate::protocol::ProviderCapability {
            kind: crate::domain::ProviderKind::OpenCode,
            status: crate::protocol::ProviderCapabilityStatus::Available,
            reason: crate::protocol::ProviderCapabilityReason::None,
            fresh_launch: true,
            exact_resume: true,
            observe: true,
            metadata_read: true,
            rename: false,
            fork: false,
        },
    ];
    assert!(!should_prepare_codex_observer(&capabilities));
}

#[test]
fn observer_activation_and_manual_reconciliation_are_hidden_from_normal_cli_help() {
    let help = Cli::command().render_help().to_string();

    assert!(!help.contains("setup"));
    assert!(!help.contains("update-observer"));
    assert!(!help.contains("trust-observer"));
    assert!(!help.contains("_observer_review"));
    assert!(help.contains("Start the Workstream's native provider"));
    assert!(help.contains("live native provider Runtime"));
    assert!(!help.contains("Start native Codex"));
    assert!(help.contains("Recover a lost private Runtime through its exact native"));
}

#[test]
fn simple_remote_registration_needs_only_the_host_token() {
    let parsed = Cli::try_parse_from(["wsnav", "register-remote", "snap"]).unwrap();

    assert!(matches!(
        parsed.command,
        Some(Commands::RegisterRemote {
            host,
            destination: None,
            executable: None,
        }) if host == "snap"
    ));
}

#[test]
fn provider_choices_are_optional_flags_on_direct_and_host_creation_commands() {
    let direct_register =
        Cli::try_parse_from(["wsnav", "register", "/checkout", "--provider", "opencode"]).unwrap();
    assert!(matches!(
        direct_register.command,
        Some(Commands::Register { provider: Some(provider), .. }) if provider == "opencode"
    ));

    let direct_new = Cli::try_parse_from([
        "wsnav",
        "new-workstream",
        "00000000-0000-0000-0000-000000000001",
    ])
    .unwrap();
    assert!(matches!(
        direct_new.command,
        Some(Commands::NewWorkstream { provider: None, .. })
    ));

    let remote_register = Cli::try_parse_from([
        "wsnav",
        "host",
        "register-checkout",
        "snap",
        "/checkout",
        "--provider",
        "codex",
    ])
    .unwrap();
    assert!(matches!(
        remote_register.command,
        Some(Commands::Host {
            command: HostCommands::RegisterCheckout {
                provider: Some(provider), ..
            }
        }) if provider == "codex"
    ));

    let remote_new = Cli::try_parse_from([
        "wsnav",
        "host",
        "new",
        "snap",
        "00000000-0000-0000-0000-000000000001",
        "4",
    ])
    .unwrap();
    assert!(matches!(
        remote_new.command,
        Some(Commands::Host {
            command: HostCommands::New { provider: None, .. }
        })
    ));
}

#[test]
fn native_trust_is_recorded_only_after_codex_completes_the_exact_review() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
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
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let manager = test_observer_profile(temporary.path(), &root);

    let activation = prepare_observer_activation_with_manager(&mut registry, &manager).unwrap();

    assert_eq!(activation, ObserverActivation::ReviewRequired);
    assert!(manager.path().is_file());
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::TrustPending
    );
}

#[test]
fn navigator_activation_reopens_missing_native_trust_without_a_separate_setup_command() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let manager = test_observer_profile(temporary.path(), &root);
    let ownership = manager.install("owner".to_owned(), None).unwrap();
    registry
        .record_codex_integration(ownership, IntegrationLifecycle::Ready)
        .unwrap();

    let activation = prepare_observer_activation_with_manager(&mut registry, &manager).unwrap();

    assert_eq!(activation, ObserverActivation::ReviewRequired);
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::TrustPending
    );
}

#[test]
fn navigator_activation_migrates_an_exact_prior_executable_before_review() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
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

    let activation = prepare_observer_activation_with_manager(&mut registry, &manager).unwrap();

    assert_eq!(activation, ObserverActivation::ReviewRequired);
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

#[test]
fn navigator_activation_never_replaces_a_profile_while_a_runtime_is_live() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let manager = test_observer_profile(temporary.path(), &root);
    let ownership = manager.install("owner".to_owned(), None).unwrap();
    registry
        .record_codex_integration(ownership, IntegrationLifecycle::TrustPending)
        .unwrap();
    let workstream = registry
        .register_external_workstream(
            temporary.path().join("checkout"),
            "repository".to_owned(),
            "commit".to_owned(),
        )
        .unwrap();
    registry.reserve_runtime(workstream.workstream_id).unwrap();

    assert!(matches!(
        prepare_observer_activation_with_manager(&mut registry, &manager),
        Err(AppError::LiveRuntimePreventsObserverActivation)
    ));
    assert_eq!(
        registry.codex_integration().unwrap().unwrap().lifecycle,
        IntegrationLifecycle::TrustPending
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
