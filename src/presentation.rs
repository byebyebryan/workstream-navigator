//! Disposable private tmux ownership for the local navigator presentation.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    domain::WorkstreamId,
    process::{BoundedProcessError, output_bounded},
};

const PRESENTATION_DIRECTORY: &str = "presentation";
const PRESENTATION_PREFIX: &str = "wsnav-presentation-";
const NAVIGATOR_WINDOW: &str = "navigator";
const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
/// The normal narrow navigator width, including its outside borders.
const DEFAULT_NAVIGATOR_PANE_WIDTH: u16 = 32;
const PREFERRED_PROVIDER_PANE_WIDTH: u16 = 96;
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_ATTACHMENT_STATUS_BYTES: u64 = 4 * 1024;
const ATTACHMENT_STATUS_FILE: &str = "attachment.json";
const PRESENTATION_TMUX_CONFIG: &str = concat!(
    "set -g status off\n",
    "set -g mouse on\n",
    "set -g remain-on-exit on\n",
    "set -g default-terminal tmux-256color\n",
    "set-environment -g COLORTERM truecolor\n",
    // The provider pane is a nested tmux client. Keep RGB styling and
    // modified keys intact both from Ghostty into this presentation and from
    // this presentation into the provider Runtime.
    "set -g extended-keys always\n",
    "set -g extended-keys-format csi-u\n",
    "set -as terminal-features ',xterm-ghostty:RGB:extkeys'\n",
    "set -as terminal-features ',tmux-256color:RGB:extkeys'\n",
    "bind-key -n MouseUp1Pane select-pane -t = \\; send-keys -M\n",
);

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
        Ok(Self {
            paths: PresentationPaths::fresh(state_root),
            executable: std::env::current_exe().map_err(PresentationError::Io)?,
            state_root: state_root.to_path_buf(),
        })
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
        if let Err(error) = self.set_default_navigator_width() {
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
        let status = self.prepare_attachment("local", workstream_id)?;
        let result = self.invoke(
            None,
            self.provider_respawn_arguments(workstream_id, status.attempt_id),
        );
        self.finish_attachment_start(status, result)
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
        let status = self.prepare_attachment(host_alias, workstream_id)?;
        let result = self.invoke(
            None,
            self.provider_remote_respawn_arguments(host_alias, workstream_id, status.attempt_id),
        );
        self.finish_attachment_start(status, result)
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
        self.invoke(
            None,
            self.provider_respawn_for_command(self.observer_review_command()),
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
        self.invoke(
            None,
            self.provider_respawn_for_command(self.remote_observer_review_command(host_alias)),
        )
    }

    fn provider_respawn_arguments(
        &self,
        workstream_id: WorkstreamId,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        let command = self.provider_attach_command(workstream_id, attempt_id);
        self.provider_respawn_for_command(command)
    }

    fn provider_remote_respawn_arguments(
        &self,
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
        self.provider_respawn_for_command(command)
    }

    fn provider_respawn_for_command(&self, command: Vec<OsString>) -> Vec<OsString> {
        let mut arguments = vec![
            "respawn-pane".into(),
            "-k".into(),
            "-t".into(),
            format!("{}:{PROVIDER_PANE}", self.paths.session_name).into(),
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
        self.invoke(
            None,
            vec![
                "select-pane".into(),
                "-t".into(),
                format!("{}:{PROVIDER_PANE}", self.paths.session_name).into(),
            ],
        )
    }

    /// Gives keyboard focus to the navigator pane without touching a provider
    /// Runtime or its attachment helper.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned pane cannot be focused.
    pub fn focus_navigator(&self) -> Result<(), PresentationError> {
        self.invoke(
            None,
            vec![
                "select-pane".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_PANE}", self.paths.session_name).into(),
            ],
        )
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
            if presentation.is_live()? {
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
        let mut command = Command::new("tmux");
        command
            .env_remove("TMUX")
            .arg("-S")
            .arg(&self.paths.socket)
            .args([
                "display-message",
                "-p",
                "-t",
                &format!("{}:{PROVIDER_PANE}", self.paths.session_name),
                "#{pane_dead}",
            ]);
        let output = output_bounded(&mut command, 16, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if !output.status.success() {
            return Err(PresentationError::TmuxRejected(sanitize_diagnostic(
                &String::from_utf8_lossy(&output.stderr),
            )));
        }
        match output.stdout.as_slice() {
            b"0\n" | b"0\r\n" => Ok(false),
            b"1\n" | b"1\r\n" => Ok(true),
            _ => Err(PresentationError::InvalidAttachmentStatus),
        }
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
        self.invoke(None, self.default_navigator_resize_arguments())
    }

    fn default_navigator_resize_arguments(&self) -> Vec<OsString> {
        vec![
            "resize-pane".into(),
            "-t".into(),
            format!("{}:{NAVIGATOR_PANE}", self.paths.session_name).into(),
            "-x".into(),
            DEFAULT_NAVIGATOR_PANE_WIDTH.to_string().into(),
        ]
    }

    fn invoke(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<(), PresentationError> {
        let mut command = Command::new("tmux");
        command.env_remove("TMUX");
        if let Some(config) = config {
            command.arg("-f").arg(config);
        }
        command.arg("-S").arg(&self.paths.socket).args(arguments);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(PresentationError::TmuxRejected(sanitize_diagnostic(
                &String::from_utf8_lossy(&output.stderr),
            )))
        }
    }
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
    fs::write(&paths.config, PRESENTATION_TMUX_CONFIG).map_err(PresentationError::Io)?;
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

/// Presentation ownership failures; no provider content is retained in their
/// diagnostics.
#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("multiple private navigator presentations are live; close one before reconnecting")]
    AmbiguousPresentations,
    #[error("invalid private presentation control path {0}")]
    InvalidControlPath(PathBuf),
    #[error("invalid private provider attachment status")]
    InvalidAttachmentStatus,
    #[error("provider attachment attempt is stale or already complete")]
    StaleAttachmentAttempt,
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

    #[test]
    fn failed_outer_attach_accepts_a_stopped_owned_presentation() {
        assert!(stopped_owned_presentation(false));
    }

    #[test]
    fn failed_outer_attach_rejects_a_live_owned_presentation() {
        assert!(!stopped_owned_presentation(true));
    }

    #[test]
    fn presentation_config_selects_the_clicked_pane_on_mouse_release() {
        assert!(PRESENTATION_TMUX_CONFIG.contains("set -g mouse on"));
        assert!(
            PRESENTATION_TMUX_CONFIG
                .contains("bind-key -n MouseUp1Pane select-pane -t = \\; send-keys -M")
        );
    }

    #[test]
    fn presentation_config_preserves_ghostty_rgb_and_extended_keys() {
        assert!(PRESENTATION_TMUX_CONFIG.contains("set -g default-terminal tmux-256color"));
        assert!(PRESENTATION_TMUX_CONFIG.contains("set-environment -g COLORTERM truecolor"));
        assert!(PRESENTATION_TMUX_CONFIG.contains("set -g extended-keys always"));
        assert!(PRESENTATION_TMUX_CONFIG.contains("set -g extended-keys-format csi-u"));
        assert!(
            PRESENTATION_TMUX_CONFIG
                .contains("set -as terminal-features ',xterm-ghostty:RGB:extkeys'")
        );
        assert!(
            PRESENTATION_TMUX_CONFIG
                .contains("set -as terminal-features ',tmux-256color:RGB:extkeys'")
        );
    }

    #[test]
    fn navigator_default_width_is_exactly_32_cells() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let arguments = presentation.default_navigator_resize_arguments();

        assert_eq!(DEFAULT_NAVIGATOR_PANE_WIDTH, 32);
        assert_eq!(arguments[0], "resize-pane");
        assert_eq!(arguments[1], "-t");
        assert_eq!(arguments[3], "-x");
        assert_eq!(arguments[4], "32");
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
        let arguments =
            presentation.provider_respawn_arguments(workstream_id, uuid::Uuid::new_v4());

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
