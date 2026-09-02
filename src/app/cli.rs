use super::{Parser, PathBuf, Subcommand};

const ABOUT: &str = "A native-workflow terminal navigator for persistent coding workstreams.";

#[derive(Debug, Parser)]
#[command(name = "wsnav", about = ABOUT, version)]
pub(super) struct Cli {
    /// Private local state root. Defaults to `XDG_STATE_HOME/wsnav`.
    #[arg(long, global = true)]
    pub(super) state_root: Option<PathBuf>,
    #[command(subcommand)]
    pub(super) command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub(super) enum Commands {
    /// Open the local two-pane Workstream Navigator presentation.
    Navigator,
    /// Inspect exact observer ownership and trust without changing it.
    Doctor,
    /// Remove only the exact unchanged observer profile after runtimes stop.
    RemoveObserver,
    /// Start the Workstream's native provider in its private tmux server.
    Start { workstream_id: String },
    /// Recover a lost private Runtime through its exact native resume flow.
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
    /// Show one local runtime's durable host-registry record.
    Status { workstream_id: String },
    /// List unresolved native-session creation operations without exposing request keys or provider data.
    Operations,
    /// Internal schema-15 Ratatui process run inside a presentation pane.
    #[command(name = "_navigator", hide = true)]
    NavigatorPane {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
    },
    /// Internal account-shell broker gate. Its successful stdout is one
    /// opaque capability consumed only by the adjacent shell wrapper.
    #[command(name = "_shell_gate", hide = true)]
    ShellGate {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        shell_leader_pid: u32,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<std::ffi::OsString>,
    },
    /// Internal account-shell launch helper. It consumes one opaque
    /// capability and replaces itself with the approved native provider.
    #[command(name = "_launch_helper", hide = true)]
    LaunchHelper {
        #[arg(long)]
        capability: String,
        #[arg(long)]
        provider: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<std::ffi::OsString>,
    },
    /// Internal interactive Codex observer setup from an exact provisional
    /// account shell. It is not a public setup or trust command.
    #[command(name = "_observer_setup", hide = true)]
    ObserverSetup {
        #[arg(long)]
        shell_leader_pid: u32,
        #[arg(long)]
        consent: bool,
    },
    /// Internal fixed two-pane presentation control helper.
    #[command(name = "_presentation_control", hide = true)]
    PresentationControl {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        action: String,
        #[arg(long)]
        source_pane: String,
        #[arg(long)]
        client_name: String,
    },
    /// Internal read-only primary-button validation for the presentation.
    #[command(name = "_presentation_mouse", hide = true)]
    PresentationMouse {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        target_pane: String,
        #[arg(long)]
        client_name: String,
    },
    /// Internal blank provider-pane placeholder before an exact attachment is selected.
    #[command(name = "_provider_wait", hide = true)]
    ProviderWait,
    /// Internal schema-15 provider attachment helper for a proven Runtime.
    #[command(name = "_provider_attach", hide = true)]
    ProviderAttach {
        workstream_id: String,
        #[arg(long)]
        expected_workstream_revision: i64,
        #[arg(long)]
        expected_runtime_id: String,
        #[arg(long)]
        expected_runtime_revision: i64,
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        attempt_id: String,
        #[arg(long)]
        provider_cycle: bool,
    },
    /// Internal passive Codex lifecycle hook entrypoint.
    #[command(name = "_hook", hide = true)]
    Hook,
    /// Internal one-shot launch barrier that replaces itself with the provider.
    #[command(name = "_runtime_launch", hide = true)]
    RuntimeLaunch {
        runtime_id: String,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        program: Vec<std::ffi::OsString>,
    },
    /// Internal generation-bound `OpenCode` lifecycle observer.
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
    /// Internal state-free launch barrier for one short-lived `OpenCode` serve helper.
    #[command(name = "_opencode_serve_barrier", hide = true)]
    OpenCodeServeBarrier {
        executable: PathBuf,
        project_root: PathBuf,
        port: u16,
    },
    /// Internal state-free guardian for one short-lived `OpenCode` serve helper.
    #[command(name = "_opencode_serve_guardian", hide = true)]
    OpenCodeServeGuardian {
        executable: PathBuf,
        project_root: PathBuf,
        port: u16,
    },
}

pub(super) const fn is_provider_pane_command(command: Option<&Commands>) -> bool {
    matches!(
        command,
        Some(
            Commands::PresentationControl { .. }
                | Commands::ProviderAttach { .. }
                | Commands::RuntimeLaunch { .. }
        )
    )
}

pub(super) const fn is_presentation_mouse_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::PresentationMouse { .. }))
}

pub(super) const fn is_observer_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::OpenCodeObserver { .. }))
}

pub(super) const fn is_shell_gate_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::ShellGate { .. }))
}

pub(super) const fn is_shell_launch_helper_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::LaunchHelper { .. }))
}

pub(super) const fn is_observer_setup_command(command: Option<&Commands>) -> bool {
    matches!(command, Some(Commands::ObserverSetup { .. }))
}
