//! Disposable private tmux ownership for the local navigator presentation.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    domain::WorkstreamId,
    private_tmux::{TERMINAL_CAPABILITY_CONFIG, copy_mode_scroll_config},
    process::{BoundedProcessError, output_bounded},
};

const PRESENTATION_DIRECTORY: &str = "presentation";
const PRESENTATION_PREFIX: &str = "wsnav-presentation-";
const NAVIGATOR_WINDOW: &str = "navigator";
const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
const NAVIGATOR_WIDTH_HOOKS: [&str; 2] = ["client-attached", "window-resized"];
/// The normal narrow navigator width, including its outside borders.
const DEFAULT_NAVIGATOR_PANE_WIDTH: u16 = 32;
const PREFERRED_PROVIDER_PANE_WIDTH: u16 = 96;
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_ATTACHMENT_STATUS_BYTES: u64 = 4 * 1024;
const ATTACHMENT_STATUS_FILE: &str = "attachment.json";
const ROLE_OPTION: &str = "@wsnav_role";
const HOST_OPTION: &str = "@wsnav_host_alias";
const WORKSTREAM_OPTION: &str = "@wsnav_workstream_id";
const SHELL_CLAIM_OPTION: &str = "@wsnav_shell_claim";
const SHELL_CLAIM_ATTEMPTS: usize = 20;
const SHELL_CLAIM_RETRY: Duration = Duration::from_millis(5);
const TOPOLOGY_FORMAT: &str = "#{pane_id}\t#{@wsnav_role}\t#{@wsnav_host_alias}\t#{@wsnav_workstream_id}\t#{pane_dead}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}\t#{window_width}\t#{window_height}";
const PRESENTATION_TMUX_CONFIG_PREFIX: &str = concat!(
    "set -g status off\n",
    "set -g mouse on\n",
    "set -g remain-on-exit on\n",
    "set -g prefix C-b\n",
    "set -g prefix2 None\n",
    "unbind-key -a -T prefix\n",
    "unbind-key -a -T root\n",
);
const PRESENTATION_TMUX_CONFIG_SUFFIX: &str = concat!(
    "bind-key -T root MouseDown1Pane select-pane -t = \\; send-keys -M\n",
    "bind-key -T root MouseUp1Pane select-pane -t = \\; send-keys -M\n",
    "bind-key -T root MouseDrag1Pane if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -M\"\n",
    "bind-key -T root WheelUpPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -e\"\n",
    "bind-key -T root WheelDownPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"send-keys -M\"\n",
);

fn presentation_tmux_config() -> String {
    let copy_mode_scroll_config = copy_mode_scroll_config();
    [
        PRESENTATION_TMUX_CONFIG_PREFIX,
        TERMINAL_CAPABILITY_CONFIG,
        &copy_mode_scroll_config,
        PRESENTATION_TMUX_CONFIG_SUFFIX,
    ]
    .concat()
}

/// Actions exposed by the private presentation prefix table. The strings are
/// fixed internal ABI values; no arbitrary tmux command can enter this path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationAction {
    CreateOrFocusShell,
    SuppressSplit,
    CloseShell,
    FocusNext,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    LiteralCtrlB,
}

impl PresentationAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateOrFocusShell => "create-or-focus-shell",
            Self::SuppressSplit => "suppress-split",
            Self::CloseShell => "close-shell",
            Self::FocusNext => "focus-next",
            Self::FocusUp => "focus-up",
            Self::FocusDown => "focus-down",
            Self::FocusLeft => "focus-left",
            Self::FocusRight => "focus-right",
            Self::LiteralCtrlB => "literal-c-b",
        }
    }
}

impl FromStr for PresentationAction {
    type Err = PresentationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "create-or-focus-shell" => Ok(Self::CreateOrFocusShell),
            "suppress-split" => Ok(Self::SuppressSplit),
            "close-shell" => Ok(Self::CloseShell),
            "focus-next" => Ok(Self::FocusNext),
            "focus-up" => Ok(Self::FocusUp),
            "focus-down" => Ok(Self::FocusDown),
            "focus-left" => Ok(Self::FocusLeft),
            "focus-right" => Ok(Self::FocusRight),
            "literal-c-b" => Ok(Self::LiteralCtrlB),
            _ => Err(PresentationError::InvalidControlAction),
        }
    }
}

/// A role recognized only after exact private tmux evidence is parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationPaneRole {
    Navigator,
    Provider,
    Utility,
}

/// Ephemeral provider-pane attempt metadata read only by the local navigator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttachmentStatus {
    pub attempt_id: uuid::Uuid,
    pub host_alias: String,
    pub workstream_id: WorkstreamId,
    pub phase: AttachmentPhase,
}

/// Observable provider attachment phases. These never enter durable host state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentPhase {
    Pending,
    Running,
    Completed,
    Failed,
}

/// The exact private paths and tmux session owned by one navigator client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationPaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
    pub attachment_status: PathBuf,
    pub session_name: String,
}

impl PresentationPaths {
    /// Creates a collision-resistant private presentation location below one
    /// state root. The presentation has no durable identity or focus record.
    #[must_use]
    pub fn fresh(state_root: &Path) -> Self {
        let full_identifier = uuid::Uuid::new_v4().simple().to_string();
        let identifier = &full_identifier[..12];
        let directory = state_root
            .join(PRESENTATION_DIRECTORY)
            .join(format!("presentation-{identifier}"));
        Self {
            socket: directory.join("tmux.sock"),
            config: directory.join("tmux.conf"),
            attachment_status: directory.join(ATTACHMENT_STATUS_FILE),
            session_name: format!("{PRESENTATION_PREFIX}{identifier}"),
            directory,
        }
    }

    /// Validates that an internal navigator process can control only a
    /// presentation beneath the supplied state root.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket or session does not describe an exact
    /// private Workstream Navigator presentation.
    pub fn from_control(
        state_root: &Path,
        socket: PathBuf,
        session_name: String,
    ) -> Result<Self, PresentationError> {
        let parent = socket
            .parent()
            .ok_or_else(|| PresentationError::InvalidControlPath(socket.clone()))?;
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        let expected_session = presentation_session_name(parent);
        if parent.parent() != Some(presentation_root.as_path())
            || socket.file_name().is_none_or(|name| name != "tmux.sock")
            || expected_session.as_deref() != Some(&session_name)
        {
            return Err(PresentationError::InvalidControlPath(socket));
        }
        Ok(Self {
            config: parent.join("tmux.conf"),
            attachment_status: parent.join(ATTACHMENT_STATUS_FILE),
            directory: parent.to_path_buf(),
            socket,
            session_name,
        })
    }
}

/// Owns one disposable two-pane local presentation server.
#[derive(Clone, Debug)]
pub struct Presentation {
    paths: PresentationPaths,
    executable: PathBuf,
    state_root: PathBuf,
}

impl Presentation {
    /// Creates an unstarted presentation owner for the current executable.
    ///
    /// # Errors
    ///
    /// Returns an error when the current executable cannot be resolved.
    pub fn fresh(state_root: &Path) -> Result<Self, PresentationError> {
        let executable = std::env::current_exe().map_err(PresentationError::Io)?;
        Ok(Self::fresh_with_executable(state_root, executable))
    }

    /// Creates an owner with an explicitly fixed executable. This is used by
    /// disposable integration fixtures so a test harness can exercise the
    /// real hidden helper instead of becoming the helper itself.
    #[doc(hidden)]
    #[must_use]
    pub fn fresh_with_executable(state_root: &Path, executable: PathBuf) -> Self {
        Self {
            paths: PresentationPaths::fresh(state_root),
            executable,
            state_root: state_root.to_path_buf(),
        }
    }

    /// Reuses the one live owned presentation, or creates a fresh owner when
    /// no presentation is live. A detached presentation is intentionally kept
    /// so a later `wsnav` invocation can reconnect without disturbing any
    /// provider Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when an owned presentation is ambiguous, malformed, or
    /// cannot be queried through its exact private tmux socket.
    pub fn open_or_create(state_root: &Path) -> Result<(Self, bool), PresentationError> {
        let live = Self::discover_live(state_root)?;
        match live.as_slice() {
            [] => Ok((Self::fresh(state_root)?, true)),
            [presentation] => Ok((presentation.clone(), false)),
            _ => Err(PresentationError::AmbiguousPresentations),
        }
    }

    /// Reopens the exact owned presentation described by a hidden child
    /// command. This does not discover or use any ordinary tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the executable cannot be resolved or the supplied
    /// control values do not name an owned private presentation.
    pub fn from_control(
        state_root: &Path,
        socket: PathBuf,
        session_name: String,
    ) -> Result<Self, PresentationError> {
        Ok(Self {
            paths: PresentationPaths::from_control(state_root, socket, session_name)?,
            executable: std::env::current_exe().map_err(PresentationError::Io)?,
            state_root: state_root.to_path_buf(),
        })
    }

    #[must_use]
    pub fn paths(&self) -> &PresentationPaths {
        &self.paths
    }

    /// Creates exactly one private tmux server with a navigator pane and a
    /// blank provider-attachment pane. Neither command invokes a shell.
    ///
    /// # Errors
    ///
    /// Returns an error when the owned paths cannot be created or tmux rejects
    /// the private presentation setup.
    pub fn start(&self) -> Result<(), PresentationError> {
        create_paths(&self.paths)?;
        let mut arguments = vec![
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            self.paths.session_name.clone().into(),
            "-n".into(),
            NAVIGATOR_WINDOW.into(),
        ];
        arguments.extend(self.navigator_command());
        let result = self.invoke(Some(&self.paths.config), arguments);
        if let Err(error) = result {
            let _ = self.close();
            return Err(error);
        }
        if let Err(error) = self
            .set_pane_role(NAVIGATOR_PANE, PresentationPaneRole::Navigator, None)
            .and_then(|()| self.set_pane_remain_on_exit(NAVIGATOR_PANE, true))
        {
            let _ = self.close();
            return Err(error);
        }
        let wait = self.provider_wait_command();
        let result = self.invoke(
            None,
            vec![
                "split-window".into(),
                "-h".into(),
                "-d".into(),
                "-t".into(),
                format!("{}:0.0", self.paths.session_name).into(),
                "-l".into(),
                PREFERRED_PROVIDER_PANE_WIDTH.to_string().into(),
                wait[0].clone(),
                wait[1].clone(),
                wait[2].clone(),
                wait[3].clone(),
            ],
        );
        if let Err(error) = result {
            let _ = self.close();
            return Err(error);
        }
        if let Err(error) = self
            .set_pane_role(PROVIDER_PANE, PresentationPaneRole::Provider, None)
            .and_then(|()| self.set_pane_remain_on_exit(PROVIDER_PANE, true))
            .and_then(|()| self.install_control_bindings())
        {
            let _ = self.close();
            return Err(error);
        }
        if let Err(error) = self
            .set_default_navigator_width()
            .and_then(|()| self.install_navigator_width_hooks())
        {
            let _ = self.close();
            return Err(error);
        }
        Ok(())
    }

    /// Directly attaches the caller's terminal to this private presentation.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux cannot attach to this exact private server.
    pub fn attach(&self) -> Result<(), PresentationError> {
        self.prepare_attach()?;
        let status = Command::new("tmux")
            .env_remove("TMUX")
            .arg("-S")
            .arg(&self.paths.socket)
            .args(["attach-session", "-t", &self.paths.session_name])
            .status()
            .map_err(PresentationError::Io)?;
        if status.success() {
            return Ok(());
        }
        if stopped_owned_presentation(self.is_live()?) {
            self.close()?;
            return Ok(());
        }
        Err(PresentationError::TmuxRejected(
            "presentation attach failed".to_owned(),
        ))
    }

    fn prepare_attach(&self) -> Result<(), PresentationError> {
        let (columns, rows) = crossterm::terminal::size()
            .map_err(|_| PresentationError::TerminalGeometryUnavailable)?;
        self.prepare_attach_with_size(columns, rows)
    }

    fn prepare_attach_with_size(&self, columns: u16, rows: u16) -> Result<(), PresentationError> {
        prepare_attach_window_with_size(&self.paths.session_name, columns, rows, |arguments| {
            self.invoke(None, arguments)
        })
    }

    /// Replaces only the outer provider attachment helper. The managed Codex
    /// runtime remains in its own private tmux server.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux rejects replacement of the exact owned pane.
    pub fn attach_workstream(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<AttachmentStatus, PresentationError> {
        self.with_attachment_claim(|| {
            let status = self.prepare_attachment("local", workstream_id)?;
            let result = (|| {
                self.retire_utility_for_attachment("local", workstream_id)?;
                let provider = self.provider_target()?;
                self.set_pane_role(
                    &provider,
                    PresentationPaneRole::Provider,
                    Some((&status.host_alias, status.workstream_id)),
                )?;
                self.invoke(
                    None,
                    self.provider_respawn_arguments(&provider, workstream_id, status.attempt_id),
                )
            })();
            self.finish_attachment_start(status, result)
        })
    }

    /// Replaces only the outer provider attachment helper with an interactive
    /// SSH attachment command. The remote native Runtime remains owned by its
    /// remote private tmux server; this local presentation owns no remote
    /// process or provider output.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux rejects replacement of the exact owned pane.
    pub fn attach_remote_workstream(
        &self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> Result<AttachmentStatus, PresentationError> {
        self.with_attachment_claim(|| {
            let status = self.prepare_attachment(host_alias, workstream_id)?;
            let result = (|| {
                self.retire_utility_for_attachment(host_alias, workstream_id)?;
                let provider = self.provider_target()?;
                self.set_pane_role(
                    &provider,
                    PresentationPaneRole::Provider,
                    Some((&status.host_alias, status.workstream_id)),
                )?;
                self.invoke(
                    None,
                    self.provider_remote_respawn_arguments(
                        &provider,
                        host_alias,
                        workstream_id,
                        status.attempt_id,
                    ),
                )
            })();
            self.finish_attachment_start(status, result)
        })
    }

    /// Retires the exact utility pane before a different Workstream can
    /// replace the provider attachment. A shell tagged for the requested
    /// host/Workstream is retained so same-Workstream reconnects preserve its
    /// launch context.
    ///
    /// This deliberately performs no provider mutation. The topology is
    /// validated before the exact utility pane is killed and again after the
    /// kill, so an ambiguous or unconfirmed cleanup refuses the attachment
    /// before its provider pane is retagged or respawned.
    ///
    /// # Errors
    ///
    /// Returns an error when the owned presentation topology is ambiguous, an
    /// exact utility cleanup is rejected, or the resulting two-pane geometry
    /// cannot be proven.
    fn retire_utility_for_attachment(
        &self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> Result<(), PresentationError> {
        validate_host_alias(host_alias)?;
        self.validate_single_presentation_window()?;
        let topology = self.read_topology()?;
        let Some(utility) = topology.utility() else {
            return Ok(());
        };
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        let provider_matches = (provider.host_alias.is_none() && provider.workstream_id.is_none())
            || (provider.host_alias.as_deref() == Some(host_alias)
                && provider.workstream_id == Some(workstream_id));
        let utility_matches = utility.host_alias.as_deref() == Some(host_alias)
            && utility.workstream_id == Some(workstream_id);
        if provider_matches && utility_matches {
            return Ok(());
        }

        let utility_id = utility.id.clone();
        self.kill_exact_pane(&utility_id)?;
        self.validate_single_presentation_window()?;
        let topology = self.read_topology()?;
        if topology.utility().is_some() {
            return Err(PresentationError::ControlRefused(
                "utility shell cleanup could not be proven",
            ));
        }
        Ok(())
    }

    /// Replaces the blank provider pane with the local temporary native Codex
    /// observer-review surface. This is not a Workstream attachment and never
    /// records provider output in presentation state.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned presentation pane cannot be
    /// replaced.
    pub fn start_observer_review(&self) -> Result<(), PresentationError> {
        let provider = self.provider_target()?;
        self.clear_pane_context(&provider)?;
        self.invoke(
            None,
            self.provider_respawn_for_command(&provider, self.observer_review_command()),
        )
    }

    /// Replaces the provider pane with the native observer-review surface on
    /// one registered remote host. The local presentation still owns only its
    /// own pane and never stores or writes provider terminal bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned presentation pane cannot be
    /// replaced.
    pub fn start_remote_observer_review(&self, host_alias: &str) -> Result<(), PresentationError> {
        let provider = self.provider_target()?;
        self.clear_pane_context(&provider)?;
        self.invoke(
            None,
            self.provider_respawn_for_command(
                &provider,
                self.remote_observer_review_command(host_alias),
            ),
        )
    }

    fn provider_respawn_arguments(
        &self,
        provider: &str,
        workstream_id: WorkstreamId,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        let command = self.provider_attach_command(workstream_id, attempt_id);
        self.provider_respawn_for_command(provider, command)
    }

    fn provider_remote_respawn_arguments(
        &self,
        provider: &str,
        host_alias: &str,
        workstream_id: WorkstreamId,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        let command = vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_remote_attach".into(),
            host_alias.into(),
            workstream_id.to_string().into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--attempt-id".into(),
            attempt_id.to_string().into(),
        ];
        self.provider_respawn_for_command(provider, command)
    }

    fn provider_respawn_for_command(
        &self,
        provider: &str,
        command: Vec<OsString>,
    ) -> Vec<OsString> {
        let mut arguments = vec![
            "respawn-pane".into(),
            "-k".into(),
            "-t".into(),
            self.pane_target(provider).into(),
        ];
        arguments.extend(command);
        arguments
    }

    /// Gives keyboard focus to the directly interactive provider pane.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned pane cannot be focused.
    pub fn focus_provider(&self) -> Result<(), PresentationError> {
        let provider = self.provider_target()?;
        self.select_owned_pane(&provider)
    }

    /// Gives keyboard focus to the navigator pane without touching a provider
    /// Runtime or its attachment helper.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned pane cannot be focused.
    pub fn focus_navigator(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.select_owned_pane(&navigator)
    }

    /// Returns the exact owned role for a pane supplied by tmux's format
    /// expansion. No positional pane index is accepted at this boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the private pane topology is missing, dead, or
    /// ambiguous, or when the source pane is not an exact owned pane.
    pub fn focused_pane_role(
        &self,
        source_pane: &str,
    ) -> Result<PresentationPaneRole, PresentationError> {
        let topology = self.read_topology()?;
        topology
            .pane(source_pane)
            .map(|pane| pane.role)
            .ok_or(PresentationError::InvalidTopology)
    }

    /// Validates that the provider role still names the exact local
    /// attachment represented by the ephemeral status row. This is called
    /// before any shell split or provider literal input.
    ///
    /// # Errors
    ///
    /// Returns an error when the private topology is ambiguous or the tagged
    /// provider context does not exactly match the supplied attachment.
    pub fn validate_provider_context(
        &self,
        workstream_id: WorkstreamId,
        host_alias: &str,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        if provider.host_alias.as_deref() != Some(host_alias)
            || provider.workstream_id != Some(workstream_id)
        {
            return Err(PresentationError::InvalidTopology);
        }
        Ok(())
    }

    /// Focuses the exact utility pane if one is already present. This check
    /// intentionally precedes attachment preflight so a shell keeps its
    /// original launch context when the provider selection later changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the source or topology is ambiguous, or when an
    /// existing utility pane cannot be focused.
    pub fn focus_existing_utility_if_present(
        &self,
        source_pane: &str,
    ) -> Result<bool, PresentationError> {
        let topology = match self.read_topology() {
            Ok(topology) => topology,
            Err(PresentationError::InvalidTopology) if self.shell_claim_present()? => {
                // A competing helper may have the one bounded claim while its
                // new pane is between split and role tagging. Let the caller
                // perform authoritative preflight and enter the same bounded
                // create/focus retry loop instead of treating that transient
                // evidence as a foreign topology.
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        let Some(utility) = topology.utility() else {
            return Ok(false);
        };
        self.select_owned_pane(&utility.id)?;
        Ok(true)
    }

    /// Arms one exact newly-created utility pane before its shell barrier
    /// replaces itself. The pane must already belong to this private
    /// presentation window; no positional pane target is accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when the pane identity is malformed, belongs to a
    /// different session/window, or cannot be switched to non-retaining mode.
    pub fn prepare_utility_pane(&self, pane: &str) -> Result<(), PresentationError> {
        let pane = parse_pane_id(pane).ok_or(PresentationError::InvalidTopology)?;
        let evidence = self.invoke_capture(
            None,
            vec![
                "display-message".into(),
                "-p".into(),
                "-t".into(),
                pane.clone().into(),
                "#{session_name}\t#{window_name}\t#{pane_id}".into(),
            ],
        )?;
        let expected = format!("{}\t{}\t{pane}", self.paths.session_name, NAVIGATOR_WINDOW);
        if evidence.trim() != expected {
            return Err(PresentationError::InvalidTopology);
        }
        self.set_pane_remain_on_exit(&pane, false)
    }

    /// Creates one local shell below the exact provider, or focuses the
    /// existing utility shell. The caller must complete authoritative state
    /// preflight before invoking this method.
    ///
    /// # Errors
    ///
    /// Returns an error when the shell path, project root, role topology, or
    /// bounded tmux mutation is not exact. A shell that exits before tagging
    /// is treated as normal cleanup.
    pub fn create_or_focus_shell(
        &self,
        source_pane: &str,
        host_alias: &str,
        workstream_id: WorkstreamId,
        cwd: &Path,
        shell: &Path,
    ) -> Result<(), PresentationError> {
        validate_host_alias(host_alias)?;
        if host_alias != "local" {
            return Err(PresentationError::ControlRefused(
                "remote presentation shell requires an SSH endpoint",
            ));
        }
        validate_shell_path(shell)?;
        if !cwd.is_dir() {
            return Err(PresentationError::ControlRefused(
                "registered project root is unavailable",
            ));
        }

        let shell_command = vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_presentation_shell".into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--shell".into(),
            shell.to_path_buf().into_os_string(),
            "--cwd".into(),
            cwd.to_path_buf().into_os_string(),
        ];
        self.create_or_focus_shell_command(source_pane, host_alias, workstream_id, &shell_command)
    }

    /// Creates one remote utility shell below the exact provider, or focuses
    /// an existing utility. The local pane receives only the fixed SSH
    /// endpoint values and opaque Workstream ID; the remote host resolves its
    /// own authoritative project root and account shell.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint, role topology, or bounded tmux
    /// mutation is not exact.
    pub fn create_or_focus_remote_shell(
        &self,
        source_pane: &str,
        host_alias: &str,
        workstream_id: WorkstreamId,
        destination: &str,
        executable: &str,
    ) -> Result<(), PresentationError> {
        validate_host_alias(host_alias)?;
        if host_alias == "local" {
            return Err(PresentationError::ControlRefused(
                "local presentation shell requires local preflight",
            ));
        }
        crate::transport::SshDestination::parse(destination)
            .map_err(|_| PresentationError::ControlRefused("remote SSH endpoint is invalid"))?;
        crate::transport::RemoteExecutable::parse(executable)
            .map_err(|_| PresentationError::ControlRefused("remote SSH endpoint is invalid"))?;
        let shell_command = self.remote_shell_command(destination, executable, workstream_id);
        self.create_or_focus_shell_command(source_pane, host_alias, workstream_id, &shell_command)
    }

    fn remote_shell_command(
        &self,
        destination: &str,
        executable: &str,
        workstream_id: WorkstreamId,
    ) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_presentation_ssh_shell".into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--destination".into(),
            destination.into(),
            "--executable".into(),
            executable.into(),
            "--workstream-id".into(),
            workstream_id.to_string().into(),
        ]
    }

    fn create_or_focus_shell_command(
        &self,
        source_pane: &str,
        host_alias: &str,
        workstream_id: WorkstreamId,
        shell_command: &[OsString],
    ) -> Result<(), PresentationError> {
        for _ in 0..SHELL_CLAIM_ATTEMPTS {
            let topology = match self.read_topology() {
                Ok(topology) => topology,
                Err(PresentationError::InvalidTopology) if self.shell_claim_present()? => {
                    thread::sleep(SHELL_CLAIM_RETRY);
                    continue;
                }
                Err(error) => return Err(error),
            };
            topology
                .pane(source_pane)
                .ok_or(PresentationError::InvalidTopology)?;
            if let Some(utility) = topology.utility() {
                self.select_owned_pane(&utility.id)?;
                return Ok(());
            }
            let provider = topology
                .provider()
                .ok_or(PresentationError::InvalidTopology)?;
            if provider.host_alias.as_deref() != Some(host_alias)
                || provider.workstream_id != Some(workstream_id)
            {
                return Err(PresentationError::InvalidTopology);
            }

            let token = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
            if !self.try_shell_claim(&token)? {
                thread::sleep(SHELL_CLAIM_RETRY);
                continue;
            }
            let result = self.create_shell_after_claim(
                &topology,
                host_alias,
                workstream_id,
                provider.id.as_str(),
                shell_command,
            );
            self.release_shell_claim(&token);
            return result;
        }
        Err(PresentationError::ControlRefused(
            "another shell action is in progress",
        ))
    }

    fn create_shell_after_claim(
        &self,
        topology: &PresentationTopology,
        host_alias: &str,
        workstream_id: WorkstreamId,
        provider: &str,
        shell_command: &[OsString],
    ) -> Result<(), PresentationError> {
        let mut split_arguments = vec![
            "split-window".into(),
            "-v".into(),
            "-P".into(),
            "-F".into(),
            "#{pane_id}".into(),
            "-t".into(),
            provider.into(),
        ];
        split_arguments.extend(shell_command.iter().cloned());
        let output = self.invoke_capture(None, split_arguments)?;
        let Some(utility_id) = parse_pane_id(output.trim()) else {
            return Err(PresentationError::InvalidTopology);
        };
        if topology.pane(&utility_id).is_some() {
            return Err(PresentationError::InvalidTopology);
        }
        let setup = (|| {
            self.set_pane_remain_on_exit(&utility_id, false)?;
            self.set_pane_role(
                &utility_id,
                PresentationPaneRole::Utility,
                Some((host_alias, workstream_id)),
            )?;
            self.select_owned_pane(&utility_id)?;
            if self.pane_is_dead(&utility_id)? {
                self.kill_exact_pane(&utility_id)?;
            }
            Ok(())
        })();
        match setup {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self.kill_exact_pane(&utility_id);
                if pane_disappeared(&error) {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Runs a bounded presentation-only action. Provider literal input is
    /// deliberately excluded: the app layer must first preflight the exact
    /// Runtime and use its private tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the action's source pane or owned role topology
    /// is ambiguous, or when the exact private tmux action is rejected.
    pub fn control(
        &self,
        action: PresentationAction,
        source_pane: &str,
    ) -> Result<(), PresentationError> {
        self.control_with_client(action, source_pane, None)
    }

    /// Runs one presentation action with the exact invoking tmux client when
    /// the action needs a client-scoped prompt.  The client identity is
    /// intentionally optional for callers that cannot originate a tmux key
    /// binding (for example, deterministic unit fixtures); utility close
    /// refuses that path instead of guessing a client.
    ///
    /// # Errors
    ///
    /// Returns an error when the action's source pane, client, or owned role
    /// topology is ambiguous, or when the exact private tmux action is
    /// rejected.
    pub fn control_with_client(
        &self,
        action: PresentationAction,
        source_pane: &str,
        client_name: Option<&str>,
    ) -> Result<(), PresentationError> {
        match action {
            PresentationAction::SuppressSplit => {
                self.focused_pane_role(source_pane)?;
                self.show_guidance("Use Ctrl+b \" for the utility shell")
            }
            PresentationAction::CloseShell => self.close_shell(source_pane, client_name),
            PresentationAction::FocusUp
            | PresentationAction::FocusDown
            | PresentationAction::FocusLeft
            | PresentationAction::FocusRight
            | PresentationAction::FocusNext => self.focus_direction(source_pane, action),
            PresentationAction::LiteralCtrlB => {
                let role = self.focused_pane_role(source_pane)?;
                if role == PresentationPaneRole::Provider {
                    return Err(PresentationError::ControlRefused(
                        "provider literal input requires Runtime preflight",
                    ));
                }
                self.send_outer_literal_c_b(source_pane)
            }
            PresentationAction::CreateOrFocusShell => Err(PresentationError::ControlRefused(
                "local shell requires attachment preflight",
            )),
        }
    }

    /// Sends one literal C-b through the outer presentation pane. Provider
    /// panes are rejected here so they cannot accidentally invoke the nested
    /// Runtime prefix table.
    ///
    /// # Errors
    ///
    /// Returns an error when the source pane is not an exact owned non-provider
    /// pane or the private tmux server rejects the literal input.
    pub fn send_outer_literal_c_b(&self, source_pane: &str) -> Result<(), PresentationError> {
        let role = self.focused_pane_role(source_pane)?;
        if role == PresentationPaneRole::Provider {
            return Err(PresentationError::ControlRefused(
                "provider literal input requires Runtime preflight",
            ));
        }
        self.invoke(
            None,
            vec![
                "send-keys".into(),
                "-t".into(),
                source_pane.into(),
                "C-b".into(),
            ],
        )
    }

    fn close_shell(
        &self,
        source_pane: &str,
        client_name: Option<&str>,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let source = topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        if source.role != PresentationPaneRole::Utility {
            return self.show_guidance("Ctrl+b x closes only the utility shell");
        }
        let client_name = client_name.ok_or(PresentationError::ControlRefused(
            "invoking presentation client is unavailable",
        ))?;
        self.validate_presentation_client(client_name)?;
        self.invoke(None, close_shell_arguments(client_name, &source.id))
    }

    fn validate_presentation_client(&self, client_name: &str) -> Result<(), PresentationError> {
        if client_name.is_empty()
            || client_name.len() > 256
            || client_name
                .chars()
                .any(|character| character.is_control() || character == '\t')
        {
            return Err(PresentationError::ControlRefused(
                "invoking presentation client is invalid",
            ));
        }
        let clients = self.invoke_capture(
            None,
            vec![
                "list-clients".into(),
                "-F".into(),
                "#{client_name}\t#{session_name}\t#{window_name}".into(),
            ],
        )?;
        if clients.lines().any(|line| {
            let mut fields = line.split('\t');
            fields.next() == Some(client_name)
                && fields.next() == Some(self.paths.session_name.as_str())
                && fields.next() == Some(NAVIGATOR_WINDOW)
                && fields.next().is_none()
        }) {
            Ok(())
        } else {
            Err(PresentationError::ControlRefused(
                "invoking client is not attached to this presentation",
            ))
        }
    }

    fn focus_direction(
        &self,
        source_pane: &str,
        action: PresentationAction,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let source = topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        let target = match action {
            PresentationAction::FocusNext => topology.next(source),
            PresentationAction::FocusUp => topology.directional(source, Direction::Up),
            PresentationAction::FocusDown => topology.directional(source, Direction::Down),
            PresentationAction::FocusLeft => topology.directional(source, Direction::Left),
            PresentationAction::FocusRight => topology.directional(source, Direction::Right),
            _ => None,
        };
        let Some(target) = target else {
            return self.show_guidance("No other owned pane in that direction");
        };
        self.select_owned_pane(&target.id)
    }

    fn select_owned_pane(&self, pane: &str) -> Result<(), PresentationError> {
        self.invoke(None, vec!["select-pane".into(), "-t".into(), pane.into()])
    }

    fn kill_exact_pane(&self, pane: &str) -> Result<(), PresentationError> {
        if parse_pane_id(pane).is_none() {
            return Err(PresentationError::InvalidTopology);
        }
        match self.invoke(None, vec!["kill-pane".into(), "-t".into(), pane.into()]) {
            Ok(()) => Ok(()),
            Err(error) if pane_disappeared(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Displays one bounded guidance message in the Navigator pane.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private presentation server rejects
    /// the bounded message action.
    pub fn show_guidance(&self, message: &str) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.invoke(
            None,
            vec![
                "display-message".into(),
                "-t".into(),
                navigator.into(),
                "-d".into(),
                "3000".into(),
                message.into(),
            ],
        )
    }

    fn read_topology(&self) -> Result<PresentationTopology, PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-panes".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_WINDOW}", self.paths.session_name).into(),
                "-F".into(),
                TOPOLOGY_FORMAT.into(),
            ],
        )?;
        parse_topology(&output)
    }

    fn validate_single_presentation_window(&self) -> Result<(), PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-windows".into(),
                "-t".into(),
                self.paths.session_name.clone().into(),
                "-F".into(),
                "#{window_name}\t#{window_id}".into(),
            ],
        )?;
        let mut windows = output.lines();
        let Some(window) = windows.next() else {
            return Err(PresentationError::InvalidTopology);
        };
        let mut fields = window.split('\t');
        if fields.next() != Some(NAVIGATOR_WINDOW)
            || !fields.next().is_some_and(parse_window_id)
            || fields.next().is_some()
            || windows.next().is_some()
        {
            return Err(PresentationError::InvalidTopology);
        }
        Ok(())
    }

    fn read_topology_allow_dead(&self) -> Result<PresentationTopology, PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-panes".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_WINDOW}", self.paths.session_name).into(),
                "-F".into(),
                TOPOLOGY_FORMAT.into(),
            ],
        )?;
        parse_topology_with_dead(&output, true)
    }

    fn set_pane_remain_on_exit(&self, pane: &str, enabled: bool) -> Result<(), PresentationError> {
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-t".into(),
                self.pane_target(pane).into(),
                "remain-on-exit".into(),
                if enabled { "on" } else { "off" }.into(),
            ],
        )
    }

    fn pane_is_dead(&self, pane: &str) -> Result<bool, PresentationError> {
        let value = self.invoke_capture(
            None,
            vec![
                "display-message".into(),
                "-p".into(),
                "-t".into(),
                self.pane_target(pane).into(),
                "#{pane_dead}".into(),
            ],
        )?;
        match value.trim() {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(PresentationError::InvalidTopology),
        }
    }

    fn try_shell_claim(&self, token: &str) -> Result<bool, PresentationError> {
        match self.invoke(
            None,
            vec![
                "set-option".into(),
                "-g".into(),
                "-o".into(),
                SHELL_CLAIM_OPTION.into(),
                token.into(),
            ],
        ) {
            Ok(()) => Ok(true),
            Err(PresentationError::TmuxRejected(message)) if message.contains("already set") => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn shell_claim_present(&self) -> Result<bool, PresentationError> {
        let value = self.invoke_capture(
            None,
            vec![
                "show-options".into(),
                "-gqv".into(),
                SHELL_CLAIM_OPTION.into(),
            ],
        )?;
        Ok(!value.trim().is_empty())
    }

    fn release_shell_claim(&self, token: &str) {
        let current = self.invoke_capture(
            None,
            vec![
                "show-options".into(),
                "-gqv".into(),
                SHELL_CLAIM_OPTION.into(),
            ],
        );
        if current
            .ok()
            .as_deref()
            .is_some_and(|value| value.trim() == token)
        {
            let _ = self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-g".into(),
                    "-u".into(),
                    SHELL_CLAIM_OPTION.into(),
                ],
            );
        }
    }

    fn with_attachment_claim<T>(
        &self,
        operation: impl FnOnce() -> Result<T, PresentationError>,
    ) -> Result<T, PresentationError> {
        // The shell claim is presentation-global, so holding it through the
        // provider retag/respawn closes the race where a new utility split
        // could appear after retirement but before attachment replacement.
        let token = format!(
            "attachment-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        if !self.try_shell_claim(&token)? {
            return Err(PresentationError::ControlRefused(
                "another presentation shell or attachment action is in progress",
            ));
        }
        let result = operation();
        self.release_shell_claim(&token);
        result
    }

    fn set_pane_role(
        &self,
        pane: &str,
        role: PresentationPaneRole,
        context: Option<(&str, WorkstreamId)>,
    ) -> Result<(), PresentationError> {
        let role_name = match role {
            PresentationPaneRole::Navigator => "navigator",
            PresentationPaneRole::Provider => "provider",
            PresentationPaneRole::Utility => "utility",
        };
        let target = self.pane_target(pane);
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-t".into(),
                target.clone().into(),
                ROLE_OPTION.into(),
                role_name.into(),
            ],
        )?;
        self.clear_pane_context(pane)?;
        if let Some((host_alias, workstream_id)) = context {
            validate_host_alias(host_alias)?;
            self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-p".into(),
                    "-t".into(),
                    target.clone().into(),
                    HOST_OPTION.into(),
                    host_alias.into(),
                ],
            )?;
            self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-p".into(),
                    "-t".into(),
                    target.into(),
                    WORKSTREAM_OPTION.into(),
                    workstream_id.to_string().into(),
                ],
            )?;
        }
        Ok(())
    }

    fn clear_pane_context(&self, pane: &str) -> Result<(), PresentationError> {
        let target = self.pane_target(pane);
        for option in [HOST_OPTION, WORKSTREAM_OPTION] {
            self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-p".into(),
                    "-u".into(),
                    "-t".into(),
                    target.clone().into(),
                    option.into(),
                ],
            )?;
        }
        Ok(())
    }

    fn pane_target(&self, pane: &str) -> String {
        if pane.starts_with('%') {
            pane.to_owned()
        } else {
            format!("{}:{pane}", self.paths.session_name)
        }
    }

    fn navigator_target(&self) -> Result<String, PresentationError> {
        self.read_topology()?
            .navigator()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    fn provider_target(&self) -> Result<String, PresentationError> {
        self.read_topology()?
            .provider()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    fn install_control_bindings(&self) -> Result<(), PresentationError> {
        let bindings = [
            ("\"", PresentationAction::CreateOrFocusShell),
            ("%", PresentationAction::SuppressSplit),
            ("x", PresentationAction::CloseShell),
            ("o", PresentationAction::FocusNext),
            ("Up", PresentationAction::FocusUp),
            ("Down", PresentationAction::FocusDown),
            ("Left", PresentationAction::FocusLeft),
            ("Right", PresentationAction::FocusRight),
            ("C-b", PresentationAction::LiteralCtrlB),
        ];
        for (key, action) in bindings {
            // Deliberately omit `-b`: tmux waits for this fixed helper before
            // accepting another key action, which makes create/focus requests
            // serialize without a lock that could outlive a failed helper.
            self.invoke(
                None,
                vec![
                    "bind-key".into(),
                    "-T".into(),
                    "prefix".into(),
                    key.into(),
                    "run-shell".into(),
                    self.control_shell_command(action)?.into(),
                ],
            )?;
        }
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                "prefix".into(),
                "d".into(),
                "detach-client".into(),
            ],
        )?;
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                "prefix".into(),
                "?".into(),
                "display-message".into(),
                "Ctrl+b: \" shell | % blocked | x close shell | o/directions focus | d detach | Ctrl+b literal | ? help".into(),
            ],
        )
    }

    fn control_shell_command(
        &self,
        action: PresentationAction,
    ) -> Result<String, PresentationError> {
        let executable = shell_quote(self.executable.as_os_str())?;
        let state_root = shell_quote(self.state_root.as_os_str())?;
        let socket = shell_quote(self.paths.socket.as_os_str())?;
        let session = shell_quote(self.paths.session_name.as_ref())?;
        Ok(format!(
            "exec {executable} --state-root {state_root} _presentation_control --presentation-socket {socket} --presentation-session {session} --action {} --source-pane '#{{pane_id}}' --client-name #{{q:client_name}}",
            action.as_str()
        ))
    }

    /// Returns the current exact provider attachment attempt. Before its helper
    /// reports `Running`, a dead pane is atomically converted to `Failed` for
    /// an exact same-row retry. Once running, the helper itself reports its
    /// terminal phase, so this method deliberately avoids repeated control
    /// queries against the presentation tmux server.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed private status or ambiguous tmux pane
    /// evidence.
    pub fn attachment_status(&self) -> Result<Option<AttachmentStatus>, PresentationError> {
        let Some(mut status) = self.read_attachment_status()? else {
            return Ok(None);
        };
        if status.phase == AttachmentPhase::Pending && self.provider_pane_is_dead()? {
            status.phase = AttachmentPhase::Failed;
            self.write_attachment_status(&status)?;
        }
        Ok(Some(status))
    }

    /// Advances only the currently recorded exact attachment attempt.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale attempt, invalid transition, or private
    /// status I/O failure.
    pub fn report_attachment_phase(
        &self,
        attempt_id: uuid::Uuid,
        phase: AttachmentPhase,
    ) -> Result<(), PresentationError> {
        let Some(mut status) = self.read_attachment_status()? else {
            return Err(PresentationError::StaleAttachmentAttempt);
        };
        if status.attempt_id != attempt_id
            || !matches!(
                (status.phase, phase),
                (
                    AttachmentPhase::Pending,
                    AttachmentPhase::Running | AttachmentPhase::Failed
                ) | (
                    AttachmentPhase::Running,
                    AttachmentPhase::Completed | AttachmentPhase::Failed
                )
            )
        {
            return Err(PresentationError::StaleAttachmentAttempt);
        }
        status.phase = phase;
        self.write_attachment_status(&status)
    }

    /// Stops only this private presentation server and removes its exact
    /// private directory. It never targets a provider runtime or default tmux.
    ///
    /// # Errors
    ///
    /// Returns an error when a live private presentation cannot be stopped or
    /// its owned directory cannot be removed.
    pub fn close(&self) -> Result<(), PresentationError> {
        let result = self.invoke(None, vec!["kill-server".into()]);
        if let Err(PresentationError::TmuxRejected(message)) = &result
            && !message.contains("no server running")
            && !message.contains("No such file")
        {
            return Err(PresentationError::TmuxRejected(message.clone()));
        }
        if self.paths.directory.exists() {
            fs::remove_dir_all(&self.paths.directory).map_err(PresentationError::Io)?;
        }
        Ok(())
    }

    fn discover_live(state_root: &Path) -> Result<Vec<Self>, PresentationError> {
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        if !presentation_root.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&presentation_root).map_err(PresentationError::Io)?;
        let mut live = Vec::new();
        for entry in entries {
            let entry = entry.map_err(PresentationError::Io)?;
            if !entry.file_type().map_err(PresentationError::Io)?.is_dir() {
                return Err(PresentationError::InvalidControlPath(entry.path()));
            }
            let directory = entry.path();
            let session_name = presentation_session_name(&directory)
                .ok_or_else(|| PresentationError::InvalidControlPath(directory.clone()))?;
            let presentation =
                Self::from_control(state_root, directory.join("tmux.sock"), session_name)?;
            let session_live = presentation.is_live()?;
            let navigator_pane_dead = session_live && presentation.navigator_pane_is_dead()?;
            if should_reuse_presentation(session_live, navigator_pane_dead) {
                live.push(presentation);
            } else {
                presentation.close()?;
            }
        }
        Ok(live)
    }

    fn is_live(&self) -> Result<bool, PresentationError> {
        let mut command = Command::new("tmux");
        command
            .env_remove("TMUX")
            .arg("-S")
            .arg(&self.paths.socket)
            .args(["has-session", "-t", &self.paths.session_name]);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            return Ok(true);
        }
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        if !self.paths.socket.exists()
            || diagnostic.contains("no server running")
            || diagnostic.contains("No such file")
        {
            return Ok(false);
        }
        Err(PresentationError::TmuxRejected(sanitize_diagnostic(
            &diagnostic,
        )))
    }

    fn navigator_command(&self) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_navigator".into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
        ]
    }

    fn provider_wait_command(&self) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_wait".into(),
        ]
    }

    fn observer_review_command(&self) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_observer_review".into(),
        ]
    }

    fn remote_observer_review_command(&self, host_alias: &str) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_remote_observer_review".into(),
            host_alias.into(),
        ]
    }

    fn provider_attach_command(
        &self,
        workstream_id: WorkstreamId,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_attach".into(),
            workstream_id.to_string().into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--attempt-id".into(),
            attempt_id.to_string().into(),
        ]
    }

    fn prepare_attachment(
        &self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> Result<AttachmentStatus, PresentationError> {
        validate_host_alias(host_alias)?;
        let status = AttachmentStatus {
            attempt_id: uuid::Uuid::new_v4(),
            host_alias: host_alias.to_owned(),
            workstream_id,
            phase: AttachmentPhase::Pending,
        };
        self.write_attachment_status(&status)?;
        Ok(status)
    }

    fn finish_attachment_start(
        &self,
        mut status: AttachmentStatus,
        result: Result<(), PresentationError>,
    ) -> Result<AttachmentStatus, PresentationError> {
        if let Err(error) = result {
            status.phase = AttachmentPhase::Failed;
            let _ = self.write_attachment_status(&status);
            return Err(error);
        }
        Ok(status)
    }

    fn read_attachment_status(&self) -> Result<Option<AttachmentStatus>, PresentationError> {
        let metadata = match fs::symlink_metadata(&self.paths.attachment_status) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PresentationError::Io(error)),
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_ATTACHMENT_STATUS_BYTES {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let file = fs::File::open(&self.paths.attachment_status).map_err(PresentationError::Io)?;
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENT_STATUS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(PresentationError::Io)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_STATUS_BYTES {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let status: AttachmentStatus = serde_json::from_slice(&bytes)
            .map_err(|_| PresentationError::InvalidAttachmentStatus)?;
        validate_host_alias(&status.host_alias)?;
        Ok(Some(status))
    }

    fn write_attachment_status(&self, status: &AttachmentStatus) -> Result<(), PresentationError> {
        validate_host_alias(&status.host_alias)?;
        let bytes =
            serde_json::to_vec(status).map_err(|_| PresentationError::InvalidAttachmentStatus)?;
        if bytes.len() > usize::try_from(MAX_ATTACHMENT_STATUS_BYTES).unwrap_or(usize::MAX) {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let temporary = self
            .paths
            .directory
            .join(format!(".attachment-{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(PresentationError::Io)?;
        file.write_all(&bytes).map_err(PresentationError::Io)?;
        file.sync_all().map_err(PresentationError::Io)?;
        set_mode(&temporary, 0o600)?;
        fs::rename(&temporary, &self.paths.attachment_status).map_err(PresentationError::Io)
    }

    fn provider_pane_is_dead(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        topology
            .provider()
            .map(|pane| pane.dead)
            .ok_or(PresentationError::InvalidTopology)
    }

    fn navigator_pane_is_dead(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        topology
            .navigator()
            .map(|pane| pane.dead)
            .ok_or(PresentationError::InvalidTopology)
    }

    #[cfg(test)]
    fn pane_dead_arguments(&self, pane: &str) -> Vec<OsString> {
        vec![
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            self.pane_target(pane).into(),
            "#{pane_dead}".into(),
        ]
    }

    /// Keeps the narrow navigator at its deliberate default width, leaving
    /// all remaining terminal columns to the native provider pane.
    /// Reapplies the compact navigator layout after tmux adopts a controlling
    /// client's terminal size.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private tmux server rejects the resize.
    pub fn set_default_navigator_width(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.invoke(
            None,
            self.default_navigator_resize_arguments_for(&navigator),
        )
    }

    fn default_navigator_resize_arguments_for(&self, navigator: &str) -> Vec<OsString> {
        vec![
            "resize-pane".into(),
            "-t".into(),
            self.pane_target(navigator).into(),
            "-x".into(),
            DEFAULT_NAVIGATOR_PANE_WIDTH.to_string().into(),
        ]
    }

    /// Keeps the compact split invariant at the private tmux event boundary.
    /// A detached server starts at its configured default size; when the first
    /// real client attaches, tmux otherwise expands both panes proportionally
    /// before the Navigator can receive a terminal resize event.
    fn install_navigator_width_hooks(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        for hook in NAVIGATOR_WIDTH_HOOKS {
            self.invoke(
                None,
                self.navigator_width_hook_arguments_for(hook, &navigator),
            )?;
        }
        Ok(())
    }

    fn navigator_width_hook_arguments_for(&self, hook: &str, navigator: &str) -> Vec<OsString> {
        vec![
            "set-hook".into(),
            "-t".into(),
            self.paths.session_name.clone().into(),
            hook.into(),
            format!(
                "resize-pane -t {} -x {DEFAULT_NAVIGATOR_PANE_WIDTH}",
                self.pane_target(navigator)
            )
            .into(),
        ]
    }

    fn invoke(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<(), PresentationError> {
        self.invoke_capture(config, arguments).map(|_| ())
    }

    fn invoke_capture(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<String, PresentationError> {
        let mut command = Command::new("tmux");
        command.env_remove("TMUX");
        if let Some(config) = config {
            command.arg("-f").arg(config);
        }
        command.arg("-S").arg(&self.paths.socket).args(arguments);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|_| {
                PresentationError::TmuxRejected(
                    "private presentation tmux output was not UTF-8".to_owned(),
                )
            })
        } else {
            Err(PresentationError::TmuxRejected(sanitize_diagnostic(
                &String::from_utf8_lossy(&output.stderr),
            )))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OwnedPane {
    id: String,
    role: PresentationPaneRole,
    host_alias: Option<String>,
    workstream_id: Option<WorkstreamId>,
    dead: bool,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresentationTopology {
    panes: Vec<OwnedPane>,
    window_width: u16,
    window_height: u16,
}

impl PresentationTopology {
    fn pane(&self, id: &str) -> Option<&OwnedPane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    fn navigator(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Navigator)
    }

    fn provider(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Provider)
    }

    fn utility(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Utility)
    }

    fn next(&self, source: &OwnedPane) -> Option<&OwnedPane> {
        let mut panes: Vec<&OwnedPane> = self.panes.iter().collect();
        panes.sort_by_key(|pane| (pane.top, pane.left, pane.id.as_str()));
        let index = panes.iter().position(|pane| pane.id == source.id)?;
        panes.get((index + 1) % panes.len()).copied()
    }

    fn directional(&self, source: &OwnedPane, direction: Direction) -> Option<&OwnedPane> {
        let source_x = i32::from(source.left) + i32::from(source.width) / 2;
        let source_y = i32::from(source.top) + i32::from(source.height) / 2;
        let mut candidates: Vec<(&OwnedPane, (i32, i32))> = self
            .panes
            .iter()
            .filter(|pane| pane.id != source.id)
            .filter_map(|pane| {
                let pane_x = i32::from(pane.left) + i32::from(pane.width) / 2;
                let pane_y = i32::from(pane.top) + i32::from(pane.height) / 2;
                let (primary, secondary) = match direction {
                    Direction::Up if pane_y < source_y => {
                        (source_y - pane_y, (source_x - pane_x).abs())
                    }
                    Direction::Down if pane_y > source_y => {
                        (pane_y - source_y, (source_x - pane_x).abs())
                    }
                    Direction::Left if pane_x < source_x => {
                        (source_x - pane_x, (source_y - pane_y).abs())
                    }
                    Direction::Right if pane_x > source_x => {
                        (pane_x - source_x, (source_y - pane_y).abs())
                    }
                    _ => return None,
                };
                Some((pane, (primary, secondary)))
            })
            .collect();
        candidates.sort_by_key(|(pane, distance)| (*distance, pane.id.as_str()));
        candidates.first().map(|(pane, _)| *pane)
    }
}

fn parse_topology(output: &str) -> Result<PresentationTopology, PresentationError> {
    parse_topology_with_dead(output, false)
}

fn parse_topology_with_dead(
    output: &str,
    allow_dead: bool,
) -> Result<PresentationTopology, PresentationError> {
    let mut panes = Vec::new();
    let mut window_size = None;
    for line in output.lines() {
        panes.push(parse_topology_line(
            line,
            allow_dead,
            &mut window_size,
            &panes,
        )?);
    }
    if !(2..=3).contains(&panes.len()) {
        return Err(PresentationError::InvalidTopology);
    }
    let (window_width, window_height) = window_size.ok_or(PresentationError::InvalidTopology)?;
    let topology = PresentationTopology {
        panes,
        window_width,
        window_height,
    };
    validate_topology_shape(&topology)?;
    Ok(topology)
}

fn parse_topology_line(
    line: &str,
    allow_dead: bool,
    window_size: &mut Option<(u16, u16)>,
    panes: &[OwnedPane],
) -> Result<OwnedPane, PresentationError> {
    if line.is_empty() {
        return Err(PresentationError::InvalidTopology);
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 11 {
        return Err(PresentationError::InvalidTopology);
    }
    let id = parse_pane_id(fields[0]).ok_or(PresentationError::InvalidTopology)?;
    if panes.iter().any(|pane| pane.id == id) {
        return Err(PresentationError::InvalidTopology);
    }
    let role = match fields[1] {
        "navigator" => PresentationPaneRole::Navigator,
        "provider" => PresentationPaneRole::Provider,
        "utility" => PresentationPaneRole::Utility,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let dead = match fields[4] {
        "0" => false,
        "1" if allow_dead => true,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let host_alias = if fields[2].is_empty() {
        None
    } else {
        validate_host_alias(fields[2])?;
        Some(fields[2].to_owned())
    };
    let workstream_id = if fields[3].is_empty() {
        None
    } else {
        Some(
            fields[3]
                .parse()
                .map_err(|_| PresentationError::InvalidTopology)?,
        )
    };
    if (role == PresentationPaneRole::Navigator
        && (host_alias.is_some() || workstream_id.is_some()))
        || (role == PresentationPaneRole::Utility
            && (host_alias.is_none() || workstream_id.is_none()))
        || (role == PresentationPaneRole::Provider
            && host_alias.is_some() != workstream_id.is_some())
    {
        return Err(PresentationError::InvalidTopology);
    }
    let window_width = topology_dimension(fields[9])?;
    let window_height = topology_dimension(fields[10])?;
    if window_width == 0 || window_height == 0 {
        return Err(PresentationError::InvalidTopology);
    }
    if let Some((expected_width, expected_height)) = window_size {
        if (*expected_width, *expected_height) != (window_width, window_height) {
            return Err(PresentationError::InvalidTopology);
        }
    } else {
        *window_size = Some((window_width, window_height));
    }
    let left = topology_dimension(fields[5])?;
    let top = topology_dimension(fields[6])?;
    let width = topology_dimension(fields[7])?;
    let height = topology_dimension(fields[8])?;
    if width == 0
        || height == 0
        || u32::from(left) + u32::from(width) > u32::from(window_width)
        || u32::from(top) + u32::from(height) > u32::from(window_height)
    {
        return Err(PresentationError::InvalidTopology);
    }
    Ok(OwnedPane {
        id,
        role,
        host_alias,
        workstream_id,
        dead,
        left,
        top,
        width,
        height,
    })
}

fn topology_dimension(value: &str) -> Result<u16, PresentationError> {
    value
        .parse::<u16>()
        .map_err(|_| PresentationError::InvalidTopology)
}

fn prepare_attach_window_with_size<F>(
    session_name: &str,
    columns: u16,
    rows: u16,
    mut invoke: F,
) -> Result<(), PresentationError>
where
    F: FnMut(Vec<OsString>) -> Result<(), PresentationError>,
{
    if columns == 0 || rows == 0 {
        return Err(PresentationError::InvalidTerminalGeometry);
    }
    let target = format!("{session_name}:{NAVIGATOR_WINDOW}");
    invoke(vec![
        "resize-window".into(),
        "-t".into(),
        target.clone().into(),
        "-x".into(),
        columns.to_string().into(),
        "-y".into(),
        rows.to_string().into(),
    ])?;
    invoke(vec![
        "set-window-option".into(),
        "-t".into(),
        target.into(),
        "window-size".into(),
        "latest".into(),
    ])
}

fn validate_topology_shape(topology: &PresentationTopology) -> Result<(), PresentationError> {
    if topology.navigator().is_none()
        || topology.provider().is_none()
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Navigator)
            .count()
            != 1
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Provider)
            .count()
            != 1
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Utility)
            .count()
            > 1
    {
        return Err(PresentationError::InvalidTopology);
    }
    let navigator = topology
        .navigator()
        .ok_or(PresentationError::InvalidTopology)?;
    let provider = topology
        .provider()
        .ok_or(PresentationError::InvalidTopology)?;
    if navigator.left != 0
        || navigator.top != 0
        || navigator.height != topology.window_height
        || provider.top != 0
        || provider.left
            != navigator
                .left
                .saturating_add(navigator.width)
                .saturating_add(1)
        || provider.left <= navigator.left
        || u32::from(provider.left) + u32::from(provider.width) != u32::from(topology.window_width)
    {
        return Err(PresentationError::InvalidTopology);
    }
    match topology.utility() {
        None if provider.height == topology.window_height => {}
        Some(utility)
            if provider.height < topology.window_height
                && utility.left == provider.left
                && utility.width == provider.width
                && u32::from(utility.top)
                    == u32::from(provider.top) + u32::from(provider.height) + 1
                && u32::from(utility.top) + u32::from(utility.height)
                    == u32::from(topology.window_height) => {}
        _ => return Err(PresentationError::InvalidTopology),
    }
    Ok(())
}

fn parse_pane_id(value: &str) -> Option<String> {
    value
        .strip_prefix('%')
        .filter(|digits| {
            !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
        })
        .map(|_| value.to_owned())
}

fn parse_window_id(value: &str) -> bool {
    value.strip_prefix('@').is_some_and(|digits| {
        !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
    })
}

fn validate_shell_path(path: &Path) -> Result<(), PresentationError> {
    let value = path
        .to_str()
        .ok_or_else(|| PresentationError::InvalidControlPath(path.to_path_buf()))?;
    if !path.is_absolute()
        || value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(PresentationError::ControlRefused(
            "ordinary shell path is invalid",
        ));
    }
    Ok(())
}

fn shell_quote(value: &std::ffi::OsStr) -> Result<String, PresentationError> {
    let value = value
        .to_str()
        .ok_or_else(|| PresentationError::InvalidControlPath(PathBuf::from("non-UTF-8")))?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(PresentationError::ControlRefused(
            "presentation control path contains an invalid character",
        ));
    }
    // tmux expands format directives before invoking the shell used by
    // `run-shell`; POSIX quoting alone does not protect `#{...}` or `#(...)`.
    // A doubled hash is tmux's literal-hash escape. The source pane format is
    // intentionally emitted separately below and remains the only live
    // expansion.
    let value = value.replace('#', "##");
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn close_shell_arguments(client_name: &str, utility_pane: &str) -> Vec<OsString> {
    vec![
        "confirm-before".into(),
        "-t".into(),
        client_name.into(),
        "-p".into(),
        "Close utility shell? (y/n)".into(),
        format!("kill-pane -t {utility_pane}").into(),
    ]
}

fn presentation_session_name(directory: &Path) -> Option<String> {
    let identifier = directory
        .file_name()?
        .to_str()?
        .strip_prefix("presentation-")?;
    if identifier.len() != 12
        || !identifier
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!("{PRESENTATION_PREFIX}{identifier}"))
}

fn sanitize_diagnostic(diagnostic: &str) -> String {
    diagnostic
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(256)
        .collect()
}

fn pane_disappeared(error: &PresentationError) -> bool {
    matches!(error, PresentationError::TmuxRejected(message) if message.contains("no such pane") || message.contains("pane not found"))
}

fn validate_host_alias(host_alias: &str) -> Result<(), PresentationError> {
    if host_alias.is_empty() || host_alias.len() > 128 || host_alias.chars().any(char::is_control) {
        return Err(PresentationError::InvalidAttachmentStatus);
    }
    Ok(())
}

fn create_paths(paths: &PresentationPaths) -> Result<(), PresentationError> {
    let parent = paths
        .directory
        .parent()
        .ok_or_else(|| PresentationError::InvalidControlPath(paths.directory.clone()))?;
    fs::create_dir_all(parent).map_err(PresentationError::Io)?;
    set_mode(parent, 0o700)?;
    fs::create_dir(&paths.directory).map_err(PresentationError::Io)?;
    set_mode(&paths.directory, 0o700)?;
    fs::write(&paths.config, presentation_tmux_config()).map_err(PresentationError::Io)?;
    set_mode(&paths.config, 0o600)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), PresentationError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(PresentationError::Io)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), PresentationError> {
    Ok(())
}

fn stopped_owned_presentation(presentation_live: bool) -> bool {
    !presentation_live
}

fn should_reuse_presentation(session_live: bool, navigator_pane_dead: bool) -> bool {
    session_live && !navigator_pane_dead
}

/// Presentation ownership failures; no provider content is retained in their
/// diagnostics.
#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("multiple private navigator presentations are live; close one before reconnecting")]
    AmbiguousPresentations,
    #[error("invalid private presentation control path {0}")]
    InvalidControlPath(PathBuf),
    #[error("invalid private presentation control action")]
    InvalidControlAction,
    #[error("private presentation pane topology is ambiguous")]
    InvalidTopology,
    #[error("presentation control refused: {0}")]
    ControlRefused(&'static str),
    #[error("invalid private provider attachment status")]
    InvalidAttachmentStatus,
    #[error("provider attachment attempt is stale or already complete")]
    StaleAttachmentAttempt,
    #[error("invoking terminal geometry is unavailable")]
    TerminalGeometryUnavailable,
    #[error("invoking terminal geometry is invalid")]
    InvalidTerminalGeometry,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("private tmux output exceeded the diagnostic limit")]
    OutputTooLarge,
    #[error("private presentation tmux action failed: {0}")]
    TmuxRejected(String),
    #[error("could not execute bounded private presentation tmux command")]
    TmuxOutput(#[source] BoundedProcessError),
}

impl PresentationError {
    fn from_bounded_tmux(source: BoundedProcessError) -> Self {
        match source {
            BoundedProcessError::OutputTooLarge => Self::OutputTooLarge,
            other => Self::TmuxOutput(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DisposableTmuxServerGuard {
        socket: PathBuf,
        directory: Option<PathBuf>,
    }

    impl DisposableTmuxServerGuard {
        fn new(socket: PathBuf, directory: Option<PathBuf>) -> Self {
            Self { socket, directory }
        }
    }

    impl Drop for DisposableTmuxServerGuard {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .env_remove("TMUX")
                .args(["-f", "/dev/null", "-S"])
                .arg(&self.socket)
                .arg("kill-server")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = fs::remove_file(&self.socket);
            if let Some(directory) = &self.directory {
                let _ = fs::remove_dir_all(directory);
            }
        }
    }

    struct DisposableChildGuard {
        child: Option<std::process::Child>,
    }

    impl DisposableChildGuard {
        fn new(child: std::process::Child) -> Self {
            Self { child: Some(child) }
        }
    }

    impl Drop for DisposableChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[test]
    fn failed_outer_attach_accepts_a_stopped_owned_presentation() {
        assert!(stopped_owned_presentation(false));
    }

    #[test]
    fn failed_outer_attach_rejects_a_live_owned_presentation() {
        assert!(!stopped_owned_presentation(true));
    }

    #[test]
    fn attach_geometry_targets_exact_window_and_restores_latest() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        prepare_attach_window_with_size(&paths.session_name, 150, 40, |arguments| {
            calls.borrow_mut().push(arguments);
            Ok(())
        })
        .unwrap();

        let calls = calls.into_inner();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            vec![
                OsString::from("resize-window"),
                OsString::from("-t"),
                OsString::from(format!("{}:navigator", paths.session_name)),
                OsString::from("-x"),
                OsString::from("150"),
                OsString::from("-y"),
                OsString::from("40"),
            ]
        );
        assert_eq!(
            calls[1],
            vec![
                OsString::from("set-window-option"),
                OsString::from("-t"),
                OsString::from(format!("{}:navigator", paths.session_name)),
                OsString::from("window-size"),
                OsString::from("latest"),
            ]
        );
    }

    #[test]
    fn attach_geometry_rejection_stops_before_restore() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        let result = prepare_attach_window_with_size(&paths.session_name, 150, 40, |arguments| {
            calls.borrow_mut().push(arguments);
            Err(PresentationError::TmuxRejected(
                "resize rejected".to_owned(),
            ))
        });

        assert!(matches!(
            result,
            Err(PresentationError::TmuxRejected(message)) if message == "resize rejected"
        ));
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn attach_geometry_latest_rejection_stops_before_native_attach() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        let result = prepare_attach_window_with_size(&paths.session_name, 150, 40, |arguments| {
            let call_number = calls.borrow().len();
            calls.borrow_mut().push(arguments);
            if call_number == 0 {
                Ok(())
            } else {
                Err(PresentationError::TmuxRejected(
                    "latest rejected".to_owned(),
                ))
            }
        });

        assert!(matches!(
            result,
            Err(PresentationError::TmuxRejected(message)) if message == "latest rejected"
        ));
        assert_eq!(calls.borrow().len(), 2);
    }

    #[test]
    fn attach_geometry_rejects_zero_dimensions_without_tmux_access() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        assert!(matches!(
            prepare_attach_window_with_size(&paths.session_name, 0, 40, |arguments| {
                calls.borrow_mut().push(arguments);
                Ok(())
            }),
            Err(PresentationError::InvalidTerminalGeometry)
        ));
        assert!(calls.borrow().is_empty());
    }

    #[test]
    #[cfg(unix)]
    #[allow(clippy::too_many_lines)]
    fn detached_nested_private_windows_keep_geometry_and_copy_mode_profile() {
        use std::{process::Stdio, time::Instant};

        if Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
            || Command::new("script")
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_err()
        {
            eprintln!("skipped: tmux and script are required");
            return;
        }

        let temporary = tempfile::tempdir().unwrap();
        let fixture = temporary.path().join("fixture");
        fs::write(&fixture, "#!/bin/sh\nexec /usr/bin/sleep 60\n").unwrap();
        set_mode(&fixture, 0o700).unwrap();

        let tmux = |socket: &Path| {
            let mut command = Command::new("tmux");
            command.env_remove("TMUX").arg("-S").arg(socket);
            command
        };
        let output = |socket: &Path, arguments: &[&str]| -> String {
            let mut command = tmux(socket);
            command.args(arguments);
            let output =
                output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES).unwrap();
            assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
            String::from_utf8(output.stdout).unwrap()
        };
        let wait_for_clients = |socket: &Path| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut command = tmux(socket);
                command.args(["list-clients", "-F", "#{client_name}"]);
                if output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
                    .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
                {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("private tmux client did not attach");
        };
        let assert_geometry = |socket: &Path, target: &str, geometry: &str| {
            assert_eq!(
                output(
                    socket,
                    [
                        "display-message",
                        "-p",
                        "-t",
                        target,
                        "#{window_width}x#{window_height}",
                    ]
                    .as_slice(),
                )
                .trim(),
                geometry
            );
        };
        let assert_window = |socket: &Path, target: &str, geometry: &str| {
            assert_geometry(socket, target, geometry);
            assert_eq!(
                output(
                    socket,
                    ["show-window-options", "-v", "-t", target, "window-size"].as_slice(),
                )
                .trim(),
                "latest"
            );
        };
        let assert_copy_mode_scroll = |socket: &Path, repeat_count: &str| {
            for (table, key, direction) in [
                ("copy-mode", "WheelUpPane", "scroll-up"),
                ("copy-mode", "WheelDownPane", "scroll-down"),
                ("copy-mode-vi", "WheelUpPane", "scroll-up"),
                ("copy-mode-vi", "WheelDownPane", "scroll-down"),
            ] {
                let expected = format!(
                    "bind-key -T {table} {key} select-pane \\; send-keys -X -N {repeat_count} {direction}"
                );
                let bindings = output(socket, ["list-keys", "-T", table].as_slice());
                assert!(
                    bindings
                        .lines()
                        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
                        .any(|line| line == expected),
                    "missing copy-mode binding: {expected}\n{bindings}"
                );
            }
        };
        let set_copy_mode_scroll_repeat = |socket: &Path, repeat_count: &str| {
            for (table, key, direction) in [
                ("copy-mode", "WheelUpPane", "scroll-up"),
                ("copy-mode", "WheelDownPane", "scroll-down"),
                ("copy-mode-vi", "WheelUpPane", "scroll-up"),
                ("copy-mode-vi", "WheelDownPane", "scroll-down"),
            ] {
                assert!(
                    tmux(socket)
                        .args([
                            "bind-key",
                            "-T",
                            table,
                            key,
                            "select-pane",
                            "\\;",
                            "send-keys",
                            "-X",
                            "-N",
                            repeat_count,
                            direction,
                        ])
                        .status()
                        .unwrap()
                        .success()
                );
            }
        };

        let presentation = Presentation::fresh_with_executable(temporary.path(), fixture);
        presentation.start().unwrap();
        let _presentation_guard = DisposableTmuxServerGuard::new(
            presentation.paths().socket.clone(),
            Some(presentation.paths().directory.clone()),
        );
        let presentation_socket = presentation.paths().socket.clone();
        let presentation_target = format!("{}:navigator", presentation.paths().session_name);
        assert_copy_mode_scroll(&presentation_socket, "1");

        let tmux_client = crate::runtime::SystemTmux::default();
        let process_probe = crate::runtime::LinuxProcessProbe;
        let runtime = crate::runtime::PrivateRuntime::new(
            &tmux_client,
            &process_probe,
            crate::runtime::RuntimePaths::for_runtime(
                temporary.path(),
                crate::domain::RuntimeId::new(),
            ),
        );
        runtime
            .start(&crate::runtime::NativeLaunch {
                cwd: temporary.path().to_path_buf(),
                program: vec![
                    OsString::from("/bin/sh"),
                    OsString::from("-c"),
                    OsString::from("sleep 60"),
                ],
                environment: std::collections::BTreeMap::new(),
            })
            .unwrap();
        let _runtime_guard = DisposableTmuxServerGuard::new(
            runtime.paths().socket.clone(),
            Some(runtime.paths().directory.clone()),
        );
        let runtime_target = format!("{}:provider", runtime.paths().session_name);
        assert_copy_mode_scroll(&runtime.paths().socket, "1");
        // Simulate a long-lived Runtime created before this profile existed.
        // The attach preparation below must converge these exact bindings
        // without restarting the sleeping provider pane.
        set_copy_mode_scroll_repeat(&runtime.paths().socket, "5");
        assert_copy_mode_scroll(&runtime.paths().socket, "5");
        assert_geometry(&presentation_socket, &presentation_target, "80x24");
        assert_geometry(&runtime.paths().socket, &runtime_target, "80x24");

        // This is the final outer PTY geometry used by the disposable nested
        // client. Both private windows are still detached at this point.
        let final_columns = 150;
        let final_rows = 40;
        presentation
            .prepare_attach_with_size(final_columns, final_rows)
            .unwrap();
        assert_window(&presentation_socket, &presentation_target, "150x40");

        let outer_socket = temporary.path().join("outer.sock");
        let outer_session = format!("outer-{}", uuid::Uuid::new_v4().simple());
        let status = tmux(&outer_socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                &outer_session,
                "/usr/bin/sleep",
                "60",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let _outer_guard = DisposableTmuxServerGuard::new(outer_socket.clone(), None);
        assert!(
            tmux(&outer_socket)
                .args(["set-option", "-g", "status", "off"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            tmux(&outer_socket)
                .args(["resize-window", "-t", &format!("{outer_session}:0")])
                .args(["-x", "150", "-y", "40"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            tmux(&outer_socket)
                .args([
                    "set-window-option",
                    "-t",
                    &format!("{outer_session}:0"),
                    "window-size",
                    "latest",
                ])
                .status()
                .unwrap()
                .success()
        );
        let nested_presentation_attach = format!(
            "env -u TMUX tmux -S {} attach-session -t {}",
            shell_quote(presentation_socket.as_os_str()).unwrap(),
            shell_quote(Path::new(&presentation.paths().session_name).as_os_str()).unwrap(),
        );
        assert!(
            tmux(&outer_socket)
                .args(["respawn-pane", "-k", "-t", &format!("{outer_session}:0.0")])
                .arg(nested_presentation_attach)
                .status()
                .unwrap()
                .success()
        );

        let outer_attach = format!(
            "stty rows 40 cols 150; exec env -u TMUX tmux -S {} attach-session -t {}",
            shell_quote(outer_socket.as_os_str()).unwrap(),
            shell_quote(Path::new(&outer_session).as_os_str()).unwrap(),
        );
        let outer_client = Command::new("script")
            .env("TERM", "xterm-256color")
            .args(["-qefc", &outer_attach, "/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _outer_client_guard = DisposableChildGuard::new(outer_client);
        wait_for_clients(&presentation_socket);
        assert_window(&presentation_socket, &presentation_target, "150x40");

        let panes = output(
            &presentation_socket,
            [
                "list-panes",
                "-t",
                &presentation_target,
                "-F",
                "#{pane_id}\t#{@wsnav_role}\t#{pane_width}\t#{pane_height}",
            ]
            .as_slice(),
        );
        let provider = panes
            .lines()
            .find_map(|line| {
                let mut fields = line.split('\t');
                let pane_id = fields.next()?;
                let role = fields.next()?;
                let columns = fields.next()?.parse::<u16>().ok()?;
                let rows = fields.next()?.parse::<u16>().ok()?;
                (role == "provider").then_some((pane_id.to_owned(), columns, rows))
            })
            .expect("provider pane geometry");
        assert!(provider.1 > 0 && provider.2 > 0);

        runtime
            .prepare_attach_with_size(provider.1, provider.2)
            .unwrap();
        assert_copy_mode_scroll(&presentation_socket, "1");
        assert_copy_mode_scroll(&runtime.paths().socket, "1");
        let runtime_geometry = format!("{}x{}", provider.1, provider.2);
        assert_window(&runtime.paths().socket, &runtime_target, &runtime_geometry);

        let nested_runtime_attach = format!(
            "env -u TMUX tmux -u -S {} attach-session -t {}",
            shell_quote(runtime.paths().socket.as_os_str()).unwrap(),
            shell_quote(Path::new(&runtime.paths().session_name).as_os_str()).unwrap(),
        );
        assert!(
            tmux(&presentation_socket)
                .args(["respawn-pane", "-k", "-t", &provider.0])
                .arg(nested_runtime_attach)
                .status()
                .unwrap()
                .success()
        );
        wait_for_clients(&runtime.paths().socket);
        assert_window(&runtime.paths().socket, &runtime_target, &runtime_geometry);
    }

    #[test]
    fn dead_navigator_pane_is_not_reused_even_when_the_session_is_live() {
        assert!(should_reuse_presentation(true, false));
        assert!(!should_reuse_presentation(true, true));
        assert!(!should_reuse_presentation(false, false));
    }

    #[test]
    fn navigator_liveness_probe_targets_only_the_exact_owned_pane() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let arguments = presentation.pane_dead_arguments("%42");

        assert_eq!(arguments[0], "display-message");
        assert_eq!(arguments[2], "-t");
        assert_eq!(arguments[3], std::ffi::OsString::from("%42"));
        assert_eq!(arguments[4], "#{pane_dead}");
        assert!(arguments.iter().all(|argument| argument != "0.1"));
    }

    #[test]
    fn presentation_config_selects_the_clicked_pane_on_mouse_release() {
        let config = presentation_tmux_config();
        assert!(config.contains("set -g mouse on"));
        assert!(config.contains("bind-key -T root MouseUp1Pane select-pane -t = \\; send-keys -M"));
        assert!(config.contains("WheelUpPane"));
        assert!(!config.contains("MouseDown3Pane"));
    }

    #[test]
    fn presentation_config_rebuilds_bounded_d12_allowlists() {
        assert_eq!(
            presentation_tmux_config(),
            concat!(
                "set -g status off\n",
                "set -g mouse on\n",
                "set -g remain-on-exit on\n",
                "set -g prefix C-b\n",
                "set -g prefix2 None\n",
                "unbind-key -a -T prefix\n",
                "unbind-key -a -T root\n",
                "set -g default-terminal tmux-256color\n",
                "set-environment -g COLORTERM truecolor\n",
                "set -g extended-keys always\n",
                "set -q -g extended-keys-format csi-u\n",
                "set -as terminal-features ',xterm-ghostty:RGB:extkeys'\n",
                "set -as terminal-features ',tmux-256color:RGB:extkeys'\n",
                "bind-key -T copy-mode WheelUpPane select-pane \\; send-keys -X -N 1 scroll-up\n",
                "bind-key -T copy-mode WheelDownPane select-pane \\; send-keys -X -N 1 scroll-down\n",
                "bind-key -T copy-mode-vi WheelUpPane select-pane \\; send-keys -X -N 1 scroll-up\n",
                "bind-key -T copy-mode-vi WheelDownPane select-pane \\; send-keys -X -N 1 scroll-down\n",
                "bind-key -T root MouseDown1Pane select-pane -t = \\; send-keys -M\n",
                "bind-key -T root MouseUp1Pane select-pane -t = \\; send-keys -M\n",
                "bind-key -T root MouseDrag1Pane if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -M\"\n",
                "bind-key -T root WheelUpPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -e\"\n",
                "bind-key -T root WheelDownPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"send-keys -M\"\n",
            )
        );
    }

    #[test]
    fn topology_parser_rejects_dead_duplicate_and_unknown_roles() {
        let valid = concat!(
            "%0\tnavigator\t\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\tlocal\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t24\t128\t24\n",
        );
        assert!(parse_topology(valid).is_ok());
        assert!(matches!(
            parse_topology(&valid.replace("\t0\t0\t0\t32", "\t1\t0\t0\t32")),
            Err(PresentationError::InvalidTopology)
        ));
        let duplicate = valid.replace("%1\tprovider", "%0\tprovider");
        assert!(matches!(
            parse_topology(&duplicate),
            Err(PresentationError::InvalidTopology)
        ));
        let unknown = valid.replace("provider", "unknown");
        assert!(matches!(
            parse_topology(&unknown),
            Err(PresentationError::InvalidTopology)
        ));
        assert!(
            parse_topology_with_dead(&valid.replace("\t0\t0\t0\t32", "\t1\t0\t0\t32"), true)
                .is_ok()
        );
    }

    #[test]
    fn tmux_window_id_requires_an_at_sign_and_decimal_digits() {
        assert!(parse_window_id("@0"));
        assert!(parse_window_id("@123"));
        assert!(!parse_window_id("@"));
        assert!(!parse_window_id("@window"));
        assert!(!parse_window_id("%0"));
    }

    #[test]
    fn topology_parser_rejects_unsupported_geometry() {
        let valid = concat!(
            "%0\tnavigator\t\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\tlocal\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t24\t128\t24\n",
        );
        assert!(parse_topology(valid).is_ok());
        assert!(parse_topology(&valid.replace("\t33\t0\t95\t24", "\t34\t0\t94\t24")).is_err());
        assert!(
            parse_topology(&valid.replace("\t0\t0\t32\t24\t128", "\t1\t0\t32\t24\t128")).is_err()
        );
        assert!(parse_topology(&valid.replace("\t128\t24", "\t127\t24")).is_err());

        let three_pane = concat!(
            "%0\tnavigator\t\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\tlocal\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t11\t128\t24\n",
            "%2\tutility\tlocal\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t12\t95\t12\t128\t24\n",
        );
        assert!(parse_topology(three_pane).is_ok());
        assert!(
            parse_topology(&three_pane.replace("\t12\t95\t12\t128", "\t11\t95\t13\t128")).is_err()
        );
        assert!(
            parse_topology(&three_pane.replace("\t33\t12\t95\t12", "\t34\t12\t94\t12")).is_err()
        );
    }

    #[test]
    fn control_binding_uses_fixed_shell_quoting_and_tmux_format_source() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state's root/#{danger}/#(marker)");
        let presentation = Presentation {
            paths: PresentationPaths::fresh(&state_root),
            executable: PathBuf::from("/tmp/wsnav's executable/#{danger}/#(marker)"),
            state_root,
        };

        let command = presentation
            .control_shell_command(PresentationAction::SuppressSplit)
            .unwrap();

        assert!(command.contains("'/tmp/wsnav'\\''s executable/##{danger}/##(marker)'"));
        assert!(command.contains("##{danger}"));
        assert!(command.contains("##(marker)"));
        let source_only = command.replace("##{danger}", "").replace("##(marker)", "");
        assert_eq!(source_only.matches("#{").count(), 2);
        assert!(!source_only.contains("#("));
        assert!(command.contains("--action suppress-split"));
        assert!(command.contains("--source-pane '#{pane_id}'"));
        assert!(command.contains("--client-name #{q:client_name}"));
        assert!(!command.contains("; tmux"));
        assert!(!command.contains("split-window"));
    }

    #[test]
    fn close_shell_targets_the_invoking_client_and_exact_utility_pane() {
        let arguments = close_shell_arguments("/dev/pts/9", "%7");
        let values = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                "confirm-before",
                "-t",
                "/dev/pts/9",
                "-p",
                "Close utility shell? (y/n)",
                "kill-pane -t %7",
            ]
        );
        assert_ne!(values[2], values[5]);
    }

    #[test]
    fn navigator_default_width_is_exactly_32_cells() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let arguments = presentation.default_navigator_resize_arguments_for("%0");

        assert_eq!(DEFAULT_NAVIGATOR_PANE_WIDTH, 32);
        assert_eq!(arguments[0], "resize-pane");
        assert_eq!(arguments[1], "-t");
        assert_eq!(arguments[3], "-x");
        assert_eq!(arguments[4], "32");
    }

    #[test]
    fn navigator_width_hooks_target_only_the_exact_private_pane() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let exact_target = "%0".to_owned();

        for hook in NAVIGATOR_WIDTH_HOOKS {
            let arguments = presentation.navigator_width_hook_arguments_for(hook, "%0");
            assert_eq!(arguments[0], "set-hook");
            assert_eq!(arguments[1], "-t");
            assert_eq!(
                arguments[2],
                OsString::from(&presentation.paths.session_name)
            );
            assert_eq!(arguments[3], hook);
            assert_eq!(
                arguments[4],
                OsString::from(format!(
                    "resize-pane -t {exact_target} -x {DEFAULT_NAVIGATOR_PANE_WIDTH}"
                ))
            );
            assert!(arguments.iter().all(|argument| argument != "run-shell"));
            assert!(arguments.iter().all(|argument| argument != PROVIDER_PANE));
        }
    }

    #[test]
    fn attachment_status_advances_only_the_exact_ephemeral_attempt() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let workstream_id = WorkstreamId::new();
        let pending = presentation
            .prepare_attachment("snap", workstream_id)
            .unwrap();

        assert_eq!(
            presentation.read_attachment_status().unwrap(),
            Some(pending.clone())
        );
        assert!(matches!(
            presentation.report_attachment_phase(uuid::Uuid::new_v4(), AttachmentPhase::Running),
            Err(PresentationError::StaleAttachmentAttempt)
        ));

        presentation
            .report_attachment_phase(pending.attempt_id, AttachmentPhase::Running)
            .unwrap();
        presentation
            .report_attachment_phase(pending.attempt_id, AttachmentPhase::Failed)
            .unwrap();
        let failed = presentation.read_attachment_status().unwrap().unwrap();
        assert_eq!(failed.phase, AttachmentPhase::Failed);
        assert_eq!(failed.host_alias, "snap");
        assert_eq!(failed.workstream_id, workstream_id);
        assert!(matches!(
            presentation.report_attachment_phase(pending.attempt_id, AttachmentPhase::Running),
            Err(PresentationError::StaleAttachmentAttempt)
        ));
    }

    #[test]
    fn running_attachment_status_does_not_probe_the_presentation_tmux_server() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let pending = presentation
            .prepare_attachment("local", WorkstreamId::new())
            .unwrap();
        presentation
            .report_attachment_phase(pending.attempt_id, AttachmentPhase::Running)
            .unwrap();

        let status = presentation.attachment_status().unwrap().unwrap();

        assert_eq!(status.attempt_id, pending.attempt_id);
        assert_eq!(status.phase, AttachmentPhase::Running);
    }

    #[test]
    #[cfg(unix)]
    fn attachment_status_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        presentation
            .prepare_attachment("local", WorkstreamId::new())
            .unwrap();

        assert_eq!(
            fs::metadata(&presentation.paths.attachment_status)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn provider_attachment_uses_direct_arguments_not_a_shell() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let command =
            presentation.provider_attach_command(WorkstreamId::new(), uuid::Uuid::new_v4());
        assert!(
            command
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
        assert!(
            command
                .iter()
                .any(|argument| argument == "_provider_attach")
        );
        assert_eq!(command.len(), 11);
    }

    #[test]
    fn observer_review_uses_only_the_owned_provider_pane_and_direct_arguments() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let command = presentation.observer_review_command();

        assert_eq!(command[0], "/workspace/wsnav");
        assert_eq!(command[1], "--state-root");
        assert_eq!(command[3], "_observer_review");
        assert!(
            command
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
    }

    #[test]
    fn remote_observer_review_uses_a_direct_provider_pane_command() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let command = presentation.remote_observer_review_command("snap");

        assert_eq!(command[0], "/workspace/wsnav");
        assert_eq!(command[3], "_provider_remote_observer_review");
        assert_eq!(command[4], "snap");
        assert!(command.iter().all(|argument| argument != "sh"));
    }

    #[test]
    fn provider_respawn_forwards_the_complete_direct_attachment_command() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let workstream_id = WorkstreamId::new();
        let arguments = presentation.provider_respawn_arguments(
            PROVIDER_PANE,
            workstream_id,
            uuid::Uuid::new_v4(),
        );

        assert_eq!(arguments.len(), 15);
        assert_eq!(arguments[0], "respawn-pane");
        assert_eq!(arguments[4], "/workspace/wsnav");
        assert_eq!(arguments[7], "_provider_attach");
        assert_eq!(arguments[8], OsString::from(workstream_id.to_string()));
        assert_eq!(arguments[9], "--presentation-socket");
        assert_eq!(arguments[13], "--attempt-id");
    }

    #[test]
    fn remote_provider_attachment_uses_only_fixed_host_command_arguments() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let workstream_id = WorkstreamId::new();

        let arguments = presentation.provider_remote_respawn_arguments(
            PROVIDER_PANE,
            "snap",
            workstream_id,
            uuid::Uuid::new_v4(),
        );

        assert_eq!(arguments[4], "/workspace/wsnav");
        assert_eq!(arguments[5], "--state-root");
        assert_eq!(arguments[7], "_provider_remote_attach");
        assert_eq!(arguments[8], "snap");
        assert_eq!(arguments[9], OsString::from(workstream_id.to_string()));
        assert_eq!(arguments[10], "--presentation-socket");
        assert_eq!(arguments[14], "--attempt-id");
    }

    #[test]
    fn remote_shell_barrier_carries_only_endpoint_and_opaque_workstream_id() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().join("state's root/#(marker)"),
        };
        let workstream_id = WorkstreamId::new();
        let command =
            presentation.remote_shell_command("snap", "/home/user/.local/bin/wsnav", workstream_id);
        let values = command
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(values[3], "_presentation_ssh_shell");
        assert_eq!(values[8], "--destination");
        assert_eq!(values[9], "snap");
        assert_eq!(values[10], "--executable");
        assert_eq!(values[11], "/home/user/.local/bin/wsnav");
        assert_eq!(values[12], "--workstream-id");
        assert_eq!(values[13], workstream_id.to_string());
        assert!(!values.iter().any(|value| value == "/private/project"));
        assert!(!values.iter().any(|value| value.contains("cwd")));
    }

    #[test]
    fn control_path_rejects_the_default_tmux_socket() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(
            PresentationPaths::from_control(
                temporary.path(),
                PathBuf::from("/tmp/tmux-default"),
                "wsnav-presentation-example".to_owned(),
            )
            .is_err()
        );
    }

    #[test]
    fn control_path_requires_the_exact_owned_session_name() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        assert!(
            PresentationPaths::from_control(
                temporary.path(),
                paths.socket,
                "wsnav-presentation-other".to_owned(),
            )
            .is_err()
        );
    }
}
