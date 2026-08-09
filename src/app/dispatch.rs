use super::{FromStr, HostRegistry, Presentation, RuntimeId, StateRoot, run_local_navigator};
use super::{
    cli::{Cli, Commands},
    launch::{
        OpenCodeObserverArguments, opencode_observer, provider_attach, provider_remote_attach,
        provider_remote_observer_review, provider_wait, runtime_launch,
    },
    lifecycle::{acknowledge, fork_workstream, new_workstream, register},
    local::{
        archive, attach, observe_hook, operations, park, recover, recover_operation, rename,
        restore, start, status,
    },
    model::{
        AppError, default_state_root, parse_operation, parse_optional_provider, parse_revision,
        parse_workstream,
    },
    observer::{
        ObserverActivation, doctor, observer_review, observer_review_once,
        prepare_observer_activation, remove_observer, setup, trust_observer, update_observer,
    },
    remote::{host_command, register_remote},
};

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
    if matches!(&command, Commands::Remote) {
        return crate::remote::serve(
            state_root,
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        )
        .map_err(AppError::Remote);
    }
    if matches!(&command, Commands::Probe) {
        return crate::build_info::write_probe(&mut std::io::stdout().lock())
            .map_err(AppError::BuildInfo);
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
    let root = StateRoot::create(state_root.unwrap_or_else(default_state_root))?;
    execute_root_command(&root, command)
}

fn execute_root_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    match command {
        Commands::Navigator => navigator(root),
        Commands::NavigatorPane {
            presentation_socket,
            presentation_session,
        } => run_local_navigator(root, presentation_socket, presentation_session)
            .map_err(AppError::Navigator),
        Commands::ProviderWait => provider_wait(),
        Commands::ObserverReview => observer_review(root),
        Commands::RemoteObserverReview => {
            observer_review_once(root);
            Ok(())
        }
        Commands::ProviderRemoteObserverReview { host_alias } => {
            provider_remote_observer_review(root, &host_alias)
        }
        command => execute_root_surface(root, command),
    }
}

fn execute_root_surface(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    let state_command = match command {
        Commands::ProviderAttach {
            workstream_id,
            presentation_socket,
            presentation_session,
            attempt_id,
        } => {
            return provider_attach(
                root,
                &workstream_id,
                presentation_socket,
                presentation_session,
                &attempt_id,
            );
        }
        Commands::ProviderRemoteAttach {
            host_alias,
            workstream_id,
            presentation_socket,
            presentation_session,
            attempt_id,
        } => {
            return provider_remote_attach(
                root,
                &host_alias,
                &workstream_id,
                presentation_socket,
                presentation_session,
                &attempt_id,
            );
        }
        Commands::RemoteAttach { runtime_id } => {
            let Ok(runtime_id) = RuntimeId::from_str(&runtime_id) else {
                return Ok(());
            };
            // This command runs directly in the provider terminal over SSH.
            // It must never print management diagnostics into that surface;
            // the local navigator observes the resulting runtime state.
            let _ = crate::remote::attach(root, runtime_id);
            return Ok(());
        }
        Commands::RuntimeLaunch {
            runtime_id,
            program,
        } => return runtime_launch(root, &runtime_id, program),
        Commands::OpenCodeObserver {
            runtime_id,
            generation,
            port,
            session_id,
            pane_pid,
            cwd,
            provider_birth,
        } => {
            return opencode_observer(
                root,
                OpenCodeObserverArguments {
                    runtime_id,
                    generation,
                    port,
                    session_id,
                    pane_pid,
                    cwd,
                    provider_birth,
                },
            );
        }
        Commands::RegisterRemote {
            host,
            destination,
            executable,
        } => return register_remote(root, &host, destination.as_deref(), executable.as_deref()),
        Commands::Host { command } => return host_command(root, command),
        other => other,
    };
    execute_state_command(root, state_command)
}

fn execute_state_command(root: &StateRoot, command: Commands) -> Result<(), AppError> {
    let mut registry = HostRegistry::open(root)?;
    match command {
        Commands::Setup { skip_review } => setup(root, &mut registry, skip_review),
        Commands::TrustObserver => trust_observer(root, &mut registry),
        Commands::Doctor => doctor(root, &mut registry),
        Commands::UpdateObserver => update_observer(root, &mut registry),
        Commands::RemoveObserver => remove_observer(root, &mut registry),
        Commands::Register { checkout, provider } => register(
            &mut registry,
            &checkout,
            parse_optional_provider(provider.as_deref())?,
        ),
        Commands::NewWorkstream {
            source_workstream_id,
            provider,
        } => new_workstream(
            root,
            &mut registry,
            parse_workstream(&source_workstream_id)?,
            parse_optional_provider(provider.as_deref())?,
        ),
        Commands::ForkWorkstream {
            source_workstream_id,
        } => fork_workstream(
            root,
            &mut registry,
            parse_workstream(&source_workstream_id)?,
        ),
        Commands::Start { workstream_id } => {
            start(root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Recover { workstream_id } => {
            recover(root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Attach { workstream_id } => {
            attach(root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Park { workstream_id } => {
            park(root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Archive {
            workstream_id,
            revision,
        } => archive(
            root,
            &mut registry,
            parse_workstream(&workstream_id)?,
            parse_revision(revision)?,
        ),
        Commands::Restore {
            workstream_id,
            revision,
        } => restore(
            &mut registry,
            parse_workstream(&workstream_id)?,
            parse_revision(revision)?,
        ),
        Commands::Status { workstream_id } => {
            status(root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Operations => operations(&registry),
        Commands::RecoverOperation { operation_id } => {
            recover_operation(root, &mut registry, parse_operation(&operation_id)?)
        }
        Commands::Rename {
            workstream_id,
            revision,
            name,
        } => rename(
            &mut registry,
            parse_workstream(&workstream_id)?,
            parse_revision(revision)?,
            &name,
        ),
        Commands::Acknowledge {
            workstream_id,
            attention_revision,
        } => acknowledge(
            &mut registry,
            parse_workstream(&workstream_id)?,
            attention_revision,
        ),
        _ => unreachable!("special command dispatch returns before state setup"),
    }
}

fn navigator(root: &StateRoot) -> Result<(), AppError> {
    let activation = {
        let mut registry = HostRegistry::open(root)?;
        prepare_navigator_observer_activation(root, &mut registry)?
    };
    let (presentation, fresh) = Presentation::open_or_create(root.base())?;
    if fresh {
        presentation.start()?;
    }
    if activation == Some(ObserverActivation::ReviewRequired) && fresh {
        presentation.start_observer_review()?;
        presentation.focus_provider()?;
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

/// Keeps provider startup scoped to the provider that can actually authorize
/// a Workstream action.  An unready Codex observer must not force a native
/// review (or otherwise block the navigator) when an eligible `OpenCode`
/// adapter is already available.  Codex setup remains an explicit Hosts-page
/// action in that case.
fn prepare_navigator_observer_activation(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<Option<ObserverActivation>, AppError> {
    let capabilities = crate::provider::discover_capabilities(registry)?;
    if !should_prepare_codex_observer(&capabilities) {
        return Ok(None);
    }
    prepare_observer_activation(root, registry).map(Some)
}

pub(super) fn should_prepare_codex_observer(
    capabilities: &[crate::protocol::ProviderCapability],
) -> bool {
    let opencode_eligible = capabilities
        .iter()
        .find(|capability| capability.kind == crate::domain::ProviderKind::OpenCode)
        .is_some_and(|capability| capability.is_new_eligible());
    let codex_eligible = capabilities
        .iter()
        .find(|capability| capability.kind == crate::domain::ProviderKind::Codex)
        .is_some_and(|capability| capability.is_new_eligible());
    !opencode_eligible || codex_eligible
}
