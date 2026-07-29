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
    provider::codex::hooks::drain_and_parse,
    runtime::{LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, SystemTmux},
    state::{HostRegistry, StateRoot},
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
    /// Internal passive Codex lifecycle hook entrypoint.
    #[command(hide = true)]
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

fn start(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let record = registry.reserve_runtime(workstream_id)?;
    let paths = RuntimePaths::for_runtime(root.base(), record.runtime_id);
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
    let launch = NativeLaunch {
        cwd: record.cwd.clone(),
        program: vec![
            "codex".into(),
            "--profile".into(),
            "wsnav-observer".into(),
            "-C".into(),
            record.cwd.into_os_string(),
        ],
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
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    State(#[from] crate::state::StateError),
}
