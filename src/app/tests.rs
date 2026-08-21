use std::fmt::Write as _;

use super::{
    HostRegistry, IntegrationLifecycle, ObserverProfile, Path, StateRoot,
    cli::{Cli, Commands, is_provider_surface_command},
    dispatch,
    model::AppError,
    observer::{finalize_native_trust, prepare_observer_activation_with_manager},
};
use crate::application::ApplicationError;
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
fn state_free_opencode_helpers_are_hidden_and_not_provider_surfaces() {
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
}

#[test]
fn provider_surface_helpers_are_local_and_silent() {
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
    assert!(Cli::try_parse_from(["wsnav", "_provider_remote_attach"]).is_err());

    let review = Cli::try_parse_from(["wsnav", "_observer_review"]).unwrap();
    assert!(is_provider_surface_command(review.command.as_ref()));
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
    assert!(is_provider_surface_command(shell.command.as_ref()));
    assert!(Cli::try_parse_from(["wsnav", "_presentation_ssh_shell"]).is_err());
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
    for retired in ["register-remote", "host", "_remote", "_probe", "_attach"] {
        assert!(
            Cli::try_parse_from(["wsnav", retired]).is_err(),
            "{retired}"
        );
    }
}

#[test]
fn register_checkout_is_reduced_to_a_current_browser_relative_path() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fresh_root(&temporary);
    let browser = temporary.path().join("browser");
    let checkout = browser.join("repo");
    std::fs::create_dir_all(&checkout).unwrap();
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let mut registry = open_registry(&root);
    registry
        .set_project_browser_root(browser.to_str().unwrap())
        .unwrap();
    drop(registry);

    let relative = dispatch::checkout_browser_path(&root, &checkout).unwrap();
    assert_eq!(relative.as_str(), "repo");
    assert!(matches!(
        dispatch::checkout_browser_path(&root, &outside),
        Err(AppError::Application(ApplicationError::InvalidBrowserPath))
    ));
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
