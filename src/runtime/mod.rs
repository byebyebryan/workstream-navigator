//! Private tmux runtime ownership and bounded process probes.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use thiserror::Error;

use crate::domain::RuntimeId;

const RUNTIME_DIRECTORY: &str = "run";
const PROVIDER_WINDOW: &str = "provider";
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;

/// A private runtime server's owned paths and stable tmux session name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimePaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
    pub session_name: String,
}

impl RuntimePaths {
    /// Derives the only private tmux path set allowed for a runtime.
    #[must_use]
    pub fn for_runtime(state_root: &Path, runtime_id: RuntimeId) -> Self {
        let directory = state_root
            .join(RUNTIME_DIRECTORY)
            .join(format!("runtime-{}", runtime_id.short()));
        Self {
            socket: directory.join("tmux.sock"),
            config: directory.join("tmux.conf"),
            session_name: format!("wsnav-{}", runtime_id.short()),
            directory,
        }
    }
}

/// Program and environment passed unchanged to the native provider inside tmux.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLaunch {
    pub cwd: PathBuf,
    pub program: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
}

impl NativeLaunch {
    /// Validates that a native process can be started without shell expansion.
    ///
    /// # Errors
    ///
    /// Returns an error when the working directory is not a directory or the
    /// program vector is empty.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.program.is_empty() {
            return Err(RuntimeError::EmptyProgram);
        }
        if !self.cwd.is_dir() {
            return Err(RuntimeError::InvalidWorkingDirectory(self.cwd.clone()));
        }
        Ok(())
    }
}

/// The observable state of one private runtime server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeProbe {
    Missing,
    Live {
        pane_id: String,
        pane_pid: u32,
        cwd: PathBuf,
        process_birth: Option<String>,
    },
    Unknown {
        diagnostic: String,
    },
}

/// An owned tmux invocation, represented without a shell command string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TmuxInvocation {
    pub socket: PathBuf,
    pub config: Option<PathBuf>,
    pub arguments: Vec<OsString>,
}

/// A bounded tmux command result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TmuxResponse {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The only tmux boundary used by runtime ownership logic.
pub trait TmuxClient {
    /// Runs one tmux command against the supplied private socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the command cannot be launched or its output
    /// exceeds the bounded diagnostic limit.
    fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError>;
}

/// The system tmux adapter. It deliberately removes inherited default-socket state.
#[derive(Clone, Debug)]
pub struct SystemTmux {
    executable: OsString,
}

impl Default for SystemTmux {
    fn default() -> Self {
        Self {
            executable: OsString::from("tmux"),
        }
    }
}

impl SystemTmux {
    /// Creates a system adapter for a fixed tmux executable path.
    #[must_use]
    pub fn new(executable: impl Into<OsString>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
}

impl TmuxClient for SystemTmux {
    fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
        let mut command = Command::new(&self.executable);
        command.env_remove("TMUX");
        if let Some(config) = &invocation.config {
            command.arg("-f").arg(config);
        }
        command.arg("-S").arg(&invocation.socket);
        command.args(&invocation.arguments);
        let output = command
            .output()
            .map_err(|source| RuntimeError::TmuxLaunch {
                executable: self.executable.clone(),
                source,
            })?;
        response_from_output(output.status, &output.stdout, &output.stderr)
    }
}

/// Platform process metadata used only to corroborate a private tmux pane.
pub trait ProcessProbe {
    /// Returns a stable process-birth token for a live process.
    fn process_birth(&self, pid: u32) -> Option<String>;
}

/// Linux process-birth probe backed by the process stat file.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxProcessProbe;

impl ProcessProbe for LinuxProcessProbe {
    fn process_birth(&self, pid: u32) -> Option<String> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close_paren = stat.rfind(')')?;
        let start_time = stat.get(close_paren + 2..)?.split_whitespace().nth(19)?;
        Some(start_time.to_owned())
    }
}

/// Owns exactly one tmux server/session/window/pane for one runtime.
pub struct PrivateRuntime<'a> {
    tmux: &'a dyn TmuxClient,
    process_probe: &'a dyn ProcessProbe,
    paths: RuntimePaths,
}

impl<'a> PrivateRuntime<'a> {
    /// Constructs an owner for an as-yet-uncreated private tmux runtime.
    #[must_use]
    pub fn new(
        tmux: &'a dyn TmuxClient,
        process_probe: &'a dyn ProcessProbe,
        paths: RuntimePaths,
    ) -> Self {
        Self {
            tmux,
            process_probe,
            paths,
        }
    }

    /// Returns the paths this owner is authorized to create or inspect.
    #[must_use]
    pub fn paths(&self) -> &RuntimePaths {
        &self.paths
    }

    /// Creates the private tmux server and starts one native provider command.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime directory already exists, tmux reports a
    /// live server, launch validation fails, or tmux cannot create the server.
    pub fn start(&self, launch: &NativeLaunch) -> Result<(), RuntimeError> {
        launch.validate()?;
        if self.paths.directory.exists() {
            return Err(RuntimeError::RuntimeAlreadyOwned(
                self.paths.directory.clone(),
            ));
        }
        create_private_runtime_directory(&self.paths.directory)?;
        write_tmux_config(&self.paths.config)?;

        let mut arguments = vec![
            OsString::from("new-session"),
            OsString::from("-d"),
            OsString::from("-s"),
            OsString::from(&self.paths.session_name),
            OsString::from("-n"),
            OsString::from(PROVIDER_WINDOW),
            OsString::from("-c"),
            launch.cwd.clone().into_os_string(),
        ];
        for (key, value) in &launch.environment {
            arguments.push(OsString::from("-e"));
            arguments.push(OsString::from(format!(
                "{}={}",
                key.to_string_lossy(),
                value.to_string_lossy()
            )));
        }
        arguments.extend(launch.program.iter().cloned());
        let response = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: Some(self.paths.config.clone()),
            arguments,
        })?;
        if !response.success {
            return Err(RuntimeError::TmuxRejected(trim_diagnostic(
                &response.stderr,
            )));
        }
        Ok(())
    }

    /// Returns the current single-pane evidence without inspecting any default tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the private tmux socket cannot be queried.
    pub fn probe(&self) -> Result<RuntimeProbe, RuntimeError> {
        let session_target = OsString::from(&self.paths.session_name);
        let exists = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: None,
            arguments: vec![
                OsString::from("has-session"),
                OsString::from("-t"),
                session_target.clone(),
            ],
        })?;
        if !exists.success {
            return Ok(RuntimeProbe::Missing);
        }

        let pane = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: None,
            arguments: vec![
                OsString::from("display-message"),
                OsString::from("-p"),
                OsString::from("-t"),
                OsString::from(format!("{}:0.0", self.paths.session_name)),
                OsString::from("#{pane_id}\t#{pane_pid}\t#{pane_current_path}"),
            ],
        })?;
        if !pane.success {
            return Ok(RuntimeProbe::Unknown {
                diagnostic: trim_diagnostic(&pane.stderr),
            });
        }
        let Some((pane_reference, process_id_text, cwd)) = parse_pane_facts(&pane.stdout) else {
            return Ok(RuntimeProbe::Unknown {
                diagnostic: "private tmux pane facts were malformed".to_owned(),
            });
        };
        let Ok(process_id) = process_id_text.parse::<u32>() else {
            return Ok(RuntimeProbe::Unknown {
                diagnostic: "private tmux pane PID was malformed".to_owned(),
            });
        };

        Ok(RuntimeProbe::Live {
            pane_id: pane_reference,
            pane_pid: process_id,
            cwd: PathBuf::from(cwd),
            process_birth: self.process_probe.process_birth(process_id),
        })
    }

    /// Builds the exact direct-attachment command for the private runtime socket.
    #[must_use]
    pub fn attach_command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.env_remove("TMUX");
        command
            .arg("-S")
            .arg(&self.paths.socket)
            .arg("attach-session")
            .arg("-t")
            .arg(&self.paths.session_name);
        command
    }

    /// Stops only the server at this runtime's recorded private socket.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux cannot be invoked or refuses the private
    /// server shutdown.
    pub fn park(&self) -> Result<(), RuntimeError> {
        let response = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: None,
            arguments: vec![OsString::from("kill-server")],
        })?;
        if response.success || is_missing_server(&response.stderr) {
            if self.paths.directory.exists() {
                fs::remove_dir_all(&self.paths.directory).map_err(|source| RuntimeError::Io {
                    path: self.paths.directory.clone(),
                    source,
                })?;
            }
            return Ok(());
        }
        Err(RuntimeError::TmuxRejected(trim_diagnostic(
            &response.stderr,
        )))
    }
}

fn response_from_output(
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<TmuxResponse, RuntimeError> {
    if stdout.len() > MAX_TMUX_OUTPUT_BYTES || stderr.len() > MAX_TMUX_OUTPUT_BYTES {
        return Err(RuntimeError::OutputTooLarge);
    }
    Ok(TmuxResponse {
        success: status.success(),
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr: String::from_utf8_lossy(stderr).into_owned(),
    })
}

fn create_private_runtime_directory(path: &Path) -> Result<(), RuntimeError> {
    let parent = path
        .parent()
        .ok_or_else(|| RuntimeError::InvalidRuntimePath(path.into()))?;
    fs::create_dir_all(parent).map_err(|source| RuntimeError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    set_mode(parent, 0o700)?;
    fs::create_dir(path).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    set_mode(path, 0o700)
}

fn write_tmux_config(path: &Path) -> Result<(), RuntimeError> {
    const CONFIG: &str = "set -g status off\nset -g mouse on\nset -g default-terminal tmux-256color\nset-environment -g COLORTERM truecolor\n";
    fs::write(path, CONFIG).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    set_mode(path, 0o600)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), RuntimeError> {
    Ok(())
}

fn parse_pane_facts(output: &str) -> Option<(String, &str, &str)> {
    let output = output.trim_end_matches(['\r', '\n']);
    let mut values = output.split('\t');
    let pane_reference = values.next()?.to_owned();
    let process_id_text = values.next()?;
    let cwd = values.next()?;
    if pane_reference.is_empty()
        || process_id_text.is_empty()
        || cwd.is_empty()
        || values.next().is_some()
    {
        return None;
    }
    Some((pane_reference, process_id_text, cwd))
}

fn trim_diagnostic(diagnostic: &str) -> String {
    diagnostic
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(256)
        .collect()
}

fn is_missing_server(diagnostic: &str) -> bool {
    diagnostic.contains("no server running") || diagnostic.contains("No such file")
}

/// Runtime-boundary failures. Diagnostics are deliberately bounded and never include provider output.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("native launch program is empty")]
    EmptyProgram,
    #[error("invalid working directory {0}")]
    InvalidWorkingDirectory(PathBuf),
    #[error("invalid private runtime path {0}")]
    InvalidRuntimePath(PathBuf),
    #[error("I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("tmux output exceeded the diagnostic limit")]
    OutputTooLarge,
    #[error("private runtime already exists at {0}")]
    RuntimeAlreadyOwned(PathBuf),
    #[error("tmux rejected the private runtime action: {0}")]
    TmuxRejected(String),
    #[error("could not launch tmux executable {executable:?}: {source}")]
    TmuxLaunch {
        executable: OsString,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque};

    use super::*;
    use crate::domain::RuntimeId;

    #[derive(Default)]
    struct FakeTmux {
        calls: RefCell<Vec<TmuxInvocation>>,
        responses: RefCell<VecDeque<TmuxResponse>>,
    }

    impl FakeTmux {
        fn with_responses(responses: impl IntoIterator<Item = TmuxResponse>) -> Self {
            Self {
                calls: RefCell::default(),
                responses: RefCell::new(responses.into_iter().collect()),
            }
        }
    }

    impl TmuxClient for FakeTmux {
        fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
            self.calls.borrow_mut().push(invocation.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| RuntimeError::TmuxRejected("missing fake response".to_owned()))
        }
    }

    #[derive(Default)]
    struct FakeProcessProbe;

    impl ProcessProbe for FakeProcessProbe {
        fn process_birth(&self, pid: u32) -> Option<String> {
            Some(format!("birth-{pid}"))
        }
    }

    fn successful() -> TmuxResponse {
        TmuxResponse {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn start_uses_only_a_private_socket_and_no_shell_command() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([successful()]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());
        let launch = NativeLaunch {
            cwd: temporary.path().to_path_buf(),
            program: vec![OsString::from("codex"), OsString::from("-C")],
            environment: BTreeMap::new(),
        };

        runtime.start(&launch).unwrap();

        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].socket, paths.socket);
        assert_eq!(calls[0].config, Some(paths.config));
        assert!(
            calls[0]
                .arguments
                .iter()
                .any(|argument| argument == "new-session")
        );
        assert!(
            calls[0]
                .arguments
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
    }

    #[test]
    fn malformed_pane_evidence_is_unknown_not_a_guess() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([
            successful(),
            TmuxResponse {
                success: true,
                stdout: "%1\tbad-pid\t/tmp\n".to_owned(),
                stderr: String::new(),
            },
        ]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.probe().unwrap(),
            RuntimeProbe::Unknown { .. }
        ));
    }

    #[test]
    fn park_tolerates_a_private_server_that_is_already_gone() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "no server running on socket".to_owned(),
        }]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        runtime.park().unwrap();
    }
}
