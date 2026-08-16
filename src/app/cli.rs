use super::{Parser, PathBuf, Subcommand};

const ABOUT: &str =
    "A native-workflow terminal navigator for persistent coding workstreams across hosts.";
#[derive(Debug, Parser)]
#[command(name = "wsnav", about = ABOUT, version)]
pub(super) struct Cli {
    /// Private host state root. Defaults to `XDG_STATE_HOME/wsnav`.
    #[arg(long, global = true)]
    pub(super) state_root: Option<PathBuf>,
    #[command(subcommand)]
    pub(super) command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub(super) enum Commands {
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
    /// Show one local runtime's durable record and live private-tmux probe.
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
    /// Internal fixed presentation control helper. It accepts only the
    /// bounded action/source values emitted by the private tmux bindings.
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
    /// Internal utility-shell barrier. It disables pane retention before
    /// replacing itself with the account's ordinary interactive shell.
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
    /// Internal remote utility-shell barrier. It validates the exact private
    /// presentation pane, then replaces itself with one fixed SSH command.
    #[command(name = "_presentation_ssh_shell", hide = true)]
    PresentationRemoteShell {
        #[arg(long)]
        presentation_socket: PathBuf,
        #[arg(long)]
        presentation_session: String,
        #[arg(long)]
        destination: String,
        #[arg(long)]
        executable: PathBuf,
        #[arg(long)]
        workstream_id: String,
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
    /// Internal host-side remote presentation shell helper. It accepts only
    /// an opaque Workstream ID and resolves all state locally.
    #[command(name = "_presentation_remote_shell", hide = true)]
    RemotePresentationShell { workstream_id: String },
    /// Internal host-side remote literal C-b helper. It accepts only an opaque
    /// Workstream ID and targets the exact private Runtime socket.
    #[command(name = "_presentation_remote_literal", hide = true)]
    RemotePresentationLiteral { workstream_id: String },
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
    /// Internal state-free launch barrier for one short-lived `OpenCode`
    /// serve helper.
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

#[derive(Debug, Subcommand)]
pub(super) enum HostCommands {
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
    /// Recover one remote Workstream through its exact native resume flow.
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

pub(super) const fn is_provider_surface_command(command: Option<&Commands>) -> bool {
    matches!(
        command,
        Some(
            Commands::ProviderAttach { .. }
                | Commands::PresentationControl { .. }
                | Commands::PresentationShell { .. }
                | Commands::PresentationRemoteShell { .. }
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
