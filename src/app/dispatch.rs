use super::{Presentation, StateRoot, materialize_initial_provisional_shell, run_d17_navigator};
use super::{
    cli::{Cli, Commands},
    launch::{
        OpenCodeObserverArguments, OpenCodeObserverStandbyArguments, attach_runtime_d17,
        opencode_observer, opencode_observer_standby, presentation_control, provider_attach_d17,
        provider_wait, runtime_launch,
    },
    local::observe_hook,
    model::{
        AppError, default_state_root, parse_operation, parse_provider, parse_revision,
        parse_workstream,
    },
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
    if let Commands::D17ShellGate {
        provider,
        shell_leader_pid,
        arguments,
    } = command
    {
        return d17_shell_gate(&provider, shell_leader_pid, &arguments);
    }
    if let Commands::D17LaunchHelper {
        capability,
        provider,
        arguments,
    } = command
    {
        return d17_launch_helper(&capability, &provider, &arguments);
    }
    if let Commands::D17ObserverSetup {
        shell_leader_pid,
        consent,
    } = command
    {
        return d17_observer_setup(shell_leader_pid, consent);
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
    if let Commands::OpenCodeObserverStandby {
        runtime_id,
        generation,
        port,
        provider_version,
        session_id,
        pane_pid,
        cwd,
        provider_birth,
    } = command
    {
        let root = state_root.unwrap_or_else(default_state_root);
        return opencode_observer_standby(
            &root,
            OpenCodeObserverStandbyArguments {
                runtime_id,
                generation,
                port,
                provider_version,
                session_id,
                pane_pid,
                cwd,
                provider_birth,
            },
        );
    }
    // Ordinary entrypoints must not create or migrate state before the D16
    // launcher has classified the root.  Hidden provider/observer helpers are
    // handled above and receive their own exact state contract.
    let root = StateRoot::select(state_root.unwrap_or_else(default_state_root));
    execute_root_command(&root, command)
}

/// The shell wrapper captures only this exact stdout stream. Any unavailable
/// state remains an exit code so a malformed capability cannot become terminal
/// traffic or a provider argument.
fn d17_shell_gate(
    provider: &str,
    shell_leader_pid: u32,
    arguments: &[std::ffi::OsString],
) -> Result<(), AppError> {
    let provider = parse_provider(provider).map_err(|_| AppError::D17ShellControlUnavailable)?;
    match crate::d17_shell_control::gate_from_account_shell(provider, arguments, shell_leader_pid)
        .map_err(|_| AppError::D17ShellControlUnavailable)?
    {
        crate::d17_shell_control::AccountShellGateOutcome::ExplicitlyUnmanaged => {
            Err(AppError::D17ShellGateUnmanaged)
        }
        crate::d17_shell_control::AccountShellGateOutcome::ObserverReadinessRequired => {
            Err(AppError::D17ObserverReadinessRequired)
        }
        crate::d17_shell_control::AccountShellGateOutcome::Prepared(handoff) => {
            let capability = handoff.capability().token();
            if !valid_launch_capability_token(capability) {
                return Err(AppError::D17ShellControlUnavailable);
            }
            let mut stdout = std::io::stdout().lock();
            stdout
                .write_all(capability.as_bytes())
                .and_then(|()| stdout.flush())
                .map_err(AppError::Io)
        }
    }
}

/// Completes the private half of a D17 account-shell handoff. The helpers
/// perform their own complete state and identity revalidation before they can
/// execute either native provider.
fn d17_launch_helper(
    capability: &str,
    provider: &str,
    arguments: &[std::ffi::OsString],
) -> Result<(), AppError> {
    if !valid_launch_capability_token(capability) {
        return Err(AppError::D17ShellControlUnavailable);
    }
    let provider = parse_provider(provider).map_err(|_| AppError::D17ShellControlUnavailable)?;
    match provider {
        ProviderKind::Codex => {
            crate::d17_shell_control::exec_codex_from_account_shell(capability, arguments)
                .map_err(|_| AppError::D17ShellControlUnavailable)
        }
        ProviderKind::OpenCode => {
            crate::d17_shell_control::exec_opencode_from_account_shell(capability, arguments)
                .map_err(|_| AppError::D17ShellControlUnavailable)
        }
    }
}

/// Runs the only interactive observer setup route.  The account-shell
/// wrapper has already collected explicit consent; this hidden route merely
/// revalidates the exact provisional shell, installs/updates the owned
/// declaration, and hosts Codex's native `/hooks` review in that same pane.
fn d17_observer_setup(shell_leader_pid: u32, consent: bool) -> Result<(), AppError> {
    if !consent {
        return Err(AppError::D17ShellControlUnavailable);
    }
    crate::d17_shell_control::prepare_observer_from_account_shell(shell_leader_pid)
        .map_err(|_| AppError::D17ShellControlUnavailable)
}

fn execute_root_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    match command {
        Commands::Navigator => navigator(root),
        Commands::NavigatorPaneD17 {
            presentation_socket,
            presentation_session,
        } => run_d17_navigator(root, presentation_socket, presentation_session)
            .map_err(AppError::D17Navigator),
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
        Commands::ProviderAttachD17 {
            workstream_id,
            expected_workstream_revision,
            expected_runtime_id,
            expected_runtime_revision,
            presentation_socket,
            presentation_session,
            attempt_id,
        } => provider_attach_d17(
            root,
            &workstream_id,
            expected_workstream_revision,
            &expected_runtime_id,
            expected_runtime_revision,
            presentation_socket,
            presentation_session,
            &attempt_id,
        ),
        Commands::RuntimeLaunch {
            runtime_id,
            program,
        } => runtime_launch(root, &runtime_id, program),
        Commands::OpenCodeObserverD16 {
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
                mode: crate::provider::opencode::OpenCodeObserverMode::D16,
            },
        ),
        Commands::OpenCodeObserverD17 {
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
                mode: crate::provider::opencode::OpenCodeObserverMode::D17,
            },
        ),
        Commands::OpenCodeObserverStandby { .. } => {
            unreachable!("standby observer is dispatched before state-root creation")
        }
        Commands::D17ShellGate { .. }
        | Commands::D17LaunchHelper { .. }
        | Commands::D17ObserverSetup { .. } => {
            unreachable!("D17 account-shell control is dispatched before state-root creation")
        }
        Commands::Doctor => exceptional_observer(root, false),
        Commands::RemoveObserver => exceptional_observer(root, true),
        Commands::ForkWorkstream { .. }
        | Commands::Start { .. }
        | Commands::Recover { .. }
        | Commands::Attach { .. }
        | Commands::Park { .. }
        | Commands::Archive { .. }
        | Commands::Restore { .. }
        | Commands::Status { .. }
        | Commands::Operations
        | Commands::RecoverOperation { .. }
        | Commands::Rename { .. }
        | Commands::Acknowledge { .. } => execute_d17_local_command(root, command),
        Commands::Navigator
        | Commands::NavigatorPaneD17 { .. }
        | Commands::PresentationControl { .. }
        | Commands::ProviderWait
        | Commands::Hook
        | Commands::OpenCodeServeBarrier { .. }
        | Commands::OpenCodeServeGuardian { .. } => {
            unreachable!("root surface command was handled by an earlier dispatch branch")
        }
    }
}

fn exceptional_observer(root: &StateRoot, remove: bool) -> Result<(), AppError> {
    let state = crate::state::open_d17_current_only(&StateRoot::select(root.base()))?;
    let registry = state.into_d17_host_registry()?;
    if remove {
        let mut registry = registry;
        remove_observer(root, &mut registry)
    } else {
        doctor(root, &registry)
    }
}

/// Runs the retained public scripting/diagnostic command matrix directly
/// against the active schema-14 snapshot/action boundary.  Passive status and
/// operation queries stop after the bounded snapshot read and never launch a
/// provider or inspect tmux.
#[allow(
    clippy::too_many_lines,
    reason = "the public D17 command matrix is one auditable revision-fenced boundary"
)]
fn execute_d17_local_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    let snapshot =
        crate::d17_snapshot::read_snapshot(root).map_err(|_| AppError::D17AttachmentUnavailable)?;
    match command {
        Commands::ForkWorkstream {
            source_workstream_id,
        } => {
            let source_workstream_id = parse_workstream(&source_workstream_id)?;
            let source = d17_workstream(&snapshot, source_workstream_id)?;
            require_direct_codex_observer_ready(root, source.provider)?;
            apply_managed_action(
                root,
                ManagedAction::Fork {
                    source_workstream_id,
                    expected_workstream_revision: source.revision,
                    provider: source.provider,
                },
            )
            .map_err(AppError::D17Navigator)?;
            Ok(())
        }
        Commands::Start { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = d17_workstream(&snapshot, workstream_id)?;
            require_direct_codex_observer_ready(root, workstream.provider)?;
            apply_managed_action(
                root,
                ManagedAction::Start {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                },
            )
            .map_err(AppError::D17Navigator)?;
            Ok(())
        }
        Commands::Recover { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = d17_workstream(&snapshot, workstream_id)?;
            require_direct_codex_observer_ready(root, workstream.provider)?;
            apply_managed_action(
                root,
                ManagedAction::Recover {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                },
            )
            .map_err(AppError::D17Navigator)?;
            Ok(())
        }
        Commands::Attach { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = d17_workstream(&snapshot, workstream_id)?;
            let runtime = workstream
                .runtime
                .ok_or(AppError::D17AttachmentUnavailable)?;
            if workstream.onboarding.is_some() {
                return Err(AppError::D17AttachmentUnavailable);
            }
            attach_runtime_d17(
                root,
                workstream_id,
                workstream.revision,
                runtime.runtime_id,
                runtime.revision,
            )
        }
        Commands::Park { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = d17_workstream(&snapshot, workstream_id)?;
            apply_managed_action(
                root,
                ManagedAction::Park {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                },
            )
            .map_err(AppError::D17Navigator)?;
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
            .map_err(AppError::D17Navigator)?;
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
            .map_err(AppError::D17Navigator)?;
            Ok(())
        }
        Commands::Status { workstream_id } => {
            let workstream = d17_workstream(&snapshot, parse_workstream(&workstream_id)?)?;
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
        Commands::RecoverOperation { operation_id } => {
            let operation_id = parse_operation(&operation_id)?;
            let operation = snapshot
                .unresolved_operations
                .iter()
                .find(|operation| operation.operation_id == operation_id)
                .ok_or(AppError::D17AttachmentUnavailable)?;
            require_direct_codex_observer_ready(root, operation.provider)?;
            apply_managed_action(
                root,
                ManagedAction::RecoverOperation {
                    operation_id,
                    expected_operation_revision: operation.revision,
                    provider: operation.provider,
                },
            )
            .map_err(AppError::D17Navigator)?;
            Ok(())
        }
        Commands::Rename {
            workstream_id,
            revision,
            name,
        } => {
            apply_managed_action(
                root,
                ManagedAction::Rename {
                    workstream_id: parse_workstream(&workstream_id)?,
                    expected_workstream_revision: parse_revision(revision)?,
                    name,
                },
            )
            .map_err(AppError::D17Navigator)?;
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
            .map_err(AppError::D17Navigator)?;
            Ok(())
        }
        _ => Err(AppError::D17AttachmentUnavailable),
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
    let state = crate::state::open_d17_current_only(root)?;
    let evidence =
        observer_readiness(root, &state).map_err(|_| AppError::D17AttachmentUnavailable)?;
    if evidence.readiness == ObserverReadiness::Ready {
        Ok(())
    } else {
        Err(AppError::D17ObserverReadinessRequired)
    }
}

fn d17_workstream(
    snapshot: &crate::d17_snapshot::D17Snapshot,
    workstream_id: crate::domain::WorkstreamId,
) -> Result<&crate::d17_snapshot::D17WorkstreamSnapshot, AppError> {
    snapshot
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .ok_or(AppError::D17AttachmentUnavailable)
}

fn navigator(root: &StateRoot) -> Result<(), AppError> {
    match prepare_d17_navigator_state(root)? {
        D17NavigatorStartup::Ready => {}
        D17NavigatorStartup::DrainOnly(plan) => return drain_only_presentation(root, &plan),
        D17NavigatorStartup::Exit => return Ok(()),
    }
    let (presentation, fresh) = Presentation::open_or_create_d17(root.base())?;
    if fresh {
        let seed_cwd = std::env::current_dir().map_err(AppError::Io)?;
        presentation.start_d17(uuid::Uuid::new_v4(), &seed_cwd)?;
        if materialize_initial_provisional_shell(root, &presentation).is_err() {
            let _ = presentation.show_guidance("Initial shell unavailable; exact state required");
        }
    } else {
        presentation.d17_context()?;
    }
    match presentation.attach_d17() {
        // A normal tmux detach leaves the private presentation available for a
        // later bare `wsnav` reconnect. It never affects a provider Runtime.
        Ok(()) => Ok(()),
        // `q` in the navigator stops the owned presentation itself. Its parent
        // sees a failed attach because the socket vanished, which is a normal
        // clean exit rather than an attachment failure.
        Err(_) if !presentation.paths().directory.exists() => {
            presentation.close_d17().map_err(Into::into)
        }
        Err(error) => {
            let cleanup = presentation.close_d17();
            cleanup?;
            Err(AppError::Presentation(error))
        }
    }
}

pub(super) enum D17NavigatorStartup {
    Ready,
    DrainOnly(Box<crate::cutover::CutoverPlan>),
    Exit,
}

/// Prepares only the durable state boundary for a normal D17 Navigator
/// launch. It deliberately proves that an old D16 presentation is absent
/// before schema-13 migration; no provider, tmux, marker, or shell action is
/// performed here.
pub(super) fn prepare_d17_navigator_state(
    root: &StateRoot,
) -> Result<D17NavigatorStartup, AppError> {
    let mut resume_d17_transition = false;
    match crate::state::open_d17_current_only(root) {
        Ok(state) => {
            drop(state);
            return Ok(D17NavigatorStartup::Ready);
        }
        Err(crate::state::StateError::FreshStateRequired) => {
            let state =
                crate::state::fresh_create_d17(root.base(), &crate::domain::RandomIdGenerator)?;
            drop(state);
            return Ok(D17NavigatorStartup::Ready);
        }
        Err(crate::state::StateError::CutoverRequired) => {}
        Err(crate::state::StateError::StateRecoveryRequired(
            crate::state::StateRecoveryReason::TransitionLeasePresent,
        )) => resume_d17_transition = true,
        Err(error) => return Err(AppError::State(error)),
    }
    let assessment = {
        let mut presentation = crate::cutover::LivePresentationAuthority::new(root.base());
        crate::startup::assess_startup(root, &mut presentation)
    };
    let assessment = match assessment {
        Ok(assessment) => assessment,
        Err(crate::startup::StartupError::State(
            crate::state::StateError::UnsupportedFutureHostSchema(version),
        )) if resume_d17_transition && version == crate::state::D17_HOST_SCHEMA_VERSION => {
            require_no_legacy_presentation(root)?;
            crate::state::migrate_current_to_d17(root)?;
            return Ok(D17NavigatorStartup::Ready);
        }
        Err(crate::startup::StartupError::State(
            crate::state::StateError::StateRecoveryRequired(
                crate::state::StateRecoveryReason::TransitionLeasePresent,
            ),
        )) if resume_d17_transition => {
            require_no_legacy_presentation(root)?;
            crate::state::migrate_current_to_d17(root)?;
            return Ok(D17NavigatorStartup::Ready);
        }
        Err(error) => return Err(AppError::Startup(error)),
    };
    match assessment {
        crate::startup::StartupAssessment::Fresh(_) => {
            let state =
                crate::state::fresh_create_d17(root.base(), &crate::domain::RandomIdGenerator)?;
            drop(state);
        }
        crate::startup::StartupAssessment::Current(state) => drop(state),
        crate::startup::StartupAssessment::Cutover(plan)
            if plan.kind() == crate::cutover::CutoverPlanKind::DrainOnly =>
        {
            return Ok(D17NavigatorStartup::DrainOnly(Box::new(plan)));
        }
        crate::startup::StartupAssessment::Cutover(plan) => {
            let confirmation = {
                let mut input = std::io::stdin().lock();
                let mut output = std::io::stdout().lock();
                crate::startup::prompt_cutover_confirmation(&mut input, &mut output)?
            };
            let mut presentation = crate::cutover::LivePresentationAuthority::new(root.base());
            let mut process =
                crate::cutover::LinuxOpenCodeCutoverProcessAuthority::new(root.base());
            let mut state_factory =
                crate::cutover::LiveCutoverStateFactory::new(StateRoot::select(root.base()));
            let mut orchestrator = crate::cutover::CutoverOrchestrator::new(
                &mut presentation,
                &mut process,
                &mut state_factory,
            );
            match orchestrator.execute(&plan, &confirmation, &crate::domain::RandomIdGenerator)? {
                crate::cutover::CutoverOutcome::Declined => return Ok(D17NavigatorStartup::Exit),
                crate::cutover::CutoverOutcome::DrainOnly(_) => {
                    return Ok(D17NavigatorStartup::DrainOnly(Box::new(plan)));
                }
                crate::cutover::CutoverOutcome::Completed(_) => {}
            }
        }
    }

    require_no_legacy_presentation(root)?;
    crate::state::migrate_current_to_d17(root)?;
    Ok(D17NavigatorStartup::Ready)
}

fn require_no_legacy_presentation(root: &StateRoot) -> Result<(), AppError> {
    let presentation = crate::presentation::classify_legacy_presentations(root.base())?;
    if presentation.state() != crate::presentation::LegacyPresentationState::None
        || presentation.proof().is_some()
    {
        return Err(AppError::D17CutoverNeedsPresentationClosed);
    }
    Ok(())
}

fn drain_only_presentation(
    root: &StateRoot,
    plan: &crate::cutover::CutoverPlan,
) -> Result<(), AppError> {
    let proof = plan
        .assessment()
        .proof()
        .ok_or(crate::cutover::CutoverError::DrainOnly)?;
    let presentation = Presentation::from_control(
        root.base(),
        proof.socket().to_path_buf(),
        proof.session_name().to_owned(),
    )?;
    presentation
        .attach_legacy_drain()
        .map_err(AppError::Presentation)
}
