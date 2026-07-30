//! Disposable private tmux ownership for the local navigator presentation.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use thiserror::Error;

use crate::domain::WorkstreamId;

const PRESENTATION_DIRECTORY: &str = "presentation";
const PRESENTATION_PREFIX: &str = "wsnav-presentation-";
const NAVIGATOR_WINDOW: &str = "navigator";
const PROVIDER_PANE: &str = "0.1";
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;

/// The exact private paths and tmux session owned by one navigator client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationPaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
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
                "96".into(),
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
            Ok(())
        } else {
            Err(PresentationError::TmuxRejected(
                "presentation attach failed".to_owned(),
            ))
        }
    }

    /// Replaces only the outer provider attachment helper. The managed Codex
    /// runtime remains in its own private tmux server.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux rejects replacement of the exact owned pane.
    pub fn attach_workstream(&self, workstream_id: WorkstreamId) -> Result<(), PresentationError> {
        self.invoke(None, self.provider_respawn_arguments(workstream_id))
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
    ) -> Result<(), PresentationError> {
        self.invoke(
            None,
            self.provider_remote_respawn_arguments(host_alias, workstream_id),
        )
    }

    fn provider_respawn_arguments(&self, workstream_id: WorkstreamId) -> Vec<OsString> {
        let command = self.provider_attach_command(workstream_id);
        self.provider_respawn_for_command(command)
    }

    fn provider_remote_respawn_arguments(
        &self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> Vec<OsString> {
        let command = vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_remote_attach".into(),
            host_alias.into(),
            workstream_id.to_string().into(),
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
        let output = Command::new("tmux")
            .env_remove("TMUX")
            .arg("-S")
            .arg(&self.paths.socket)
            .args(["has-session", "-t", &self.paths.session_name])
            .output()
            .map_err(PresentationError::Io)?;
        if output.stdout.len() > MAX_TMUX_OUTPUT_BYTES
            || output.stderr.len() > MAX_TMUX_OUTPUT_BYTES
        {
            return Err(PresentationError::OutputTooLarge);
        }
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

    fn provider_attach_command(&self, workstream_id: WorkstreamId) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_attach".into(),
            workstream_id.to_string().into(),
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
        let output = command
            .arg("-S")
            .arg(&self.paths.socket)
            .args(arguments)
            .output()
            .map_err(PresentationError::Io)?;
        if output.stdout.len() > MAX_TMUX_OUTPUT_BYTES
            || output.stderr.len() > MAX_TMUX_OUTPUT_BYTES
        {
            return Err(PresentationError::OutputTooLarge);
        }
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

fn create_paths(paths: &PresentationPaths) -> Result<(), PresentationError> {
    let parent = paths
        .directory
        .parent()
        .ok_or_else(|| PresentationError::InvalidControlPath(paths.directory.clone()))?;
    fs::create_dir_all(parent).map_err(PresentationError::Io)?;
    set_mode(parent, 0o700)?;
    fs::create_dir(&paths.directory).map_err(PresentationError::Io)?;
    set_mode(&paths.directory, 0o700)?;
    fs::write(
        &paths.config,
        "set -g status off\nset -g mouse on\nset -g remain-on-exit on\nset -g default-terminal tmux-256color\nset-environment -g COLORTERM truecolor\n",
    )
    .map_err(PresentationError::Io)?;
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

/// Presentation ownership failures; no provider content is retained in their
/// diagnostics.
#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("multiple private navigator presentations are live; close one before reconnecting")]
    AmbiguousPresentations,
    #[error("invalid private presentation control path {0}")]
    InvalidControlPath(PathBuf),
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("private tmux output exceeded the diagnostic limit")]
    OutputTooLarge,
    #[error("private presentation tmux action failed: {0}")]
    TmuxRejected(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_attachment_uses_direct_arguments_not_a_shell() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let command = presentation.provider_attach_command(WorkstreamId::new());
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
        assert_eq!(command.len(), 5);
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
        let arguments = presentation.provider_respawn_arguments(workstream_id);

        assert_eq!(arguments.len(), 9);
        assert_eq!(arguments[0], "respawn-pane");
        assert_eq!(arguments[4], "/workspace/wsnav");
        assert_eq!(arguments[7], "_provider_attach");
        assert_eq!(arguments[8], OsString::from(workstream_id.to_string()));
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

        let arguments = presentation.provider_remote_respawn_arguments("snap", workstream_id);

        assert_eq!(arguments[4], "/workspace/wsnav");
        assert_eq!(arguments[5], "--state-root");
        assert_eq!(arguments[7], "_provider_remote_attach");
        assert_eq!(arguments[8], "snap");
        assert_eq!(arguments[9], OsString::from(workstream_id.to_string()));
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
