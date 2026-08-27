use super::{Presentation, StateRoot, run_d17_navigator, run_local_navigator};
use super::{
    cli::{Cli, Commands},
    launch::{
        OpenCodeObserverArguments, OpenCodeObserverStandbyArguments, attach_runtime,
        opencode_observer, opencode_observer_standby, presentation_control, presentation_shell,
        provider_attach, provider_wait, runtime_launch,
    },
    local::observe_hook,
    model::{
        AppError, default_state_root, parse_operation, parse_optional_provider, parse_provider,
        parse_revision, parse_workstream,
    },
    observer::{doctor, observer_review, remove_observer},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{io::Write as _, path::Component};

use crate::application::{
    ApplicationAction, ApplicationError, ApplicationOutcome, ApplicationSnapshot, AttentionKind,
    BrowserPath, BrowserRootPath, HostRegistryApplicationBackend, LocalApplication,
    ProviderCapability, operating_system_hostname,
};
use crate::domain::ProviderKind;

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
        crate::d17_shell_control::AccountShellGateOutcome::Prepared(handoff) => {
            let capability = handoff.capability().token();
            if !valid_d17_capability(capability) {
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
    if !valid_d17_capability(capability) {
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

fn valid_d17_capability(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
}

fn execute_root_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    match command {
        Commands::Navigator => navigator(root),
        Commands::NavigatorPane {
            presentation_socket,
            presentation_session,
        } => run_local_navigator(root, presentation_socket, presentation_session)
            .map_err(AppError::D16Navigator),
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
        Commands::PresentationShell {
            presentation_socket,
            presentation_session,
            shell,
            cwd,
        } => presentation_shell(root, presentation_socket, presentation_session, shell, cwd),
        Commands::ProviderWait => provider_wait(),
        Commands::ObserverReview => observer_review(root),
        command => execute_root_surface(root, command),
    }
}

fn execute_root_surface(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    match command {
        Commands::ProviderAttach {
            workstream_id,
            presentation_socket,
            presentation_session,
            attempt_id,
        } => provider_attach(
            root,
            &workstream_id,
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
        Commands::D17ShellGate { .. } | Commands::D17LaunchHelper { .. } => {
            unreachable!("D17 account-shell control is dispatched before state-root creation")
        }
        Commands::Doctor => exceptional_observer(root, false),
        Commands::RemoveObserver => exceptional_observer(root, true),
        Commands::Register { .. }
        | Commands::NewWorkstream { .. }
        | Commands::ForkWorkstream { .. }
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
        | Commands::Acknowledge { .. } => execute_d16_local_command(root, command),
        Commands::Navigator
        | Commands::NavigatorPane { .. }
        | Commands::NavigatorPaneD17 { .. }
        | Commands::PresentationControl { .. }
        | Commands::PresentationShell { .. }
        | Commands::ProviderWait
        | Commands::ObserverReview
        | Commands::Hook
        | Commands::OpenCodeServeBarrier { .. }
        | Commands::OpenCodeServeGuardian { .. } => {
            unreachable!("root surface command was handled by an earlier dispatch branch")
        }
    }
}

fn exceptional_observer(root: &StateRoot, remove: bool) -> Result<(), AppError> {
    let state = crate::state::open_current_only(&StateRoot::select(root.base()))?;
    let registry = state.into_host_registry()?;
    if remove {
        let mut registry = registry;
        remove_observer(root, &mut registry)
    } else {
        doctor(root, &registry)
    }
}

fn open_local_application(
    root: &StateRoot,
) -> Result<LocalApplication<HostRegistryApplicationBackend>, AppError> {
    LocalApplication::open_host_local(StateRoot::select(root.base()), operating_system_hostname())
        .map_err(AppError::Application)
}

#[allow(
    clippy::too_many_lines,
    reason = "The public local command matrix remains one auditable typed-facade dispatch boundary."
)]
fn execute_d16_local_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    let mut application = open_local_application(root)?;
    let snapshot = application.snapshot().map_err(AppError::Application)?;
    match command {
        Commands::Register { checkout, provider } => {
            let relative_path = checkout_browser_path(root, &checkout)?;
            let requested = parse_optional_provider(provider.as_deref())?;
            let provider = choose_provider(
                &snapshot,
                requested,
                None,
                ProviderCapability::eligible_for_new,
            )?;
            apply_and_report(
                &mut application,
                ApplicationAction::RegisterLocation {
                    relative_path,
                    expected_browser_revision: snapshot.project_browser.revision,
                    provider,
                },
            )
        }
        Commands::NewWorkstream {
            source_workstream_id,
            provider,
        } => {
            let source_id = parse_workstream(&source_workstream_id)?;
            let source = find_workstream(&snapshot, source_id)?;
            let requested = parse_optional_provider(provider.as_deref())?;
            let provider = choose_provider(
                &snapshot,
                requested,
                Some(source.provider),
                ProviderCapability::eligible_for_new,
            )?;
            apply_and_report(
                &mut application,
                ApplicationAction::NewAtSameLocation {
                    source_workstream_id: source_id,
                    expected_workstream_revision: source.revision,
                    provider,
                },
            )
        }
        Commands::ForkWorkstream {
            source_workstream_id,
        } => {
            let source_id = parse_workstream(&source_workstream_id)?;
            let source = find_workstream(&snapshot, source_id)?;
            let capability = capability(&snapshot, source.provider)?;
            if !capability.eligible_for_fork() {
                return Err(AppError::NoEligibleLocalProvider);
            }
            apply_and_report(
                &mut application,
                ApplicationAction::Fork {
                    source_workstream_id: source_id,
                    expected_workstream_revision: source.revision,
                    provider: source.provider,
                },
            )
        }
        Commands::Start { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = find_workstream(&snapshot, workstream_id)?;
            apply_and_report(
                &mut application,
                ApplicationAction::Start {
                    workstream_id,
                    expected_revision: workstream.revision,
                    provider: workstream.provider,
                },
            )
        }
        Commands::Recover { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = find_workstream(&snapshot, workstream_id)?;
            apply_and_report(
                &mut application,
                ApplicationAction::Recover {
                    workstream_id,
                    expected_revision: workstream.revision,
                    provider: workstream.provider,
                },
            )
        }
        Commands::Attach { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = find_workstream(&snapshot, workstream_id)?;
            let runtime = workstream
                .runtime
                .ok_or(AppError::NoRuntime(workstream_id))?;
            application
                .attach(crate::application::AttachEvidence {
                    workstream_id,
                    runtime_id: runtime.runtime_id,
                    expected_workstream_revision: workstream.revision,
                    expected_runtime_revision: runtime.revision,
                })
                .map_err(AppError::Application)?;
            attach_runtime(root, workstream_id)
        }
        Commands::Park { workstream_id } => {
            let workstream_id = parse_workstream(&workstream_id)?;
            let workstream = find_workstream(&snapshot, workstream_id)?;
            apply_and_report(
                &mut application,
                ApplicationAction::Park {
                    workstream_id,
                    expected_revision: workstream.revision,
                },
            )
        }
        Commands::Archive {
            workstream_id,
            revision,
        } => apply_and_report(
            &mut application,
            ApplicationAction::Archive {
                workstream_id: parse_workstream(&workstream_id)?,
                expected_revision: parse_revision(revision)?,
            },
        ),
        Commands::Restore {
            workstream_id,
            revision,
        } => apply_and_report(
            &mut application,
            ApplicationAction::Restore {
                workstream_id: parse_workstream(&workstream_id)?,
                expected_revision: parse_revision(revision)?,
            },
        ),
        Commands::Status { workstream_id } => {
            let workstream = find_workstream(&snapshot, parse_workstream(&workstream_id)?)?;
            println!(
                "workstream {} provider={:?} lifecycle={:?} archived={} revision={} runtime={}",
                workstream.workstream_id,
                workstream.provider,
                workstream.lifecycle,
                workstream.archived,
                workstream.revision.value(),
                workstream.runtime.map_or_else(
                    || "none".to_owned(),
                    |runtime| {
                        let rendered_status = if runtime.observer_degraded {
                            crate::domain::RuntimeStatus::Unknown
                        } else {
                            runtime.status
                        };
                        format!("{:?}/{}", rendered_status, runtime.revision.value())
                    }
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
                .ok_or_else(|| AppError::Application(ApplicationError::UnknownLocalIdentity))?;
            apply_and_report(
                &mut application,
                ApplicationAction::RecoverOperation {
                    operation_id,
                    expected_revision: operation.revision,
                    provider: operation.provider,
                },
            )
        }
        Commands::Rename {
            workstream_id,
            revision,
            name,
        } => apply_and_report(
            &mut application,
            ApplicationAction::Rename {
                workstream_id: parse_workstream(&workstream_id)?,
                expected_revision: parse_revision(revision)?,
                name,
            },
        ),
        Commands::Acknowledge {
            workstream_id,
            attention_revision,
        } => apply_and_report(
            &mut application,
            ApplicationAction::AcknowledgeAttention {
                workstream_id: parse_workstream(&workstream_id)?,
                expected_revision: parse_revision(attention_revision)?,
                kind: AttentionKind::Result,
            },
        ),
        _ => unreachable!("non-local command reached the D16 local facade"),
    }
}

fn apply_and_report(
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    action: ApplicationAction,
) -> Result<(), AppError> {
    match application.apply(action).map_err(AppError::Application)? {
        ApplicationOutcome::ObserverReadinessRequired(_) => {
            Err(AppError::ObserverReadinessGuideRequired)
        }
        ApplicationOutcome::Applied { .. }
        | ApplicationOutcome::Created { .. }
        | ApplicationOutcome::BrowserListed(_)
        | ApplicationOutcome::ProjectRefreshed { .. } => Ok(()),
    }
}

pub(super) fn checkout_browser_path(
    root: &StateRoot,
    checkout: &std::path::Path,
) -> Result<BrowserPath, AppError> {
    let browser_root = read_browser_root(root)?;
    let checkout = std::fs::canonicalize(checkout)
        .map_err(|_| AppError::Application(ApplicationError::InvalidBrowserPath))?;
    if checkout == browser_root || !checkout.starts_with(&browser_root) {
        return Err(AppError::Application(ApplicationError::InvalidBrowserPath));
    }
    let relative = checkout
        .strip_prefix(&browser_root)
        .map_err(|_| AppError::Application(ApplicationError::InvalidBrowserPath))?;
    let mut text = String::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(AppError::Application(ApplicationError::InvalidBrowserPath));
        };
        let component = component
            .to_str()
            .ok_or(AppError::Application(ApplicationError::InvalidBrowserPath))?;
        if !text.is_empty() {
            text.push('/');
        }
        text.push_str(component);
    }
    BrowserPath::new(text).map_err(AppError::Application)
}

fn read_browser_root(root: &StateRoot) -> Result<std::path::PathBuf, AppError> {
    let database = root.host_database_path();
    let connection = Connection::open_with_flags(
        &database,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| AppError::Application(ApplicationError::InvalidBrowserPath))?;
    let configured: Option<String> = connection
        .query_row(
            "SELECT root_path FROM project_browser_settings WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| AppError::Application(ApplicationError::InvalidBrowserPath))?;
    let configured = configured
        .map(|value| BrowserRootPath::new(value).map_err(AppError::Application))
        .transpose()?;
    let root = configured.map_or_else(
        || {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .ok_or(AppError::Application(ApplicationError::InvalidBrowserPath))
        },
        |path| Ok(std::path::PathBuf::from(path.as_str())),
    )?;
    let root = std::fs::canonicalize(root)
        .map_err(|_| AppError::Application(ApplicationError::InvalidBrowserPath))?;
    root.is_dir()
        .then_some(root)
        .ok_or(AppError::Application(ApplicationError::InvalidBrowserPath))
}

fn find_workstream(
    snapshot: &ApplicationSnapshot,
    workstream_id: crate::domain::WorkstreamId,
) -> Result<&crate::application::WorkstreamSnapshot, AppError> {
    snapshot
        .active_workstreams()
        .chain(snapshot.archived_workstreams())
        .find(|workstream| workstream.workstream_id == workstream_id)
        .ok_or_else(|| AppError::Application(ApplicationError::UnknownLocalIdentity))
}

fn capability(
    snapshot: &ApplicationSnapshot,
    provider: ProviderKind,
) -> Result<ProviderCapability, AppError> {
    snapshot
        .provider_capabilities
        .iter()
        .find(|capability| capability.provider == provider)
        .copied()
        .ok_or(AppError::NoEligibleLocalProvider)
}

fn choose_provider(
    snapshot: &ApplicationSnapshot,
    requested: Option<ProviderKind>,
    preferred: Option<ProviderKind>,
    eligible: fn(ProviderCapability) -> bool,
) -> Result<ProviderKind, AppError> {
    if let Some(provider) = requested {
        let capability = capability(snapshot, provider)?;
        return eligible(capability)
            .then_some(capability.provider)
            .ok_or(AppError::NoEligibleLocalProvider);
    }
    if let Some(provider) = preferred
        && let Ok(capability) = capability(snapshot, provider)
        && eligible(capability)
    {
        return Ok(provider);
    }
    snapshot
        .provider_capabilities
        .iter()
        .copied()
        .find(|capability| eligible(*capability))
        .map(|capability| capability.provider)
        .ok_or(AppError::NoEligibleLocalProvider)
}

fn navigator(root: &StateRoot) -> Result<(), AppError> {
    let assessment = {
        let mut presentation = crate::cutover::LivePresentationAuthority::new(root.base());
        crate::startup::assess_startup(root, &mut presentation)?
    };
    match assessment {
        crate::startup::StartupAssessment::Fresh(_) => {
            let state = crate::state::fresh_create(root.base(), &crate::domain::RandomIdGenerator)?;
            let _ = state.into_host_registry()?;
        }
        crate::startup::StartupAssessment::Current(_) => {}
        crate::startup::StartupAssessment::Cutover(plan)
            if plan.kind() == crate::cutover::CutoverPlanKind::DrainOnly =>
        {
            return drain_only_presentation(root, &plan);
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
                crate::cutover::CutoverOutcome::Declined => return Ok(()),
                crate::cutover::CutoverOutcome::DrainOnly(_) => {
                    return drain_only_presentation(root, &plan);
                }
                crate::cutover::CutoverOutcome::Completed(_) => {}
            }
        }
    }
    let (presentation, fresh) = Presentation::open_or_create(root.base())?;
    if fresh {
        presentation.start()?;
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
    presentation.attach().map_err(AppError::Presentation)
}
