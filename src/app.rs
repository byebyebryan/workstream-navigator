//! Thin provider-aware CLI orchestration for local and SSH Workstreams.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    str::FromStr,
};

use clap::{Parser, Subcommand};
use thiserror::Error;

use crate::{
    actions::{self},
    domain::{OperationId, ProviderSessionId, Revision, RuntimeId, WorkstreamId},
    navigator::run_local_navigator,
    presentation::{AttachmentPhase, Presentation},
    provider::codex::app_server::EphemeralAppServer,
    provider::codex::hooks::drain_and_parse,
    provider::codex::profile::{OBSERVER_PROFILE_SCHEMA_VERSION, ObserverProfile},
    provider::lifecycle::LifecycleEvent,
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
        await_launch_release, is_direct_provider_hook,
    },
    state::{
        ClientCatalog, ClientHostTransport, HostIdentity, HostRegistry, IntegrationLifecycle,
        StateError, StateRoot,
    },
    transport::{
        HostClient, RemoteExecutable, STANDARD_REMOTE_EXECUTABLE, SshDestination, SshEndpoint,
        SystemCommandRunner, attach_ssh,
    },
};

#[cfg(test)]
use crate::provider::names::NameState;

const ABOUT: &str =
    "A native-workflow terminal navigator for persistent coding workstreams across hosts.";
/// Runs one direct local CLI command.
#[must_use]
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let provider_surface = is_provider_surface_command(cli.command.as_ref());
    match execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        // These helpers execute inside a provider pane. They deliberately do
        // not expose CLI diagnostics there; normal navigator polling owns the
        // bounded state presentation after an attachment ends.
        Err(_) if provider_surface => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "wsnav", about = ABOUT, version)]
struct Cli {
    /// Private host state root. Defaults to `XDG_STATE_HOME/wsnav`.
    #[arg(long, global = true)]
    state_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Open the local two-pane Workstream Navigator presentation.
    Navigator,
    /// Internal direct observer setup and native hook review.
    #[command(hide = true)]
    Setup {
        /// Install without opening the native review TUI. This test-only escape
        /// hatch never marks the observer ready.
        #[arg(long, hide = true)]
        skip_review: bool,
    },
    /// Verify an already-completed native hook review. Normal setup performs
    /// this automatically after the review TUI exits.
    #[command(hide = true)]
    TrustObserver,
    /// Inspect the exact observer ownership and trust lifecycle without changing it.
    Doctor,
    /// Replace an exact owned observer declaration and require fresh native trust.
    #[command(hide = true)]
    UpdateObserver,
    /// Remove only the exact unchanged owned observer profile after all runtimes stop.
    RemoveObserver,
    /// Register one existing Git project as the initial Workstream location.
    Register {
        checkout: PathBuf,
        /// Provider to use when more than one provider is eligible.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Create and start an independent Workstream from one registered project location.
    NewWorkstream {
        source_workstream_id: String,
        /// Provider to use when the source provider is not an eligible default.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Fork one live Workstream at its last completed native Codex turn.
    ForkWorkstream { source_workstream_id: String },
    /// Start the Workstream's native provider in its private tmux server.
    Start { workstream_id: String },
    /// Recover a lost private Runtime through Codex's native resume flow.
    Recover { workstream_id: String },
    /// Attach this terminal directly to a live native provider Runtime.
    Attach { workstream_id: String },
    /// Park a Runtime without deleting project files or provider session history.
    Park { workstream_id: String },
    /// Hide a Workstream from the ordinary navigator without deleting retained state.
    Archive {
        workstream_id: String,
        revision: i64,
    },
    /// Return an archived Workstream without starting its native provider.
    Restore {
        workstream_id: String,
        revision: i64,
    },
    /// Show one local runtime's durable record and live private-tmux probe.
    Status { workstream_id: String },
    /// List unresolved Fork operations without exposing request keys or provider data.
    Operations,
    /// Reopen one exact unresolved Fork operation.
    RecoverOperation { operation_id: String },
    /// Rename the current managed Codex thread through its canonical name field.
    Rename {
        workstream_id: String,
        revision: i64,
        name: String,
    },
    /// Clear one observed result/recovery attention revision without sending provider input.
    Acknowledge {
        workstream_id: String,
        attention_revision: i64,
    },
    /// Register one SSH host using the standard remote wsnav installation.
    RegisterRemote {
        /// Navigator host label and, by default, the SSH destination.
        host: String,
        /// Override the SSH destination while retaining the host label.
        #[arg(long)]
        destination: Option<String>,
        /// Override the standard remote executable with an absolute path.
        #[arg(long)]
        executable: Option<PathBuf>,
    },
    /// Register and inspect explicit SSH host control-plane endpoints.
    Host {
        #[command(subcommand)]
        command: HostCommands,
    },
    /// Internal Ratatui process run inside an owned presentation pane.
    #[command(name = "_navigator", hide = true)]
    NavigatorPane {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
    },
    /// Internal blank provider-pane placeholder before an exact attachment is selected.
    #[command(name = "_provider_wait", hide = true)]
    ProviderWait,
    /// Internal temporary native Codex observer-review surface. It is not a
    /// Workstream and must never emit diagnostics into the provider pane.
    #[command(name = "_observer_review", hide = true)]
    ObserverReview,
    /// Internal one-shot remote observer review. Unlike the local provider-pane
    /// helper, it returns after native Codex exits so SSH can close cleanly.
    #[command(name = "_remote_observer_review", hide = true)]
    RemoteObserverReview,
    /// Internal remote native observer-review surface. It is not a Workstream
    /// attachment and must never emit navigator diagnostics into the provider pane.
    #[command(name = "_provider_remote_observer_review", hide = true)]
    ProviderRemoteObserverReview { host_alias: String },
    /// Internal local provider-pane attachment helper. It intentionally keeps
    /// all navigator diagnostics out of the native provider surface.
    #[command(name = "_provider_attach", hide = true)]
    ProviderAttach {
        workstream_id: String,
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        attempt_id: String,
    },
    /// Internal SSH provider-pane attachment helper. It intentionally keeps
    /// all navigator diagnostics out of the native provider surface.
    #[command(name = "_provider_remote_attach", hide = true)]
    ProviderRemoteAttach {
        host_alias: String,
        workstream_id: String,
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        attempt_id: String,
    },
    /// Internal passive Codex lifecycle hook entrypoint.
    #[command(name = "_hook", hide = true)]
    Hook,
    /// Internal one-shot local/SSH host control-protocol endpoint.
    #[command(name = "_remote", hide = true)]
    Remote,
    /// Internal state-free compatibility endpoint used before remote control.
    #[command(name = "_probe", hide = true)]
    Probe,
    /// Internal native-terminal-only attachment endpoint used through ssh -tt.
    #[command(name = "_attach", hide = true)]
    RemoteAttach { runtime_id: String },
    /// Internal one-shot launch barrier that replaces itself with the provider.
    #[command(name = "_runtime_launch", hide = true)]
    RuntimeLaunch {
        runtime_id: String,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        program: Vec<std::ffi::OsString>,
    },
    /// Internal disconnected `OpenCode` lifecycle observer.  It never writes to
    /// the provider pane and persists only bounded handle status.
    #[command(name = "_opencode_observer", hide = true)]
    OpenCodeObserver {
        runtime_id: String,
        generation: String,
        port: u16,
        session_id: String,
        pane_pid: u32,
        cwd: PathBuf,
        provider_birth: String,
    },
}

#[derive(Debug, Subcommand)]
enum HostCommands {
    /// List explicitly registered SSH hosts without contacting them.
    List,
    /// Fetch one validated bounded snapshot from a registered SSH host.
    Snapshot { alias: String },
    /// List unresolved creation operations on one registered SSH host.
    Operations { alias: String },
    /// Verify a remote executable's stateless release probe and registered host identity.
    Doctor { alias: String },
    /// Register one existing Git project on a verified SSH host.
    RegisterCheckout {
        alias: String,
        checkout: String,
        /// Provider to use when more than one provider is eligible.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Install or reconcile a remote exact observer profile before native review.
    PrepareObserver { alias: String },
    /// Remove only an exact remote observer profile after managed Runtimes stop.
    RemoveObserver { alias: String },
    /// Start or cold-resume one remote Workstream at an observed revision.
    Start {
        alias: String,
        workstream_id: String,
        revision: i64,
    },
    /// Recover one remote Workstream through its native Codex resume flow.
    Recover {
        alias: String,
        workstream_id: String,
        revision: i64,
    },
    /// Park one remote Workstream at an observed revision.
    Park {
        alias: String,
        workstream_id: String,
        revision: i64,
    },
    /// Hide one remote Workstream without deleting its retained state.
    Archive {
        alias: String,
        workstream_id: String,
        revision: i64,
    },
    /// Return one archived remote Workstream without starting its native provider.
    Restore {
        alias: String,
        workstream_id: String,
        revision: i64,
    },
    /// Set one remote Workstream's canonical Codex thread title.
    Rename {
        alias: String,
        workstream_id: String,
        revision: i64,
        name: String,
    },
    /// Create and start an independent managed Workstream on a remote host.
    New {
        alias: String,
        source_workstream_id: String,
        revision: i64,
        /// Provider to use when the source provider is not an eligible default.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Fork one remote live Workstream at its last completed native turn.
    Fork {
        alias: String,
        source_workstream_id: String,
        revision: i64,
    },
    /// Reopen one exact unresolved creation operation on a registered SSH host.
    RecoverOperation { alias: String, operation_id: String },
    /// Clear one remote result-attention revision without provider input.
    Acknowledge {
        alias: String,
        workstream_id: String,
        attention_revision: i64,
    },
    /// Attach the current terminal directly to a remote native Runtime.
    Attach {
        alias: String,
        workstream_id: String,
    },
    /// Forget one SSH registration after identity/generation/capability drift.
    Reset { alias: String },
}

const fn is_provider_surface_command(command: Option<&Commands>) -> bool {
    matches!(
        command,
        Some(
            Commands::ProviderAttach { .. }
                | Commands::ProviderRemoteAttach { .. }
                | Commands::ProviderRemoteObserverReview { .. }
                | Commands::ObserverReview
                | Commands::RemoteObserverReview
                | Commands::RemoteAttach { .. }
                | Commands::RuntimeLaunch { .. }
                | Commands::OpenCodeObserver { .. }
        )
    )
}

fn execute(cli: Cli) -> Result<(), AppError> {
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

fn runtime_launch(
    root: &StateRoot,
    runtime_id: &str,
    mut program: Vec<std::ffi::OsString>,
) -> Result<(), AppError> {
    let runtime_id = RuntimeId::from_str(runtime_id).map_err(AppError::InvalidRuntimeId)?;
    let paths = RuntimePaths::for_runtime(root.base(), runtime_id);
    await_launch_release(&paths)?;
    let executable = program.remove(0);
    let mut command = Command::new(executable);
    command.args(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(AppError::RuntimeExec(command.exec()))
    }
    #[cfg(not(unix))]
    {
        let status = command.status().map_err(AppError::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(AppError::RuntimeExited)
        }
    }
}

struct OpenCodeObserverArguments {
    runtime_id: String,
    generation: String,
    port: u16,
    session_id: String,
    pane_pid: u32,
    cwd: PathBuf,
    provider_birth: String,
}

fn opencode_observer(
    root: &StateRoot,
    arguments: OpenCodeObserverArguments,
) -> Result<(), AppError> {
    let context = crate::provider::opencode::OpenCodeObserverContext {
        runtime_id: RuntimeId::from_str(&arguments.runtime_id)
            .map_err(AppError::InvalidRuntimeId)?,
        generation: arguments.generation,
        endpoint: crate::provider::opencode::OpenCodeEndpoint::loopback(arguments.port)
            .map_err(AppError::OpenCode)?,
        session: ProviderSessionId::new(
            crate::domain::ProviderKind::OpenCode,
            &arguments.session_id,
        )
        .map_err(AppError::Domain)?,
        pane_pid: arguments.pane_pid,
        cwd: arguments.cwd,
        provider_birth: arguments.provider_birth,
    };
    crate::provider::opencode::run_observer(root, &context).map_err(AppError::OpenCodeObserver)
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

fn should_prepare_codex_observer(capabilities: &[crate::protocol::ProviderCapability]) -> bool {
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

fn host_command(root: &StateRoot, command: HostCommands) -> Result<(), AppError> {
    let mut catalog = ClientCatalog::open(root)?;
    match command {
        HostCommands::List => list_ssh_hosts(&catalog),
        HostCommands::Snapshot { alias } => snapshot_ssh_host(&catalog, &alias),
        HostCommands::Operations { alias } => operations_ssh_host(&catalog, &alias),
        HostCommands::Doctor { alias } => doctor_ssh_host(&catalog, &alias),
        HostCommands::RegisterCheckout {
            alias,
            checkout,
            provider,
        } => register_remote_checkout(
            &catalog,
            &alias,
            &checkout,
            parse_optional_provider(provider.as_deref())?,
        ),
        HostCommands::PrepareObserver { alias } => prepare_remote_observer(&catalog, &alias),
        HostCommands::RemoveObserver { alias } => remove_remote_observer(&catalog, &alias),
        HostCommands::Start {
            alias,
            workstream_id,
            revision,
        } => start_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Recover {
            alias,
            workstream_id,
            revision,
        } => recover_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Park {
            alias,
            workstream_id,
            revision,
        } => park_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Archive {
            alias,
            workstream_id,
            revision,
        } => archive_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Restore {
            alias,
            workstream_id,
            revision,
        } => restore_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Rename {
            alias,
            workstream_id,
            revision,
            name,
        } => rename_remote_workstream(&catalog, &alias, &workstream_id, revision, &name),
        HostCommands::New {
            alias,
            source_workstream_id,
            revision,
            provider,
        } => new_remote_workstream(
            &catalog,
            &alias,
            &source_workstream_id,
            revision,
            parse_optional_provider(provider.as_deref())?,
        ),
        HostCommands::Fork {
            alias,
            source_workstream_id,
            revision,
        } => fork_remote_workstream(&catalog, &alias, &source_workstream_id, revision),
        HostCommands::RecoverOperation {
            alias,
            operation_id,
        } => recover_remote_operation(&catalog, &alias, &operation_id),
        HostCommands::Acknowledge {
            alias,
            workstream_id,
            attention_revision,
        } => acknowledge_remote_workstream(&catalog, &alias, &workstream_id, attention_revision),
        HostCommands::Attach {
            alias,
            workstream_id,
        } => attach_remote_workstream(&catalog, &alias, &workstream_id),
        HostCommands::Reset { alias } => {
            catalog.reset_ssh_host(&alias)?;
            println!("reset SSH host {alias}");
            Ok(())
        }
    }
}

fn register_remote(
    root: &StateRoot,
    host: &str,
    destination: Option<&str>,
    executable: Option<&Path>,
) -> Result<(), AppError> {
    let mut catalog = ClientCatalog::open(root)?;
    if destination.is_none()
        && executable.is_none()
        && let Some(existing) = catalog.host(host)?
    {
        let ClientHostTransport::Ssh {
            destination: existing_destination,
        } = existing.transport
        else {
            return Err(AppError::HostIsNotSsh);
        };
        return register_ssh_host(
            &mut catalog,
            host,
            &existing_destination,
            &existing.executable_path,
        );
    }
    let destination = destination.unwrap_or(host);
    let executable = executable.unwrap_or_else(|| Path::new(STANDARD_REMOTE_EXECUTABLE));
    register_ssh_host(&mut catalog, host, destination, executable)
}

fn register_ssh_host(
    catalog: &mut ClientCatalog,
    alias: &str,
    destination: &str,
    executable: &Path,
) -> Result<(), AppError> {
    let endpoint = ssh_endpoint(destination, executable)?;
    let client = HostClient::new(SystemCommandRunner);
    client
        .probe_ssh(&endpoint)?
        .ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    let identity = HostIdentity {
        host_id: hello.host_id,
        registry_generation: hello.registry_generation,
    };
    catalog.register_ssh_host(
        alias,
        &identity,
        executable,
        endpoint.destination.as_str(),
        hello.capabilities,
    )?;
    println!("registered remote host {alias}");
    Ok(())
}

fn list_ssh_hosts(catalog: &ClientCatalog) -> Result<(), AppError> {
    for host in catalog.ssh_hosts()? {
        let ClientHostTransport::Ssh { destination } = host.transport else {
            continue;
        };
        println!("{} {}", host.alias, destination);
    }
    Ok(())
}

fn snapshot_ssh_host(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let snapshot = HostClient::new(SystemCommandRunner).snapshot_ssh(&endpoint)?;
    println!("host: {alias}");
    for workstream in snapshot.workstreams {
        println!(
            "{} {} {}",
            workstream.workstream_id.short(),
            runtime_status_label(workstream.runtime_status),
            workstream.display_name
        );
    }
    Ok(())
}

fn operations_ssh_host(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let operations = HostClient::new(SystemCommandRunner).operations_ssh(&endpoint)?;
    print_operations(operations.operations.into_iter().map(|operation| {
        (
            operation.operation_id,
            operation.kind,
            operation.phase,
            operation.revision,
        )
    }));
    Ok(())
}

fn register_remote_checkout(
    catalog: &ClientCatalog,
    alias: &str,
    checkout: &str,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let capabilities = HostClient::new(SystemCommandRunner)
        .snapshot_ssh(&endpoint)?
        .provider_capabilities;
    let provider =
        crate::provider::select_registration_provider(&capabilities, requested_provider)?;
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::RegisterCheckout {
            checkout_path: checkout.to_owned(),
            provider,
        },
    )?;
    println!("registered workstream {workstream_id}");
    Ok(())
}

fn prepare_remote_observer(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    apply_remote_action(catalog, alias, crate::protocol::HostAction::PrepareObserver)?;
    println!("remote observer profile is ready for native hook review");
    Ok(())
}

fn remove_remote_observer(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    apply_remote_action(catalog, alias, crate::protocol::HostAction::RemoveObserver)?;
    println!("remote observer profile removed");
    Ok(())
}

fn doctor_ssh_host(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    let endpoint = registered_ssh_endpoint(catalog, alias)?;
    let client = HostClient::new(SystemCommandRunner);
    let build = client.probe_ssh(&endpoint)?;
    build.ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    catalog.verify_hello(alias, &hello)?;
    println!("host: {alias}");
    println!("build: {}", build.package_version);
    println!("control ABI: {}", build.control_abi);
    println!("protocol: {}", build.protocol_version);
    println!("host schema: {}", build.host_schema_version);
    println!("release compatibility: ready");
    Ok(())
}

fn start_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Start {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("started remote workstream {workstream_id}");
    Ok(())
}

fn recover_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Recover {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("recovering remote workstream {workstream_id}");
    Ok(())
}

fn park_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Park {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("parked remote workstream {workstream_id}");
    Ok(())
}

fn archive_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Archive {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("archived remote workstream {workstream_id}");
    Ok(())
}

fn restore_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Restore {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("restored remote workstream {workstream_id}");
    Ok(())
}

fn rename_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
    name: &str,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Rename {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
            name: name.to_owned(),
        },
    )?;
    println!("renamed remote workstream {workstream_id}");
    Ok(())
}

fn new_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    source_workstream_id: &str,
    revision: i64,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let snapshot = HostClient::new(SystemCommandRunner).snapshot_ssh(&endpoint)?;
    let source_workstream_id = parse_workstream(source_workstream_id)?;
    let source = snapshot
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream_id == source_workstream_id)
        .ok_or(StateError::UnknownOpenWorkstream(source_workstream_id))?;
    let provider = crate::provider::select_new_provider(
        &snapshot.provider_capabilities,
        requested_provider,
        source.provider,
    )?;
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::NewWorkstream {
            source_workstream_id,
            expected_revision: revision,
            request_key: uuid::Uuid::new_v4().to_string(),
            provider,
        },
    )?;
    println!("started independent workstream {workstream_id}");
    Ok(())
}

fn fork_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    source_workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::ForkWorkstream {
            source_workstream_id: parse_workstream(source_workstream_id)?,
            expected_revision: revision,
            request_key: uuid::Uuid::new_v4().to_string(),
        },
    )?;
    println!("forked workstream {workstream_id}");
    Ok(())
}

fn recover_remote_operation(
    catalog: &ClientCatalog,
    alias: &str,
    operation_id: &str,
) -> Result<(), AppError> {
    let operation_id = parse_operation(operation_id)?;
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::RecoverOperation { operation_id },
    )?;
    println!("recovered operation {operation_id}; workstream {workstream_id}");
    Ok(())
}

fn acknowledge_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    attention_revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::AcknowledgeAttention {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: attention_revision,
        },
    )?;
    println!("acknowledged remote workstream {workstream_id}");
    Ok(())
}

fn attach_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let workstream_id = parse_workstream(workstream_id)?;
    let runtime_id = HostClient::new(SystemCommandRunner)
        .snapshot_ssh(&endpoint)?
        .workstreams
        .into_iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .and_then(|workstream| workstream.runtime_id)
        .ok_or(AppError::RemoteRuntimeUnavailable)?;
    attach_ssh(&endpoint, runtime_id)?;
    Ok(())
}

/// Runs an attachment only inside the presentation provider pane.
///
/// A provider pane is reserved for native provider bytes. The navigator refreshes
/// lifecycle state independently, so an unavailable or unexpectedly stopped
/// Runtime must leave this pane blank rather than render a CLI diagnostic.
fn provider_attach(
    root: &StateRoot,
    workstream_id: &str,
    presentation_socket: PathBuf,
    presentation_session: String,
    attempt_id: &str,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let attempt_id =
        uuid::Uuid::parse_str(attempt_id).map_err(AppError::InvalidAttachmentAttempt)?;
    presentation.report_attachment_phase(attempt_id, AttachmentPhase::Running)?;
    let outcome = (|| -> Result<(), AppError> {
        let workstream_id = parse_workstream(workstream_id)?;
        let mut registry = HostRegistry::open(root)?;
        attach(root, &mut registry, workstream_id)
    })();
    let phase = if outcome.is_ok() {
        AttachmentPhase::Completed
    } else {
        AttachmentPhase::Failed
    };
    presentation.report_attachment_phase(attempt_id, phase)?;
    provider_wait()
}

/// Runs an SSH attachment only inside the presentation provider pane.
///
/// The remote `_attach` endpoint follows the same no-diagnostics rule, while
/// the navigator's normal polling displays the resulting bounded state.
fn provider_remote_attach(
    root: &StateRoot,
    host_alias: &str,
    workstream_id: &str,
    presentation_socket: PathBuf,
    presentation_session: String,
    attempt_id: &str,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let attempt_id =
        uuid::Uuid::parse_str(attempt_id).map_err(AppError::InvalidAttachmentAttempt)?;
    presentation.report_attachment_phase(attempt_id, AttachmentPhase::Running)?;
    let outcome = (|| -> Result<(), AppError> {
        let catalog = ClientCatalog::open(root)?;
        attach_remote_workstream(&catalog, host_alias, workstream_id)
    })();
    let phase = if outcome.is_ok() {
        AttachmentPhase::Completed
    } else {
        AttachmentPhase::Failed
    };
    presentation.report_attachment_phase(attempt_id, phase)?;
    provider_wait()
}

/// Runs the remote observer review only in the presentation provider pane.
/// Codex owns every visible byte. This helper intentionally discards transport
/// diagnostics and returns to the blank pane after the native review exits.
fn provider_remote_observer_review(root: &StateRoot, host_alias: &str) -> Result<(), AppError> {
    let _ = (|| -> Result<(), AppError> {
        let catalog = ClientCatalog::open(root)?;
        let endpoint = checked_ssh_endpoint(&catalog, host_alias)?;
        crate::transport::review_observer_ssh(&endpoint)?;
        Ok(())
    })();
    provider_wait()
}

fn registered_ssh_endpoint(catalog: &ClientCatalog, alias: &str) -> Result<SshEndpoint, AppError> {
    let host = catalog.host(alias)?.ok_or(AppError::UnknownHostAlias)?;
    let ClientHostTransport::Ssh { destination } = host.transport else {
        return Err(AppError::HostIsNotSsh);
    };
    ssh_endpoint(&destination, &host.executable_path)
}

fn checked_ssh_endpoint(catalog: &ClientCatalog, alias: &str) -> Result<SshEndpoint, AppError> {
    let endpoint = registered_ssh_endpoint(catalog, alias)?;
    let client = HostClient::new(SystemCommandRunner);
    client
        .probe_ssh(&endpoint)?
        .ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    catalog.verify_hello(alias, &hello)?;
    Ok(endpoint)
}

fn apply_remote_action(
    catalog: &ClientCatalog,
    alias: &str,
    action: crate::protocol::HostAction,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let client = HostClient::new(SystemCommandRunner);
    client.apply_ssh(&endpoint, action)?;
    Ok(())
}

fn create_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    action: crate::protocol::HostAction,
) -> Result<WorkstreamId, AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    Ok(HostClient::new(SystemCommandRunner).create_ssh(&endpoint, action)?)
}

fn ssh_endpoint(destination: &str, executable: &Path) -> Result<SshEndpoint, AppError> {
    let destination = SshDestination::parse(destination)?;
    let executable = executable
        .to_str()
        .ok_or(AppError::RemoteExecutableNotUtf8)
        .and_then(|value| RemoteExecutable::parse(value).map_err(AppError::Transport))?;
    Ok(SshEndpoint::new(destination, executable))
}

const fn runtime_status_label(status: crate::domain::RuntimeStatus) -> &'static str {
    match status {
        crate::domain::RuntimeStatus::Starting => "starting",
        crate::domain::RuntimeStatus::Idle | crate::domain::RuntimeStatus::Attention => "idle",
        crate::domain::RuntimeStatus::Working => "working",
        crate::domain::RuntimeStatus::Stopped => "parked",
        crate::domain::RuntimeStatus::Unknown => "unknown",
        crate::domain::RuntimeStatus::Unreachable => "unreachable",
    }
}

fn acknowledge(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    attention_revision: i64,
) -> Result<(), AppError> {
    let revision = crate::domain::Revision::try_from(attention_revision)
        .map_err(|_| AppError::InvalidAttentionRevision)?;
    registry.acknowledge_result_attention(workstream_id, revision)?;
    println!("acknowledged workstream {workstream_id}");
    Ok(())
}

fn provider_wait() -> Result<(), AppError> {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

fn register(
    registry: &mut HostRegistry,
    checkout: &Path,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let repository = crate::repository::inspect(checkout)?;
    let capabilities = crate::provider::discover_capabilities(registry)?;
    let provider =
        crate::provider::select_registration_provider(&capabilities, requested_provider)?;
    crate::provider::require_new_eligible(registry, provider)?;
    let registered = registry.register_external_workstream_with_metadata(
        &repository.project_root,
        &repository.display_name,
        repository.remote_identity_fingerprint.as_deref(),
        repository.remote_identity_display.as_deref(),
        provider,
    )?;
    println!("registered workstream {}", registered.workstream_id);
    Ok(())
}

fn new_workstream(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let request_key = uuid::Uuid::new_v4().to_string();
    let source_provider = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == source_workstream_id)
        .ok_or(StateError::UnknownOpenWorkstream(source_workstream_id))?
        .provider;
    let capabilities = crate::provider::discover_capabilities(registry)?;
    let provider =
        crate::provider::select_new_provider(&capabilities, requested_provider, source_provider)?;
    let workstream_id = actions::start_independent_workstream(
        root,
        registry,
        source_workstream_id,
        None,
        &request_key,
        provider,
    )?;
    println!("started independent workstream {workstream_id}");
    Ok(())
}

fn fork_workstream(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let workstream_id = actions::fork_workstream(
        root,
        registry,
        source_workstream_id,
        None,
        uuid::Uuid::new_v4().to_string(),
    )?;
    println!("forked workstream {workstream_id}");
    Ok(())
}

fn setup(root: &StateRoot, registry: &mut HostRegistry, skip_review: bool) -> Result<(), AppError> {
    match prepare_observer_activation(root, registry)? {
        ObserverActivation::Ready => {
            println!("observer profile is already ready");
            Ok(())
        }
        ObserverActivation::ReviewRequired if skip_review => {
            println!("observer profile installed; native hook trust remains pending");
            Ok(())
        }
        ObserverActivation::ReviewRequired => {
            native_trust_review(root)?;
            let integration = registry
                .codex_integration()?
                .ok_or(AppError::ObserverNotInstalled)?;
            let manager = observer_profile(root)?;
            if finalize_native_trust(registry, &manager, &integration.ownership)? {
                println!("observer profile is ready");
                Ok(())
            } else {
                Err(AppError::NativeTrustReviewIncomplete)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverActivation {
    Ready,
    ReviewRequired,
}

/// Reconciles the exact observer declaration before native work can begin.
///
/// `wsnav` itself is the explicit user intent for this bounded setup action.
/// It never trusts a hook, rewrites an unowned declaration, or changes a
/// profile while a managed Runtime is live.
pub(crate) fn prepare_observer_activation(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<ObserverActivation, AppError> {
    let manager = observer_profile(root)?;
    prepare_observer_activation_with_manager(registry, &manager)
}

fn prepare_observer_activation_with_manager(
    registry: &mut HostRegistry,
    manager: &ObserverProfile,
) -> Result<ObserverActivation, AppError> {
    let existing = registry.codex_integration()?;
    let Some(integration) = existing else {
        if registry.has_live_runtime()? {
            return Err(AppError::LiveRuntimePreventsObserverActivation);
        }
        let ownership = manager.install(uuid::Uuid::new_v4().to_string(), None)?;
        registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?;
        return Ok(ObserverActivation::ReviewRequired);
    };

    if integration.ownership.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
        if registry.has_live_runtime()? {
            return Err(AppError::LiveRuntimePreventsObserverActivation);
        }
        let ownership = manager.update(&integration.ownership)?;
        registry.replace_codex_integration(
            &integration.ownership,
            ownership,
            IntegrationLifecycle::TrustPending,
        )?;
        return Ok(ObserverActivation::ReviewRequired);
    }

    let ownership = match manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    ) {
        Ok(ownership) => ownership,
        Err(crate::provider::codex::profile::ProfileError::OwnershipMismatch) => {
            if registry.has_live_runtime()? {
                return Err(AppError::LiveRuntimePreventsObserverActivation);
            }
            let ownership = manager.update(&integration.ownership)?;
            registry.replace_codex_integration(
                &integration.ownership,
                ownership,
                IntegrationLifecycle::TrustPending,
            )?;
            return Ok(ObserverActivation::ReviewRequired);
        }
        Err(error) => return Err(AppError::Profile(error)),
    };
    if finalize_native_trust(registry, manager, &ownership)? {
        return Ok(ObserverActivation::Ready);
    }
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsObserverActivation);
    }
    if integration.lifecycle != IntegrationLifecycle::TrustPending {
        registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?;
    }
    Ok(ObserverActivation::ReviewRequired)
}

fn update_observer(root: &StateRoot, registry: &mut HostRegistry) -> Result<(), AppError> {
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsUpdate);
    }
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let ownership = observer_profile(root)?.update(&integration.ownership)?;
    if ownership == integration.ownership {
        println!("observer profile is already current");
        return Ok(());
    }
    registry.replace_codex_integration(
        &integration.ownership,
        ownership,
        IntegrationLifecycle::TrustPending,
    )?;
    println!("observer profile updated; open a fresh wsnav to complete native hook review");
    Ok(())
}

fn doctor(root: &StateRoot, registry: &mut HostRegistry) -> Result<(), AppError> {
    actions::reconcile_observer_trust(root, registry)?;
    let integration = registry.codex_integration()?;
    let Some(integration) = integration else {
        println!("observer: not installed");
        return Ok(());
    };
    if integration.ownership.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
        println!("observer: update required");
        return Ok(());
    }
    let manager = observer_profile(root)?;
    match manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    ) {
        Err(crate::provider::codex::profile::ProfileError::UpdateRequired) => {
            println!("observer: update required");
            return Ok(());
        }
        Err(error) => return Err(AppError::Profile(error)),
        Ok(_) => {}
    }
    if integration.lifecycle == IntegrationLifecycle::Ready
        && manager.verify_native_trust(&integration.ownership).is_err()
    {
        println!("observer: trust pending");
        return Ok(());
    }
    println!("observer: {:?}", integration.lifecycle);
    Ok(())
}

fn remove_observer(root: &StateRoot, registry: &mut HostRegistry) -> Result<(), AppError> {
    remove_observer_exact(root, registry)?;
    println!("observer profile removed");
    Ok(())
}

/// Removes only the exact observer declaration. The remote control service
/// uses this silent helper so its protocol stdout remains one framed response.
pub(crate) fn remove_observer_exact(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), AppError> {
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsRemoval);
    }
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    observer_profile(root)?.remove(&integration.ownership)?;
    registry.remove_codex_integration(&integration.ownership)?;
    Ok(())
}

fn trust_observer(root: &StateRoot, registry: &mut HostRegistry) -> Result<(), AppError> {
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let manager = observer_profile(root)?;
    if finalize_native_trust(registry, &manager, &integration.ownership)? {
        println!("observer profile marked ready");
        Ok(())
    } else {
        Err(AppError::NativeTrustReviewIncomplete)
    }
}

/// Verifies Codex's own completed native review before recording this observer
/// as usable. `false` means the exact owned profile is still untrusted; other
/// profile errors fail closed instead of starting or marking an observer ready.
fn finalize_native_trust(
    registry: &mut HostRegistry,
    manager: &ObserverProfile,
    ownership: &crate::provider::codex::profile::ProfileOwnership,
) -> Result<bool, AppError> {
    match manager.verify_native_trust(ownership) {
        Ok(()) => {
            let ownership = manager.install(ownership.owner_id.clone(), Some(ownership))?;
            registry.record_codex_integration(ownership, IntegrationLifecycle::Ready)?;
            Ok(true)
        }
        Err(crate::provider::codex::profile::ProfileError::NativeTrustPending) => Ok(false),
        Err(error) => Err(AppError::Profile(error)),
    }
}

/// Runs only in the presentation's provider pane. Native Codex owns every
/// visible byte while the user reviews the exact hook declaration. After exit,
/// this helper silently reconciles native trust and returns the pane to its
/// blank wait state.
fn observer_review(root: &StateRoot) -> Result<(), AppError> {
    observer_review_once(root);
    provider_wait()
}

fn observer_review_once(root: &StateRoot) {
    let _ = native_trust_review_in_provider_pane(root);
    let _ = reconcile_observer_review(root);
}

fn reconcile_observer_review(root: &StateRoot) -> Result<(), AppError> {
    let mut registry = HostRegistry::open(root)?;
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let manager = observer_profile(root)?;
    let _ = finalize_native_trust(&mut registry, &manager, &integration.ownership)?;
    Ok(())
}

fn native_trust_review_in_provider_pane(root: &StateRoot) -> Result<(), AppError> {
    let review_root = root.base().join("review");
    fs::create_dir_all(&review_root).map_err(AppError::Io)?;
    let review_cwd = review_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&review_cwd).map_err(AppError::Io)?;
    let result = Command::new("codex")
        .args(["--profile", "wsnav-observer", "-C"])
        .arg(&review_cwd)
        .status()
        .map_err(AppError::Io);
    let remove = fs::remove_dir_all(&review_cwd).map_err(AppError::Io);
    let _ = fs::remove_dir(&review_root);
    result?;
    remove?;
    Ok(())
}

fn native_trust_review(root: &StateRoot) -> Result<(), AppError> {
    let review_root = root.base().join("review");
    fs::create_dir_all(&review_root).map_err(AppError::Io)?;
    let review_cwd = review_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&review_cwd).map_err(AppError::Io)?;

    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), RuntimeId::new()),
    );
    let launch = NativeLaunch {
        cwd: review_cwd.clone(),
        program: vec![
            "codex".into(),
            "--profile".into(),
            "wsnav-observer".into(),
            "-C".into(),
            review_cwd.clone().into_os_string(),
        ],
        environment: BTreeMap::new(),
    };
    if let Err(error) = runtime.start(&launch) {
        let _ = runtime.park();
        let _ = fs::remove_dir_all(&review_cwd);
        let _ = fs::remove_dir(&review_root);
        return Err(AppError::Runtime(error));
    }
    let attach = runtime.attach_command().status().map_err(AppError::Io);
    let park = runtime.park();
    let remove = fs::remove_dir_all(&review_cwd).map_err(AppError::Io);
    let _ = fs::remove_dir(&review_root);
    attach?;
    park?;
    remove?;
    Ok(())
}

fn start(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    match actions::start(root, registry, workstream_id, None)? {
        actions::StartOutcome::Started => println!("started workstream {workstream_id}"),
        actions::StartOutcome::AlreadyLive => {
            println!("workstream {workstream_id} is already live");
        }
    }
    Ok(())
}

fn recover(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    match actions::recover(root, registry, workstream_id, None)? {
        actions::StartOutcome::Started => {
            println!("recovering workstream {workstream_id}; complete native Codex resume");
        }
        actions::StartOutcome::AlreadyLive => {
            println!("workstream {workstream_id} is already live");
        }
    }
    Ok(())
}

#[cfg(test)]
fn codex_launch_program(
    cwd: &Path,
    binding: Option<&crate::state::ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    actions::codex_launch_program(cwd, binding)
}

fn observer_profile(root: &StateRoot) -> Result<ObserverProfile, AppError> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or(AppError::CodexHomeUnavailable)?;
    let executable = env::current_exe().map_err(AppError::Io)?;
    Ok(ObserverProfile::new(codex_home, executable, root.base()))
}

fn observe_hook(state_root: Option<PathBuf>) {
    // Drain before inspecting state or process evidence. Codex can still be
    // writing a large lifecycle payload when an unmanaged hook is rejected.
    let Ok(observation) = drain_and_parse(&mut std::io::stdin().lock()) else {
        return;
    };
    let Some(state_root) = state_root else {
        return;
    };
    let Ok(root) = StateRoot::create(state_root) else {
        return;
    };
    let Ok(mut registry) = HostRegistry::open(&root) else {
        return;
    };
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let Ok(candidates) = registry.hook_runtime_candidates() else {
        return;
    };
    let matches = candidates
        .into_iter()
        .filter(|record| record.cwd.as_path() == Path::new(&observation.cwd))
        .filter_map(|record| {
            let paths =
                RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)
                    .ok()?;
            let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
            let RuntimeProbe::Live {
                pane_pid,
                cwd,
                process_birth: Some(actual_birth),
                ..
            } = runtime.probe().ok()?
            else {
                return None;
            };
            let expected_birth = record.process_birth.as_deref()?;
            (cwd == record.cwd
                && actual_birth == expected_birth
                && is_direct_provider_hook(pane_pid, expected_birth))
            .then_some(record)
        })
        .collect::<Vec<_>>();
    let [record] = matches.as_slice() else {
        return;
    };
    let metadata = if matches!(observation.event, LifecycleEvent::SessionStart) {
        match EphemeralAppServer::default().read_thread_for_hook(&observation.native_session_id) {
            Ok(metadata) => Some(metadata),
            Err(_) => return,
        }
    } else {
        None
    };
    let Ok(session_id) = ProviderSessionId::codex(observation.native_session_id.clone()) else {
        return;
    };
    if registry
        .apply_lifecycle_observation(record.runtime_id, &record.tmux_generation, observation)
        .is_ok()
        && let Some(metadata) = metadata
    {
        let _ = registry.record_thread_metadata(
            record.runtime_id,
            &session_id,
            metadata.name.as_deref(),
        );
    }
}

fn rename(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    revision: Revision,
    name: &str,
) -> Result<(), AppError> {
    actions::rename(registry, workstream_id, revision, name)?;
    println!("renamed workstream {workstream_id}");
    Ok(())
}

fn attach(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let record = actions::preflight_attachment(root, registry, workstream_id)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    let mut command = runtime.attach_command();
    command.stderr(Stdio::null());
    let status = command.status().map_err(AppError::Io)?;
    if status.success()
        || actions::await_deliberate_park(root, record.runtime_id, record.workstream_id)?
    {
        Ok(())
    } else {
        Err(AppError::AttachFailed)
    }
}

fn park(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    actions::park(root, registry, workstream_id, None)?;
    println!("parked workstream {workstream_id}");
    Ok(())
}

fn archive(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    revision: Revision,
) -> Result<(), AppError> {
    actions::archive(root, registry, workstream_id, revision)?;
    println!("archived workstream {workstream_id}");
    Ok(())
}

fn restore(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    revision: Revision,
) -> Result<(), AppError> {
    actions::restore(registry, workstream_id, revision)?;
    println!("restored workstream {workstream_id}");
    Ok(())
}

fn status(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    actions::reconcile_lost_runtimes(root, registry)?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(AppError::NoRuntime(workstream_id))?;
    let record = overview.runtime.ok_or(AppError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    let probe = runtime.probe()?;
    let binding = registry.binding_for_runtime(record.runtime_id)?.is_some();
    let attention = overview.attention;
    println!("workstream: {:?}", overview.lifecycle);
    println!("lifecycle: {:?}", record.status);
    println!("private runtime: {}", runtime_probe_label(&probe));
    println!(
        "provider binding: {}",
        if binding { "bound" } else { "pending" }
    );
    println!(
        "result attention: {}",
        if attention
            .as_ref()
            .and_then(|value| value.result_unseen_since_revision)
            .is_some()
        {
            "unseen"
        } else {
            "none"
        }
    );
    println!(
        "recovery attention: {}",
        if attention
            .as_ref()
            .and_then(|value| value.recovery_unseen_since_revision)
            .is_some()
        {
            "unseen"
        } else {
            "none"
        }
    );
    Ok(())
}

fn operations(registry: &HostRegistry) -> Result<(), AppError> {
    let operations = registry.unresolved_operation_overviews()?;
    print_operations(operations.into_iter().map(|operation| {
        (
            operation.operation_id,
            operation.kind,
            operation.phase,
            operation.revision.value(),
        )
    }));
    Ok(())
}

fn print_operations(
    operations: impl IntoIterator<
        Item = (
            OperationId,
            crate::domain::OperationKind,
            crate::domain::OperationPhase,
            i64,
        ),
    >,
) {
    let mut any = false;
    for (operation_id, kind, phase, revision) in operations {
        any = true;
        println!("operation {operation_id} {kind:?} {phase:?} revision {revision}");
    }
    if !any {
        println!("no unresolved operations");
    }
}

fn recover_operation(
    root: &StateRoot,
    registry: &mut HostRegistry,
    operation_id: OperationId,
) -> Result<(), AppError> {
    let workstream_id = actions::recover_managed_operation(root, registry, operation_id)?;
    println!("recovered operation {operation_id}; workstream {workstream_id}");
    Ok(())
}

const fn runtime_probe_label(probe: &RuntimeProbe) -> &'static str {
    match probe {
        RuntimeProbe::Live { .. } => "live",
        RuntimeProbe::Missing => "missing",
        RuntimeProbe::Unknown { .. } => "unknown",
    }
}

fn parse_workstream(value: &str) -> Result<WorkstreamId, AppError> {
    WorkstreamId::from_str(value).map_err(AppError::InvalidWorkstreamId)
}

fn parse_optional_provider(
    value: Option<&str>,
) -> Result<Option<crate::domain::ProviderKind>, AppError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|error| AppError::State(StateError::Domain(error)))
        })
        .transpose()
}

fn parse_operation(value: &str) -> Result<OperationId, AppError> {
    OperationId::from_str(value).map_err(AppError::InvalidOperationId)
}

fn parse_revision(value: i64) -> Result<Revision, AppError> {
    Revision::try_from(value).map_err(|_| AppError::InvalidWorkstreamRevision)
}

fn default_state_root() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".wsnav-state"))
        .join("wsnav")
}

/// User-facing local-command failures.
#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error("native tmux attach failed")]
    AttachFailed,
    #[error("attention revision is invalid")]
    InvalidAttentionRevision,
    #[error("invalid workstream ID")]
    InvalidWorkstreamId(uuid::Error),
    #[error("invalid operation ID")]
    InvalidOperationId(uuid::Error),
    #[error("workstream revision is invalid")]
    InvalidWorkstreamRevision,
    #[error("invalid runtime ID")]
    InvalidRuntimeId(uuid::Error),
    #[error("invalid provider attachment attempt")]
    InvalidAttachmentAttempt(uuid::Error),
    #[error("host alias is not registered")]
    UnknownHostAlias,
    #[error("host alias is not an SSH host")]
    HostIsNotSsh,
    #[error("remote executable path is not valid UTF-8")]
    RemoteExecutableNotUtf8,
    #[error("remote Workstream has no live Runtime to attach")]
    RemoteRuntimeUnavailable,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("native provider exec failed")]
    RuntimeExec(std::io::Error),
    #[cfg(not(unix))]
    #[error("native provider exited during the internal launch handoff")]
    RuntimeExited,
    #[error("workstream {0} has no runtime")]
    NoRuntime(WorkstreamId),
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("observer profile is not installed; open wsnav to activate it")]
    ObserverNotInstalled,
    #[error(
        "native hook trust remains pending; open wsnav and approve the exact observer hooks in Codex"
    )]
    NativeTrustReviewIncomplete,
    #[error("observer profile removal is refused while a managed runtime is live")]
    LiveRuntimePreventsRemoval,
    #[error("observer profile update is refused while a managed runtime is live")]
    LiveRuntimePreventsUpdate,
    #[error("observer activation is refused while a managed runtime is live")]
    LiveRuntimePreventsObserverActivation,
    #[error(transparent)]
    Repository(#[from] crate::repository::RepositoryError),
    #[error(transparent)]
    BuildInfo(#[from] crate::build_info::BuildInfoError),
    #[error(transparent)]
    Profile(#[from] crate::provider::codex::profile::ProfileError),
    #[error(transparent)]
    Provider(#[from] crate::provider::ProviderReadinessError),
    #[error(transparent)]
    ProviderSelection(#[from] crate::provider::ProviderSelectionError),
    #[error(transparent)]
    Domain(#[from] crate::domain::DomainError),
    #[error(transparent)]
    OpenCode(#[from] crate::provider::opencode::OpenCodeError),
    #[error(transparent)]
    OpenCodeObserver(#[from] crate::provider::opencode::OpenCodeObserverError),
    #[error(transparent)]
    AppServer(#[from] crate::provider::codex::app_server::AppServerError),
    #[error(transparent)]
    Navigator(#[from] crate::navigator::NavigatorError),
    #[error(transparent)]
    Presentation(#[from] crate::presentation::PresentationError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    Remote(#[from] crate::remote::RemoteError),
    #[error(transparent)]
    Action(#[from] crate::actions::ActionError),
    #[error(transparent)]
    Transport(#[from] crate::transport::TransportError),
    #[error(transparent)]
    State(#[from] crate::state::StateError),
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn resuming_uses_the_exact_bound_native_session() {
        let binding = crate::state::ProviderBinding {
            runtime_id: RuntimeId::new(),
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

        let user = Cli::try_parse_from(["wsnav", "attach", "00000000-0000-0000-0000-000000000001"])
            .unwrap();
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
        assert!(help.contains("Recover a lost private Runtime through Codex"));
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
            Cli::try_parse_from(["wsnav", "register", "/checkout", "--provider", "opencode"])
                .unwrap();
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
}
