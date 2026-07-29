//! Thin local CLI orchestration for the D1 native Codex slice.

use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    str::FromStr,
};

use clap::{Parser, Subcommand};
use thiserror::Error;

use crate::{
    domain::{RuntimeId, WorkstreamId},
    provider::codex::app_server::EphemeralAppServer,
    provider::codex::hooks::drain_and_parse,
    provider::codex::profile::ObserverProfile,
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{HostRegistry, IntegrationLifecycle, StateRoot},
};

const ABOUT: &str =
    "A native-workflow terminal navigator for persistent coding workstreams across hosts.";

/// Runs one direct local CLI command.
#[must_use]
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
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
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Install the owned observer profile and leave native hook trust pending.
    Setup,
    /// Confirm native hook trust after reviewing it in Codex's `/hooks` UI.
    TrustObserver,
    /// Inspect the exact observer ownership and trust lifecycle without changing it.
    Doctor,
    /// Remove only the exact unchanged owned observer profile after all runtimes stop.
    RemoveObserver,
    /// Register one existing Git checkout as the initial external workstream.
    Register { checkout: PathBuf },
    /// Start native Codex in a private tmux server for one registered workstream.
    Start { workstream_id: String },
    /// Attach this terminal directly to a live native Codex runtime.
    Attach { workstream_id: String },
    /// Park a runtime without deleting its checkout or Codex session history.
    Park { workstream_id: String },
    /// Show one local runtime's durable record and live private-tmux probe.
    Status { workstream_id: String },
    /// Rename the current managed Codex thread through its canonical name field.
    Rename { workstream_id: String, name: String },
    /// Internal passive Codex lifecycle hook entrypoint.
    #[command(name = "_hook", hide = true)]
    Hook,
}

fn execute(cli: Cli) -> Result<(), AppError> {
    if matches!(cli.command, Commands::Hook) {
        observe_hook(cli.state_root);
        return Ok(());
    }
    let root = StateRoot::create(cli.state_root.unwrap_or_else(default_state_root))?;
    let mut registry = HostRegistry::open(&root)?;
    match cli.command {
        Commands::Setup => setup(&mut registry),
        Commands::TrustObserver => trust_observer(&mut registry),
        Commands::Doctor => doctor(&registry),
        Commands::RemoveObserver => remove_observer(&mut registry),
        Commands::Register { checkout } => register(&mut registry, &checkout),
        Commands::Start { workstream_id } => {
            start(&root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Attach { workstream_id } => {
            attach(&root, &registry, parse_workstream(&workstream_id)?)
        }
        Commands::Park { workstream_id } => {
            park(&root, &mut registry, parse_workstream(&workstream_id)?)
        }
        Commands::Status { workstream_id } => {
            status(&root, &registry, parse_workstream(&workstream_id)?)
        }
        Commands::Rename {
            workstream_id,
            name,
        } => rename(&mut registry, parse_workstream(&workstream_id)?, &name),
        Commands::Hook => unreachable!("hook dispatch returns before state setup"),
    }
}

fn register(registry: &mut HostRegistry, checkout: &Path) -> Result<(), AppError> {
    let checkout = checkout.canonicalize().map_err(AppError::Io)?;
    let repository_identity = git_value(
        &checkout,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let default_base_ref = git_value(&checkout, &["rev-parse", "HEAD"])?;
    let registered =
        registry.register_external_workstream(checkout, repository_identity, default_base_ref)?;
    println!("registered workstream {}", registered.workstream_id);
    Ok(())
}

fn setup(registry: &mut HostRegistry) -> Result<(), AppError> {
    let manager = observer_profile()?;
    let existing = registry.codex_integration()?;
    let ownership = manager.install(
        uuid::Uuid::new_v4().to_string(),
        existing.as_ref().map(|integration| &integration.ownership),
    )?;
    let lifecycle = existing.map_or(IntegrationLifecycle::TrustPending, |integration| {
        integration.lifecycle
    });
    registry.record_codex_integration(ownership, lifecycle)?;
    println!(
        "observer profile installed; review and trust it in Codex /hooks, then run wsnav trust-observer"
    );
    Ok(())
}

fn doctor(registry: &HostRegistry) -> Result<(), AppError> {
    let integration = registry.codex_integration()?;
    let Some(integration) = integration else {
        println!("observer: not installed");
        return Ok(());
    };
    let manager = observer_profile()?;
    manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    )?;
    println!("observer: {:?}", integration.lifecycle);
    Ok(())
}

fn remove_observer(registry: &mut HostRegistry) -> Result<(), AppError> {
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsRemoval);
    }
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    observer_profile()?.remove(&integration.ownership)?;
    registry.remove_codex_integration(&integration.ownership)?;
    println!("observer profile removed");
    Ok(())
}

fn trust_observer(registry: &mut HostRegistry) -> Result<(), AppError> {
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let manager = observer_profile()?;
    let ownership = manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    )?;
    registry.record_codex_integration(ownership, IntegrationLifecycle::Ready)?;
    println!("observer profile marked ready");
    Ok(())
}

fn start(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    if integration.lifecycle != IntegrationLifecycle::Ready {
        return Err(AppError::ObserverNotReady);
    }
    let manager = observer_profile()?;
    manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    )?;
    let prior_runtime = registry.runtime_for_workstream(workstream_id)?;
    if let Some(prior_runtime) = &prior_runtime {
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let prior = PrivateRuntime::new(
            &tmux,
            &process_probe,
            RuntimePaths::for_runtime(root.base(), prior_runtime.runtime_id),
        );
        match prior.probe()? {
            RuntimeProbe::Live { .. } => {
                println!("workstream {workstream_id} is already live");
                return Ok(());
            }
            RuntimeProbe::Missing => {
                if !matches!(prior_runtime.status, crate::domain::RuntimeStatus::Stopped) {
                    registry
                        .mark_runtime_stopped(prior_runtime.runtime_id, prior_runtime.revision)?;
                }
            }
            RuntimeProbe::Unknown { .. } => return Err(AppError::RuntimeProbeAmbiguous),
        }
    }
    let prior_binding = prior_runtime
        .as_ref()
        .map(|runtime| registry.binding_for_runtime(runtime.runtime_id))
        .transpose()?
        .flatten();
    let record = registry.reserve_runtime(workstream_id)?;
    let paths = RuntimePaths::for_runtime(root.base(), record.runtime_id);
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
    let launch = NativeLaunch {
        cwd: record.cwd.clone(),
        program: codex_launch_program(&record.cwd, prior_binding.as_ref()),
        environment: BTreeMap::from([
            (
                "WSNAV_STATE_ROOT".into(),
                root.base().as_os_str().to_owned(),
            ),
            (
                "WSNAV_RUNTIME_ID".into(),
                record.runtime_id.to_string().into(),
            ),
            (
                "WSNAV_RUNTIME_GENERATION".into(),
                record.tmux_generation.clone().into(),
            ),
        ]),
    };
    if let Err(error) = runtime.start(&launch) {
        let _ = registry.mark_runtime_stopped(record.runtime_id, record.revision);
        return Err(AppError::Runtime(error));
    }
    println!("started workstream {workstream_id}");
    Ok(())
}

fn codex_launch_program(
    cwd: &Path,
    binding: Option<&crate::state::ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    let mut program = vec![
        "codex".into(),
        "--profile".into(),
        "wsnav-observer".into(),
        "-C".into(),
        cwd.as_os_str().to_owned(),
    ];
    if let Some(binding) = binding {
        program.push("resume".into());
        program.push(binding.native_session_id.clone().into());
    }
    program
}

fn observer_profile() -> Result<ObserverProfile, AppError> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or(AppError::CodexHomeUnavailable)?;
    let executable = env::current_exe().map_err(AppError::Io)?;
    Ok(ObserverProfile::new(codex_home, executable))
}

fn observe_hook(state_root: Option<PathBuf>) {
    let Some(state_root) =
        state_root.or_else(|| env::var_os("WSNAV_STATE_ROOT").map(PathBuf::from))
    else {
        return;
    };
    let Ok(runtime_id) = env::var("WSNAV_RUNTIME_ID") else {
        return;
    };
    let Ok(runtime_id) = RuntimeId::from_str(&runtime_id) else {
        return;
    };
    let Ok(generation) = env::var("WSNAV_RUNTIME_GENERATION") else {
        return;
    };
    let Ok(observation) = drain_and_parse(&mut std::io::stdin().lock()) else {
        return;
    };
    let Ok(root) = StateRoot::create(state_root) else {
        return;
    };
    let Ok(mut registry) = HostRegistry::open(&root) else {
        return;
    };
    let _ = registry.apply_hook_observation(runtime_id, &generation, observation);
}

fn rename(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    name: &str,
) -> Result<(), AppError> {
    let runtime = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(AppError::NoRuntime(workstream_id))?;
    let binding = registry
        .binding_for_runtime(runtime.runtime_id)?
        .ok_or(AppError::NoBinding(workstream_id))?;
    EphemeralAppServer::default().set_thread_name(&binding.native_session_id, name)?;
    registry.record_thread_name(runtime.runtime_id, &binding.native_session_id, name)?;
    println!("renamed workstream {workstream_id}");
    Ok(())
}

fn attach(
    root: &StateRoot,
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(AppError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), record.runtime_id),
    );
    let status = runtime.attach_command().status().map_err(AppError::Io)?;
    if status.success() {
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
    let record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(AppError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), record.runtime_id),
    );
    runtime.park()?;
    registry.mark_runtime_stopped(record.runtime_id, record.revision)?;
    println!("parked workstream {workstream_id}");
    Ok(())
}

fn status(
    root: &StateRoot,
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(AppError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), record.runtime_id),
    );
    println!("runtime: {}", record.runtime_id);
    println!("status: {:?}", runtime.probe()?);
    Ok(())
}

fn parse_workstream(value: &str) -> Result<WorkstreamId, AppError> {
    WorkstreamId::from_str(value).map_err(AppError::InvalidWorkstreamId)
}

fn default_state_root() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".wsnav-state"))
        .join("wsnav")
}

fn git_value(checkout: &Path, arguments: &[&str]) -> Result<String, AppError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(arguments)
        .output()
        .map_err(AppError::Io)?;
    if !output.status.success() {
        return Err(AppError::NotGitCheckout);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() || value.len() > 4096 || value.contains('\n') {
        return Err(AppError::NotGitCheckout);
    }
    Ok(value)
}

/// User-facing local-command failures.
#[derive(Debug, Error)]
enum AppError {
    #[error("native tmux attach failed")]
    AttachFailed,
    #[error("invalid workstream ID")]
    InvalidWorkstreamId(uuid::Error),
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("not a usable Git checkout")]
    NotGitCheckout,
    #[error("workstream {0} has no runtime")]
    NoRuntime(WorkstreamId),
    #[error("workstream {0} has no exact native Codex binding")]
    NoBinding(WorkstreamId),
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("observer profile is not installed; run wsnav setup")]
    ObserverNotInstalled,
    #[error(
        "observer profile trust is pending; complete native Codex /hooks review then run wsnav trust-observer"
    )]
    ObserverNotReady,
    #[error("observer profile removal is refused while a managed runtime is live")]
    LiveRuntimePreventsRemoval,
    #[error("private runtime probe is ambiguous; refusing to create another Codex process")]
    RuntimeProbeAmbiguous,
    #[error(transparent)]
    Profile(#[from] crate::provider::codex::profile::ProfileError),
    #[error(transparent)]
    AppServer(#[from] crate::provider::codex::app_server::AppServerError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    State(#[from] crate::state::StateError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resuming_uses_the_exact_bound_native_session() {
        let binding = crate::state::ProviderBinding {
            runtime_id: RuntimeId::new(),
            native_session_id: "exact-session".to_owned(),
            last_settled_turn_id: Some("settled-turn".to_owned()),
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
        assert!(matches!(parsed.unwrap().command, Commands::Hook));
        assert!(Cli::try_parse_from(["wsnav", "hook"]).is_err());
    }
}
