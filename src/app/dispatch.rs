use super::{Presentation, StateRoot, materialize_initial_provisional_shell, run_navigator};
use super::{
    cli::{Cli, Commands},
    launch::{
        OpenCodeObserverArguments, attach_runtime, opencode_observer, presentation_control,
        presentation_mouse_validate, provider_attach, provider_wait, runtime_launch,
    },
    local::observe_hook,
    model::{AppError, default_state_root, parse_provider, parse_revision, parse_workstream},
    observer::{ObserverReadiness, doctor, observer_readiness, remove_observer},
};
use std::io::Write as _;

use crate::domain::ProviderKind;
use crate::navigator::{ManagedAction, apply_managed_action};
use crate::onboarding::valid_launch_capability_token;

pub(super) fn execute(cli: Cli) -> Result<(), AppError> {
    let Cli {
        state_root,
        command,
    } = cli;
    let command = command.unwrap_or(Commands::Navigator);
    if matches!(&command, Commands::Hook) {
        observe_hook(state_root);
        return Ok(());
    }
    if let Commands::ShellGate {
        provider,
        shell_leader_pid,
        arguments,
    } = command
    {
        return shell_gate(&provider, shell_leader_pid, &arguments);
    }
    if let Commands::LaunchHelper {
        capability,
        provider,
        arguments,
    } = command
    {
        return launch_helper(&capability, &provider, &arguments);
    }
    if let Commands::ObserverSetup {
        shell_leader_pid,
        consent,
    } = command
    {
        return observer_setup(shell_leader_pid, consent);
    }
    if let Commands::OpenCodeServeBarrier {
        executable,
        project_root,
        port,
    } = command
    {
        let endpoint = crate::provider::opencode::OpenCodeEndpoint::loopback(port)?;
        return crate::provider::opencode::run_barrier(&executable, &project_root, &endpoint)
            .map_err(AppError::OpenCode);
    }
    if let Commands::OpenCodeServeGuardian {
        executable,
        project_root,
        port,
    } = command
    {
        let endpoint = crate::provider::opencode::OpenCodeEndpoint::loopback(port)?;
        return crate::provider::opencode::run_guardian(&executable, &project_root, &endpoint)
            .map_err(AppError::OpenCode);
    }
    // Ordinary entrypoints must not create or migrate state before the current
    // launcher has classified the root.  Hidden provider/observer helpers are
    // handled above and receive their own exact state contract.
    let root = StateRoot::select(state_root.unwrap_or_else(default_state_root));
    execute_root_command(&root, command)
}

/// The shell wrapper captures only this exact stdout stream. Any unavailable
/// state remains an exit code so a malformed capability cannot become terminal
/// traffic or a provider argument.
fn shell_gate(
    provider: &str,
    shell_leader_pid: u32,
    arguments: &[std::ffi::OsString],
) -> Result<(), AppError> {
    let provider = parse_provider(provider).map_err(|_| AppError::ShellControlUnavailable)?;
    match crate::shell_control::gate_from_account_shell(provider, arguments, shell_leader_pid)
        .map_err(|_| AppError::ShellControlUnavailable)?
    {
        crate::shell_control::AccountShellGateOutcome::ExplicitlyUnmanaged => {
            Err(AppError::ShellGateUnmanaged)
        }
        crate::shell_control::AccountShellGateOutcome::ObserverReadinessRequired => {
            Err(AppError::ObserverReadinessRequired)
        }
        crate::shell_control::AccountShellGateOutcome::Prepared(handoff) => {
            let capability = handoff.capability().token();
            if !valid_launch_capability_token(capability) {
                return Err(AppError::ShellControlUnavailable);
            }
            let mut stdout = std::io::stdout().lock();
            stdout
                .write_all(capability.as_bytes())
                .and_then(|()| stdout.flush())
                .map_err(AppError::Io)
        }
    }
}

/// Completes the private half of a account-shell handoff. The helpers
/// perform their own complete state and identity revalidation before they can
/// execute either native provider.
fn launch_helper(
    capability: &str,
    provider: &str,
    arguments: &[std::ffi::OsString],
) -> Result<(), AppError> {
    if !valid_launch_capability_token(capability) {
        return Err(AppError::ShellControlUnavailable);
    }
    let provider = parse_provider(provider).map_err(|_| AppError::ShellControlUnavailable)?;
    match provider {
        ProviderKind::Codex => {
            crate::shell_control::exec_codex_from_account_shell(capability, arguments)
                .map_err(|_| AppError::ShellControlUnavailable)
        }
        ProviderKind::OpenCode => {
            crate::shell_control::exec_opencode_from_account_shell(capability, arguments)
                .map_err(|_| AppError::ShellControlUnavailable)
        }
    }
}

/// Runs the only interactive observer setup route.  The account-shell
/// wrapper has already collected explicit consent; this hidden route merely
/// revalidates the exact provisional shell, installs/updates the owned
/// declaration, and hosts Codex's native `/hooks` review in that same pane.
fn observer_setup(shell_leader_pid: u32, consent: bool) -> Result<(), AppError> {
    if !consent {
        return Err(AppError::ShellControlUnavailable);
    }
    crate::shell_control::prepare_observer_from_account_shell(shell_leader_pid)
        .map_err(|_| AppError::ShellControlUnavailable)
}

fn execute_root_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    match command {
        Commands::Navigator => navigator(root),
        Commands::NavigatorPane {
            presentation_socket,
            presentation_session,
        } => run_navigator(root, presentation_socket, presentation_session)
            .map_err(AppError::Navigator),
        Commands::PresentationControl {
            presentation_socket,
            presentation_session,
            action,
            source_pane,
            client_name,
        } => presentation_control(
            root,
            presentation_socket,
            presentation_session,
            &action,
            &source_pane,
            &client_name,
        ),
        Commands::PresentationMouse {
            presentation_socket,
            presentation_session,
            target_pane,
            client_name,
        } => presentation_mouse_validate(
            root,
            presentation_socket,
            presentation_session,
            &target_pane,
            &client_name,
        ),
        Commands::ProviderWait => provider_wait(),
        command => execute_root_surface(root, command),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "The typed hidden-helper matrix keeps schema and pane boundaries auditable in one dispatch."
)]
fn execute_root_surface(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    match command {
        Commands::ProviderAttach {
            workstream_id,
            expected_workstream_revision,
            expected_runtime_id,
            expected_runtime_revision,
            presentation_socket,
            presentation_session,
            attempt_id,
            provider_cycle,
        } => provider_attach(
            root,
            &workstream_id,
            expected_workstream_revision,
            &expected_runtime_id,
            expected_runtime_revision,
            presentation_socket,
            presentation_session,
            &attempt_id,
            provider_cycle,
        ),
        Commands::RuntimeLaunch {
            runtime_id,
            program,
        } => runtime_launch(root, &runtime_id, program),
        Commands::OpenCodeObserver {
            runtime_id,
            generation,
            port,
            session_id,
            pane_pid,
            cwd,
            provider_birth,
        } => opencode_observer(
            root,
            OpenCodeObserverArguments {
                runtime_id,
                generation,
                port,
                session_id,
                pane_pid,
                cwd,
                provider_birth,
                mode: crate::provider::opencode::OpenCodeObserverMode::Current,
            },
        ),
        Commands::ShellGate { .. }
        | Commands::LaunchHelper { .. }
        | Commands::ObserverSetup { .. } => {
            unreachable!("account-shell control is dispatched before state-root creation")
        }
        Commands::Doctor => exceptional_observer(root, false),
        Commands::RemoveObserver => exceptional_observer(root, true),
        Commands::Start { .. }
        | Commands::Recover { .. }
        | Commands::Attach { .. }
        | Commands::Park { .. }
        | Commands::Archive { .. }
        | Commands::Restore { .. }
        | Commands::Status { .. }
        | Commands::Operations
        | Commands::Acknowledge { .. } => execute_local_command(root, command),
        Commands::Navigator
        | Commands::NavigatorPane { .. }
        | Commands::PresentationControl { .. }
        | Commands::PresentationMouse { .. }
        | Commands::ProviderWait
        | Commands::Hook
        | Commands::OpenCodeServeBarrier { .. }
        | Commands::OpenCodeServeGuardian { .. } => {
            unreachable!("root surface command was handled by an earlier dispatch branch")
        }
    }
}

fn exceptional_observer(root: &StateRoot, remove: bool) -> Result<(), AppError> {
    let state = crate::state::open_current(&StateRoot::select(root.base()))?;
    let registry = state.into_host_registry()?;
    if remove {
        let mut registry = registry;
        remove_observer(root, &mut registry)
    } else {
        doctor(root, &registry)
    }
}

/// Runs the retained public scripting/diagnostic command matrix directly
/// against the active schema-15 snapshot/action boundary.  Passive status and
/// operation queries stop after the bounded snapshot read and never launch a
/// provider or inspect tmux.
#[allow(
    clippy::too_many_lines,
    reason = "the public command matrix is one auditable revision-fenced boundary"
)]
fn execute_local_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    let snapshot =
        crate::snapshot::read_snapshot(root).map_err(|_| AppError::AttachmentUnavailable)?;
    match command {
        Commands::Start { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = workstream(&snapshot, workstream_id)?;
            require_direct_codex_observer_ready(root, workstream.provider)?;
            apply_managed_action(
                root,
                ManagedAction::Start {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                },
            )
            .map_err(AppError::Navigator)?;
            Ok(())
        }
        Commands::Recover { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = workstream(&snapshot, workstream_id)?;
            require_direct_codex_observer_ready(root, workstream.provider)?;
            apply_managed_action(
                root,
                ManagedAction::Recover {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                },
            )
            .map_err(AppError::Navigator)?;
            Ok(())
        }
        Commands::Attach { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = workstream(&snapshot, workstream_id)?;
            let runtime = workstream.runtime.ok_or(AppError::AttachmentUnavailable)?;
            if workstream.onboarding.is_some() {
                return Err(AppError::AttachmentUnavailable);
            }
            attach_runtime(
                root,
                workstream_id,
                workstream.revision,
                runtime.runtime_id,
                runtime.revision,
            )
        }
        Commands::Park { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = workstream(&snapshot, workstream_id)?;
            apply_managed_action(
                root,
                ManagedAction::Park {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                },
            )
            .map_err(AppError::Navigator)?;
            Ok(())
        }
        Commands::Archive {
            workstream_id,
            revision,
        } => {
            apply_managed_action(
                root,
                ManagedAction::Archive {
                    workstream_id: parse_workstream(&workstream_id)?,
                    expected_workstream_revision: parse_revision(revision)?,
                },
            )
            .map_err(AppError::Navigator)?;
            Ok(())
        }
        Commands::Restore {
            workstream_id,
            revision,
        } => {
            apply_managed_action(
                root,
                ManagedAction::Restore {
                    workstream_id: parse_workstream(&workstream_id)?,
                    expected_workstream_revision: parse_revision(revision)?,
                },
            )
            .map_err(AppError::Navigator)?;
            Ok(())
        }
        Commands::Status { workstream_id } => {
            let workstream = workstream(&snapshot, parse_workstream(&workstream_id)?)?;
            println!(
                "workstream {} provider={:?} lifecycle={:?} archived={} revision={} runtime={}",
                workstream.workstream_id,
                workstream.provider,
                workstream.lifecycle,
                workstream.archived,
                workstream.revision.value(),
                workstream.runtime.map_or_else(
                    || "none".to_owned(),
                    |runtime| format!("{:?}/{}", runtime.status, runtime.revision.value()),
                ),
            );
            Ok(())
        }
        Commands::Operations => {
            for operation in &snapshot.unresolved_operations {
                println!(
                    "operation {} kind={:?} provider={:?} phase={:?} revision={}",
                    operation.operation_id,
                    operation.kind,
                    operation.provider,
                    operation.phase,
                    operation.revision.value(),
                );
            }
            Ok(())
        }
        Commands::Acknowledge {
            workstream_id,
            attention_revision,
        } => {
            apply_managed_action(
                root,
                ManagedAction::AcknowledgeResult {
                    workstream_id: parse_workstream(&workstream_id)?,
                    expected_attention_revision: parse_revision(attention_revision)?,
                },
            )
            .map_err(AppError::Navigator)?;
            Ok(())
        }
        _ => Err(AppError::AttachmentUnavailable),
    }
}

/// Direct scripting commands are intentionally non-interactive.  A Codex
/// launch may proceed only when the exact observer profile is already Ready;
/// setup, native trust review, and profile updates remain contextual UI/shell
/// flows and never occur as a side effect of this boundary.
fn require_direct_codex_observer_ready(
    root: &StateRoot,
    provider: ProviderKind,
) -> Result<(), AppError> {
    if provider != ProviderKind::Codex {
        return Ok(());
    }
    let state = crate::state::open_current(root)?;
    let evidence = observer_readiness(root, &state).map_err(|_| AppError::AttachmentUnavailable)?;
    if evidence.readiness == ObserverReadiness::Ready {
        Ok(())
    } else {
        Err(AppError::ObserverReadinessRequired)
    }
}

fn workstream(
    snapshot: &crate::snapshot::Snapshot,
    workstream_id: crate::domain::WorkstreamId,
) -> Result<&crate::snapshot::WorkstreamSnapshot, AppError> {
    snapshot
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .ok_or(AppError::AttachmentUnavailable)
}

fn navigator(root: &StateRoot) -> Result<(), AppError> {
    prepare_navigator_state(root)?;
    let (presentation, fresh) = Presentation::open_or_create(root.base())?;
    if fresh {
        let seed_cwd = std::env::current_dir().map_err(AppError::Io)?;
        presentation.start(uuid::Uuid::new_v4(), &seed_cwd)?;
        if materialize_initial_provisional_shell(root, &presentation).is_err() {
            let _ = presentation.show_guidance("Initial shell unavailable; exact state required");
        }
    } else {
        presentation.context()?;
    }
    match presentation.attach() {
        // A normal tmux detach leaves the private presentation available for a
        // later bare `wsnav` reconnect. It never affects a provider Runtime.
        Ok(()) => Ok(()),
        // `q` in the navigator stops the owned presentation itself. Its parent
        // sees a failed attach because the socket vanished, which is a normal
        // clean exit rather than an attachment failure.
        Err(_) if !presentation.paths().directory.exists() => {
            presentation.close().map_err(Into::into)
        }
        Err(error) => {
            let cleanup = presentation.close();
            cleanup?;
            Err(AppError::Presentation(error))
        }
    }
}

pub(super) enum NavigatorStartup {
    Ready,
}

/// Prepares only the durable state boundary for a normal Navigator launch.
/// Current startup opens an exact schema-15 root or bootstraps one through the current
/// lock protocol; no provider, tmux, marker, or shell action is performed
/// here.
pub(super) fn prepare_navigator_state(root: &StateRoot) -> Result<NavigatorStartup, AppError> {
    match crate::state::open_current(root) {
        Ok(state) => drop(state),
        Err(crate::state::StateError::FreshStateRequired) => {
            let state =
                crate::state::create_current(root.base(), &crate::domain::RandomIdGenerator)?;
            drop(state);
        }
        Err(error) => return Err(AppError::State(error)),
    }
    Ok(NavigatorStartup::Ready)
}
