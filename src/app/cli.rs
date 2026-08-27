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
    /// Fork one live Workstream at its last completed native provider turn.
    ForkWorkstream { source_workstream_id: String },
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
    /// List unresolved Fork operations without exposing request keys or provider data.
    Operations,
    /// Reopen one exact unresolved Fork operation.
    RecoverOperation { operation_id: String },
    /// Rename the current managed provider thread when canonically supported.
    Rename {
        workstream_id: String,
        revision: i64,
        name: String,
    },
    /// Clear one observed result/recovery attention revision without provider input.
    Acknowledge {
        workstream_id: String,
        attention_revision: i64,
    },
    /// Internal Ratatui process run inside an owned presentation pane.
    #[command(name = "_navigator", hide = true)]
    NavigatorPane {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
    },
    /// Internal schema-14 Ratatui process run inside a D17 presentation pane.
    #[command(name = "_navigator_d17", hide = true)]
    NavigatorPaneD17 {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
    },
    /// Internal fixed presentation control helper.
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
    /// Internal local utility-shell launch barrier.
    #[command(name = "_presentation_shell", hide = true)]
    PresentationShell {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        shell: PathBuf,
        #[arg(long)]
        cwd: PathBuf,
    },
    /// Internal blank provider-pane placeholder before an exact attachment is selected.
    #[command(name = "_provider_wait", hide = true)]
    ProviderWait,
    /// Internal temporary native Codex observer-review surface.
    #[command(name = "_observer_review", hide = true)]
    ObserverReview,
    /// Internal local provider-pane attachment helper.
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
    /// Internal D16 generation-bound `OpenCode` lifecycle observer.
    #[command(name = "_opencode_observer_d16", hide = true)]
    OpenCodeObserverD16 {
        runtime_id: String,
        generation: String,
        port: u16,
        session_id: String,
        pane_pid: u32,
        cwd: PathBuf,
        provider_birth: String,
    },
    /// Internal D17 generation-bound `OpenCode` lifecycle observer.
    #[command(name = "_opencode_observer_d17", hide = true)]
    OpenCodeObserverD17 {
        runtime_id: String,
        generation: String,
        port: u16,
        session_id: String,
        pane_pid: u32,
        cwd: PathBuf,
        provider_birth: String,
    },
    /// Internal state-free D16 standby observer.
    #[command(name = "_opencode_observer_standby", hide = true)]
    OpenCodeObserverStandby {
        runtime_id: String,
        generation: String,
        port: u16,
        provider_version: String,
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
            Commands::ProviderAttach { .. }
                | Commands::PresentationControl { .. }
                | Commands::PresentationShell { .. }
                | Commands::ObserverReview
                | Commands::RuntimeLaunch { .. }
        )
    )
}

pub(super) const fn is_observer_command(command: Option<&Commands>) -> bool {
    matches!(
        command,
        Some(
            Commands::OpenCodeObserverD16 { .. }
                | Commands::OpenCodeObserverD17 { .. }
                | Commands::OpenCodeObserverStandby { .. }
        )
    )
}
