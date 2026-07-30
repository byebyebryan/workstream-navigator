//! Bounded local and SSH execution for the host control protocol.
//!
//! Every invocation uses an argument vector, never a shell string. SSH target
//! aliases and remote executable paths are deliberately restricted so the
//! remote shell cannot reinterpret registered configuration as syntax.

use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::domain::RuntimeId;
use crate::protocol::{
    HelloResponse, HostAction, HostRequest, HostResponse, MAX_FRAME_BYTES, RequestEnvelope,
    ResponseEnvelope, SnapshotResponse,
};

const MAX_STDERR_BYTES: usize = 4096;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_SSH_TARGET_BYTES: usize = 255;
const MAX_REMOTE_EXECUTABLE_BYTES: usize = 1024;

/// The conventional per-user installation used by the ordinary remote-host
/// registration flow. It is a fixed literal, expanded only by the remote
/// login shell; callers cannot supply arbitrary shell syntax through it.
pub const STANDARD_REMOTE_EXECUTABLE: &str = "~/.local/bin/wsnav";

/// A validated SSH destination supplied during explicit host registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshDestination(String);

impl SshDestination {
    /// Parses the intentionally narrow SSH destination grammar used by V1.
    ///
    /// It permits host aliases plus optional `user@`, port, and bracket-free
    /// address syntax, but never whitespace or shell metacharacters.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or unsafe target.
    pub fn parse(value: &str) -> Result<Self, TransportError> {
        if value.is_empty()
            || value.len() > MAX_SSH_TARGET_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-@:".contains(&byte))
        {
            return Err(TransportError::UnsafeSshDestination);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated remote executable path used by a fixed SSH command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteExecutable(String);

impl RemoteExecutable {
    /// Parses either the fixed standard user-local executable or an explicit
    /// absolute path. Arbitrary relative paths and shell expansion are refused.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, non-absolute, oversized, or unsafe paths.
    pub fn parse(value: &str) -> Result<Self, TransportError> {
        let standard_user_local = value == STANDARD_REMOTE_EXECUTABLE;
        let safe_absolute_path = value.starts_with('/')
            && value.len() <= MAX_REMOTE_EXECUTABLE_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte));
        if !standard_user_local && !safe_absolute_path {
            return Err(TransportError::UnsafeRemoteExecutable);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One exact remote host endpoint registered by the local client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshEndpoint {
    pub destination: SshDestination,
    pub executable: RemoteExecutable,
}

impl SshEndpoint {
    #[must_use]
    pub fn new(destination: SshDestination, executable: RemoteExecutable) -> Self {
        Self {
            destination,
            executable,
        }
    }
}

/// One local endpoint used to prove the same JSON service contract without
/// involving the user's default tmux server or a real remote machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalEndpoint {
    pub executable: PathBuf,
    pub state_root: PathBuf,
}

/// Injectable process runner used by deterministic transport tests.
pub trait CommandRunner {
    /// Executes one fixed command with a bounded stdin frame.
    ///
    /// # Errors
    ///
    /// Returns an error when the command cannot be launched, completed, or
    /// represented within the bounded transport contract.
    fn run(&self, invocation: CommandInvocation) -> Result<CommandResult, TransportError>;
}

/// One shell-free process invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandInvocation {
    pub program: OsString,
    pub arguments: Vec<OsString>,
    pub stdin: Vec<u8>,
}

/// Bounded process output. Stderr is intentionally not retained in errors:
/// remote tools can include paths or unrelated local diagnostics there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: Vec<u8>,
}

/// The production bounded process runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, invocation: CommandInvocation) -> Result<CommandResult, TransportError> {
        if invocation.stdin.len() > MAX_FRAME_BYTES {
            return Err(TransportError::RequestTooLarge);
        }
        let mut child = Command::new(&invocation.program)
            .args(&invocation.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(TransportError::Launch)?;
        let stdout = child.stdout.take().ok_or(TransportError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(TransportError::MissingPipe)?;
        let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_FRAME_BYTES));
        let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&invocation.stdin)
                .map_err(TransportError::Write)?;
        } else {
            return Err(TransportError::MissingPipe);
        }
        let status = wait_bounded(&mut child)?;
        let stdout = stdout_reader
            .join()
            .map_err(|_| TransportError::ReaderPanicked)??;
        let _stderr = stderr_reader
            .join()
            .map_err(|_| TransportError::ReaderPanicked)??;
        Ok(CommandResult {
            success: status,
            stdout,
        })
    }
}

/// One client that speaks the same protocol through either local subprocesses
/// or SSH. It deliberately has no mutation authority of its own: callers must
/// compare `hello` evidence with their registered host record first.
#[derive(Clone, Debug)]
pub struct HostClient<R> {
    runner: R,
}

impl<R> HostClient<R> {
    #[must_use]
    pub fn new(runner: R) -> Self {
        Self { runner }
    }
}

impl<R: CommandRunner> HostClient<R> {
    /// Requests one remote host handshake.
    ///
    /// # Errors
    ///
    /// Returns an error for launch/transport/protocol failures or a response
    /// kind different from the requested handshake.
    pub fn hello_ssh(
        &self,
        endpoint: &SshEndpoint,
        client_alias: &str,
    ) -> Result<HelloResponse, TransportError> {
        self.hello(&ssh_invocation(endpoint, &hello_request(client_alias)?))
    }

    /// Requests a local subprocess handshake using the exact remote service
    /// framing. This is the deterministic parity adapter for D3.
    ///
    /// # Errors
    ///
    /// Returns an error for launch/transport/protocol failures or a response
    /// kind different from the requested handshake.
    pub fn hello_local(
        &self,
        endpoint: &LocalEndpoint,
        client_alias: &str,
    ) -> Result<HelloResponse, TransportError> {
        self.hello(&local_invocation(endpoint, &hello_request(client_alias)?))
    }

    /// Retrieves one bounded remote snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for launch/transport/protocol failures or a response
    /// kind different from the requested snapshot.
    pub fn snapshot_ssh(&self, endpoint: &SshEndpoint) -> Result<SnapshotResponse, TransportError> {
        self.snapshot(&ssh_invocation(endpoint, &snapshot_request()?))
    }

    /// Retrieves one bounded local subprocess snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for launch/transport/protocol failures or a response
    /// kind different from the requested snapshot.
    pub fn snapshot_local(
        &self,
        endpoint: &LocalEndpoint,
    ) -> Result<SnapshotResponse, TransportError> {
        self.snapshot(&local_invocation(endpoint, &snapshot_request()?))
    }

    /// Applies one explicitly revision-guarded action through SSH.
    ///
    /// # Errors
    ///
    /// Returns an error for launch/transport/protocol failures, a remote
    /// rejection, or a response kind different from the requested action.
    pub fn apply_ssh(
        &self,
        endpoint: &SshEndpoint,
        action: HostAction,
    ) -> Result<i64, TransportError> {
        self.apply(&ssh_invocation(endpoint, &apply_request(action)?))
    }

    /// Applies one explicitly revision-guarded action through the local
    /// subprocess parity endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for launch/transport/protocol failures, a remote
    /// rejection, or a response kind different from the requested action.
    pub fn apply_local(
        &self,
        endpoint: &LocalEndpoint,
        action: HostAction,
    ) -> Result<i64, TransportError> {
        self.apply(&local_invocation(endpoint, &apply_request(action)?))
    }

    fn hello(&self, invocation: &CommandInvocation) -> Result<HelloResponse, TransportError> {
        match self.request(invocation)? {
            HostResponse::Hello(response) => Ok(response),
            HostResponse::Rejected { diagnostic } => Err(TransportError::Rejected(diagnostic)),
            _ => Err(TransportError::UnexpectedResponse),
        }
    }

    fn snapshot(&self, invocation: &CommandInvocation) -> Result<SnapshotResponse, TransportError> {
        match self.request(invocation)? {
            HostResponse::Snapshot(response) => Ok(response),
            HostResponse::Rejected { diagnostic } => Err(TransportError::Rejected(diagnostic)),
            _ => Err(TransportError::UnexpectedResponse),
        }
    }

    fn apply(&self, invocation: &CommandInvocation) -> Result<i64, TransportError> {
        match self.request(invocation)? {
            HostResponse::Applied { revision } => Ok(revision),
            HostResponse::Rejected { diagnostic } => Err(TransportError::Rejected(diagnostic)),
            _ => Err(TransportError::UnexpectedResponse),
        }
    }

    fn request(&self, invocation: &CommandInvocation) -> Result<HostResponse, TransportError> {
        let result = self.runner.run(invocation.clone())?;
        if !result.success {
            return Err(TransportError::RemoteCommandFailed);
        }
        let response = ResponseEnvelope::decode(&result.stdout)?;
        Ok(response.response)
    }
}

fn hello_request(client_alias: &str) -> Result<RequestEnvelope, TransportError> {
    let request = RequestEnvelope {
        version: crate::protocol::CURRENT_PROTOCOL_VERSION,
        request: HostRequest::Hello {
            client_alias: client_alias.to_owned(),
        },
    };
    request.validate()?;
    Ok(request)
}

fn snapshot_request() -> Result<RequestEnvelope, TransportError> {
    let request = RequestEnvelope {
        version: crate::protocol::CURRENT_PROTOCOL_VERSION,
        request: HostRequest::Snapshot,
    };
    request.validate()?;
    Ok(request)
}

fn apply_request(action: HostAction) -> Result<RequestEnvelope, TransportError> {
    let request = RequestEnvelope {
        version: crate::protocol::CURRENT_PROTOCOL_VERSION,
        request: HostRequest::Apply { action },
    };
    request.validate()?;
    Ok(request)
}

/// Attaches the current terminal to exactly one remote Runtime through an
/// interactive SSH stream. This bypasses the JSON control runner entirely:
/// the provider terminal is the only bytes that travel after connection.
///
/// # Errors
///
/// Returns an error when SSH cannot launch or the remote native attachment
/// exits unsuccessfully.
pub fn attach_ssh(endpoint: &SshEndpoint, runtime_id: RuntimeId) -> Result<(), TransportError> {
    let status = Command::new("ssh")
        .args(ssh_attach_arguments(endpoint, runtime_id))
        .status()
        .map_err(TransportError::Launch)?;
    if status.success() {
        Ok(())
    } else {
        Err(TransportError::RemoteCommandFailed)
    }
}

fn ssh_invocation(endpoint: &SshEndpoint, request: &RequestEnvelope) -> CommandInvocation {
    CommandInvocation {
        program: OsString::from("ssh"),
        arguments: vec![
            OsString::from("-T"),
            OsString::from("-o"),
            OsString::from("BatchMode=yes"),
            OsString::from("-o"),
            OsString::from("ConnectTimeout=8"),
            OsString::from(endpoint.destination.as_str()),
            OsString::from(endpoint.executable.as_str()),
            OsString::from("_remote"),
        ],
        stdin: request
            .encode()
            .expect("validated fixed protocol requests always encode"),
    }
}

fn local_invocation(endpoint: &LocalEndpoint, request: &RequestEnvelope) -> CommandInvocation {
    CommandInvocation {
        program: endpoint.executable.clone().into_os_string(),
        arguments: vec![
            OsString::from("--state-root"),
            endpoint.state_root.clone().into_os_string(),
            OsString::from("_remote"),
        ],
        stdin: request
            .encode()
            .expect("validated fixed protocol requests always encode"),
    }
}

fn ssh_attach_arguments(endpoint: &SshEndpoint, runtime_id: RuntimeId) -> Vec<OsString> {
    vec![
        OsString::from("-tt"),
        OsString::from("-o"),
        OsString::from("BatchMode=yes"),
        OsString::from("-o"),
        OsString::from("ConnectTimeout=8"),
        OsString::from(endpoint.destination.as_str()),
        OsString::from(endpoint.executable.as_str()),
        OsString::from("_attach"),
        OsString::from(runtime_id.to_string()),
    ]
}

fn wait_bounded(child: &mut Child) -> Result<bool, TransportError> {
    let deadline = Instant::now() + CONTROL_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(TransportError::Wait)? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            child.kill().map_err(TransportError::Kill)?;
            let status = child.wait().map_err(TransportError::Wait)?;
            if status.success() {
                return Ok(true);
            }
            return Err(TransportError::TimedOut);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn read_bounded(mut reader: impl Read, maximum: usize) -> Result<Vec<u8>, TransportError> {
    let mut output = Vec::with_capacity(maximum.min(4096));
    let mut buffer = [0_u8; 4096];
    let mut oversized = false;
    loop {
        let read = reader.read(&mut buffer).map_err(TransportError::Read)?;
        if read == 0 {
            break;
        }
        let available = maximum.saturating_sub(output.len());
        let stored = available.min(read);
        output.extend_from_slice(&buffer[..stored]);
        oversized |= stored != read;
    }
    if oversized {
        return Err(TransportError::OutputTooLarge);
    }
    Ok(output)
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("SSH destination must be a bounded host alias or address")]
    UnsafeSshDestination,
    #[error("remote executable must be a bounded absolute safe path")]
    UnsafeRemoteExecutable,
    #[error("protocol request exceeds its maximum size")]
    RequestTooLarge,
    #[error("could not launch host command")]
    Launch(std::io::Error),
    #[error("could not write host request")]
    Write(std::io::Error),
    #[error("could not read host response")]
    Read(std::io::Error),
    #[error("could not wait for host response")]
    Wait(std::io::Error),
    #[error("could not stop timed out host command")]
    Kill(std::io::Error),
    #[error("host command did not expose an expected pipe")]
    MissingPipe,
    #[error("host output reader failed")]
    ReaderPanicked,
    #[error("host command timed out")]
    TimedOut,
    #[error("host response exceeded its bounded transport size")]
    OutputTooLarge,
    #[error("host command failed without a usable protocol response")]
    RemoteCommandFailed,
    #[error("host returned an unexpected protocol response")]
    UnexpectedResponse,
    #[error("host rejected the request: {0}")]
    Rejected(String),
    #[error(transparent)]
    Protocol(#[from] crate::protocol::ProtocolError),
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::{
        domain::HostId,
        protocol::{CURRENT_PROTOCOL_VERSION, Capabilities},
    };

    #[derive(Clone, Debug)]
    struct RecordingRunner {
        response: ResponseEnvelope,
        calls: std::sync::Arc<std::sync::Mutex<Vec<CommandInvocation>>>,
    }

    impl CommandRunner for RecordingRunner {
        fn run(&self, invocation: CommandInvocation) -> Result<CommandResult, TransportError> {
            self.calls.lock().unwrap().push(invocation);
            Ok(CommandResult {
                success: true,
                stdout: self.response.encode().unwrap(),
            })
        }
    }

    #[test]
    fn ssh_control_uses_fixed_arguments_and_never_a_shell_string() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner = RecordingRunner {
            response: hello_response(),
            calls: calls.clone(),
        };
        let endpoint = SshEndpoint::new(
            SshDestination::parse("snap").unwrap(),
            RemoteExecutable::parse("/home/bryan/.local/bin/wsnav").unwrap(),
        );

        HostClient::new(runner)
            .hello_ssh(&endpoint, "workstation")
            .unwrap();

        let invocation = calls.lock().unwrap().pop().unwrap();
        assert_eq!(invocation.program, OsStr::new("ssh"));
        assert_eq!(
            invocation.arguments,
            vec![
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                "snap",
                "/home/bryan/.local/bin/wsnav",
                "_remote",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
        );
        assert!(RequestEnvelope::decode(&invocation.stdin).is_ok());
    }

    #[test]
    fn unsafe_remote_values_cannot_reach_the_ssh_command() {
        assert!(SshDestination::parse("snap; whoami").is_err());
        assert!(RemoteExecutable::parse(STANDARD_REMOTE_EXECUTABLE).is_ok());
        assert!(RemoteExecutable::parse("~/bin/wsnav").is_err());
        assert!(RemoteExecutable::parse("/tmp/wsnav $(id)").is_err());
    }

    #[test]
    fn rejected_frames_are_not_mistaken_for_a_snapshot() {
        let runner = RecordingRunner {
            response: ResponseEnvelope::rejected("host state is unavailable".to_owned()).unwrap(),
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let endpoint = SshEndpoint::new(
            SshDestination::parse("snap").unwrap(),
            RemoteExecutable::parse("/home/bryan/.local/bin/wsnav").unwrap(),
        );

        assert!(matches!(
            HostClient::new(runner).snapshot_ssh(&endpoint),
            Err(TransportError::Rejected(_))
        ));
    }

    #[test]
    fn interactive_attachment_is_a_fixed_tty_command_with_no_control_stream() {
        let endpoint = SshEndpoint::new(
            SshDestination::parse("snap").unwrap(),
            RemoteExecutable::parse("/home/bryan/.local/bin/wsnav").unwrap(),
        );
        let runtime_id = RuntimeId::new();

        let arguments = ssh_attach_arguments(&endpoint, runtime_id);

        assert_eq!(arguments[0], OsStr::new("-tt"));
        assert_eq!(arguments[5], OsStr::new("snap"));
        assert_eq!(arguments[6], OsStr::new("/home/bryan/.local/bin/wsnav"));
        assert_eq!(arguments[7], OsStr::new("_attach"));
        assert_eq!(arguments[8], OsStr::new(&runtime_id.to_string()));
    }

    fn hello_response() -> ResponseEnvelope {
        ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Hello(HelloResponse {
                host_id: HostId::new(),
                registry_generation: "generation".to_owned(),
                wsnav_version: "0.1.0".to_owned(),
                capabilities: Capabilities {
                    codex: true,
                    git: true,
                    tmux: true,
                },
            }),
        }
    }
}
