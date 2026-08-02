//! Bounded local and SSH execution for the host control protocol.
//!
//! Every invocation uses an argument vector, never a shell string. SSH target
//! aliases and remote executable paths are deliberately restricted so the
//! remote shell cannot reinterpret registered configuration as syntax.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::build_info::BuildInfo;
use crate::domain::{RuntimeId, WorkstreamId};
use crate::protocol::{
    HelloResponse, HostAction, HostRequest, HostResponse, MAX_FRAME_BYTES, MAX_SNAPSHOT_PAGES,
    ObserverStatus, OperationsResponse, ProjectDirectoriesResponse, RequestEnvelope,
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
        Self::run_with_timeout(&invocation, CONTROL_TIMEOUT)
    }
}

impl SystemCommandRunner {
    fn run_with_timeout(
        invocation: &CommandInvocation,
        timeout: Duration,
    ) -> Result<CommandResult, TransportError> {
        if invocation.stdin.len() > MAX_FRAME_BYTES {
            return Err(TransportError::RequestTooLarge);
        }
        let mut command = Command::new(&invocation.program);
        command
            .args(&invocation.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(TransportError::Launch)?;
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
        let status = wait_bounded(&mut child, timeout);
        let stdout = stdout_reader
            .join()
            .map_err(|_| TransportError::ReaderPanicked)??;
        let _stderr = stderr_reader
            .join()
            .map_err(|_| TransportError::ReaderPanicked)??;
        Ok(CommandResult {
            success: status?,
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
    /// Reads state-free remote build compatibility metadata through SSH.
    ///
    /// # Errors
    ///
    /// Returns an error when the executable is missing, does not expose the
    /// required probe, or emits malformed/oversized probe metadata.
    pub fn probe_ssh(&self, endpoint: &SshEndpoint) -> Result<BuildInfo, TransportError> {
        self.probe(&ssh_probe_invocation(endpoint))
    }

    /// Reads state-free local build compatibility metadata through the same
    /// subprocess seam used by SSH control tests.
    ///
    /// # Errors
    ///
    /// Returns an error under the same bounds as [`Self::probe_ssh`].
    pub fn probe_local(&self, endpoint: &LocalEndpoint) -> Result<BuildInfo, TransportError> {
        self.probe(&local_probe_invocation(endpoint))
    }

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
        Self::snapshot_pages(|cursor| {
            self.snapshot_page(&ssh_invocation(endpoint, &snapshot_request(cursor)?))
        })
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
        Self::snapshot_pages(|cursor| {
            self.snapshot_page(&local_invocation(endpoint, &snapshot_request(cursor)?))
        })
    }

    /// Lists unresolved remote creation operations through the same bounded
    /// stateful protocol as snapshots.
    ///
    /// # Errors
    ///
    /// Returns an error for transport/protocol failure, host rejection, or an
    /// unexpected response type.
    pub fn operations_ssh(
        &self,
        endpoint: &SshEndpoint,
    ) -> Result<OperationsResponse, TransportError> {
        self.operations(&ssh_invocation(endpoint, &operations_request()?))
    }

    /// Local-subprocess parity path for unresolved operation inspection.
    ///
    /// # Errors
    ///
    /// Returns an error under the same bounds as [`Self::operations_ssh`].
    pub fn operations_local(
        &self,
        endpoint: &LocalEndpoint,
    ) -> Result<OperationsResponse, TransportError> {
        self.operations(&local_invocation(endpoint, &operations_request()?))
    }

    /// Lists one bounded page of host-private project browser entries through
    /// SSH. The response contains no absolute checkout paths.
    ///
    /// # Errors
    ///
    /// Returns an error for transport/protocol failure, host rejection, or an
    /// unexpected response type.
    pub fn project_directories_ssh(
        &self,
        endpoint: &SshEndpoint,
        relative_path: &str,
    ) -> Result<ProjectDirectoriesResponse, TransportError> {
        self.project_directories(&ssh_invocation(
            endpoint,
            &project_directories_request(relative_path)?,
        ))
    }

    /// Local-subprocess parity path for bounded project browser listing.
    ///
    /// # Errors
    ///
    /// Returns an error under the same bounds as [`Self::project_directories_ssh`].
    pub fn project_directories_local(
        &self,
        endpoint: &LocalEndpoint,
        relative_path: &str,
    ) -> Result<ProjectDirectoriesResponse, TransportError> {
        self.project_directories(&local_invocation(
            endpoint,
            &project_directories_request(relative_path)?,
        ))
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

    /// Creates one remote Workstream through the same revision-guarded
    /// protocol used by the navigator. The returned ID is only accepted from
    /// the dedicated creation response, never parsed from diagnostics.
    ///
    /// # Errors
    ///
    /// Returns an error for transport/protocol failures, a remote rejection,
    /// or a response kind other than one exact created Workstream.
    pub fn create_ssh(
        &self,
        endpoint: &SshEndpoint,
        action: HostAction,
    ) -> Result<WorkstreamId, TransportError> {
        self.create(&ssh_invocation(endpoint, &apply_request(action)?))
    }

    /// Local-subprocess parity path for remote Workstream creation.
    ///
    /// # Errors
    ///
    /// Returns an error under the same bounds as [`Self::create_ssh`].
    pub fn create_local(
        &self,
        endpoint: &LocalEndpoint,
        action: HostAction,
    ) -> Result<WorkstreamId, TransportError> {
        self.create(&local_invocation(endpoint, &apply_request(action)?))
    }

    fn hello(&self, invocation: &CommandInvocation) -> Result<HelloResponse, TransportError> {
        match self.request(invocation)? {
            HostResponse::Hello(response) => Ok(response),
            HostResponse::Rejected { diagnostic } => Err(TransportError::Rejected(diagnostic)),
            _ => Err(TransportError::UnexpectedResponse),
        }
    }

    fn probe(&self, invocation: &CommandInvocation) -> Result<BuildInfo, TransportError> {
        let result = self.runner.run(invocation.clone())?;
        if !result.success {
            return Err(TransportError::ReleaseProbeUnavailable);
        }
        serde_json::from_slice(&result.stdout).map_err(|_| TransportError::ReleaseProbeMalformed)
    }

    fn snapshot_page(
        &self,
        invocation: &CommandInvocation,
    ) -> Result<SnapshotResponse, TransportError> {
        match self.request(invocation)? {
            HostResponse::Snapshot(response) => Ok(response),
            HostResponse::Rejected { diagnostic } => Err(TransportError::Rejected(diagnostic)),
            _ => Err(TransportError::UnexpectedResponse),
        }
    }

    fn snapshot_pages(
        mut fetch: impl FnMut(Option<u32>) -> Result<SnapshotResponse, TransportError>,
    ) -> Result<SnapshotResponse, TransportError> {
        let mut cursor = None;
        let mut workstreams = Vec::new();
        let mut identities = BTreeSet::new();
        let mut unresolved_operation_count = None;
        let mut observer_status = None;
        for _ in 0..MAX_SNAPSHOT_PAGES {
            let page = fetch(cursor)?;
            if let Some(expected) = unresolved_operation_count {
                if expected != page.unresolved_operation_count {
                    return Err(TransportError::InconsistentSnapshotPage);
                }
            } else {
                unresolved_operation_count = Some(page.unresolved_operation_count);
            }
            if let Some(expected) = observer_status {
                if expected != page.observer_status {
                    return Err(TransportError::InconsistentSnapshotPage);
                }
            } else {
                observer_status = Some(page.observer_status);
            }
            for workstream in page.workstreams {
                if !identities.insert(workstream.workstream_id) {
                    return Err(TransportError::InconsistentSnapshotPage);
                }
                workstreams.push(workstream);
            }
            let Some(next_cursor) = page.next_cursor else {
                return Ok(SnapshotResponse {
                    workstreams,
                    unresolved_operation_count: unresolved_operation_count.unwrap_or(0),
                    observer_status: observer_status.unwrap_or(ObserverStatus::NotInstalled),
                    next_cursor: None,
                });
            };
            if next_cursor <= cursor.unwrap_or(0) {
                return Err(TransportError::InconsistentSnapshotPage);
            }
            cursor = Some(next_cursor);
        }
        Err(TransportError::SnapshotPageLimit)
    }

    fn operations(
        &self,
        invocation: &CommandInvocation,
    ) -> Result<OperationsResponse, TransportError> {
        match self.request(invocation)? {
            HostResponse::Operations(response) => Ok(response),
            HostResponse::Rejected { diagnostic } => Err(TransportError::Rejected(diagnostic)),
            _ => Err(TransportError::UnexpectedResponse),
        }
    }

    fn project_directories(
        &self,
        invocation: &CommandInvocation,
    ) -> Result<ProjectDirectoriesResponse, TransportError> {
        match self.request(invocation)? {
            HostResponse::ProjectDirectories(response) => Ok(response),
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

    fn create(&self, invocation: &CommandInvocation) -> Result<WorkstreamId, TransportError> {
        match self.request(invocation)? {
            HostResponse::WorkstreamCreated { workstream_id, .. } => Ok(workstream_id),
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

fn snapshot_request(cursor: Option<u32>) -> Result<RequestEnvelope, TransportError> {
    let request = RequestEnvelope {
        version: crate::protocol::CURRENT_PROTOCOL_VERSION,
        request: HostRequest::Snapshot { cursor },
    };
    request.validate()?;
    Ok(request)
}

fn operations_request() -> Result<RequestEnvelope, TransportError> {
    let request = RequestEnvelope {
        version: crate::protocol::CURRENT_PROTOCOL_VERSION,
        request: HostRequest::Operations,
    };
    request.validate()?;
    Ok(request)
}

fn project_directories_request(relative_path: &str) -> Result<RequestEnvelope, TransportError> {
    let request = RequestEnvelope {
        version: crate::protocol::CURRENT_PROTOCOL_VERSION,
        request: HostRequest::ProjectDirectories {
            relative_path: relative_path.to_owned(),
        },
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
        // The provider terminal is the SSH stdout stream. Keep SSH transport
        // diagnostics out of that terminal; the navigator independently
        // observes the bounded host state after an attachment ends.
        .stderr(Stdio::null())
        .status()
        .map_err(TransportError::Launch)?;
    if status.success() {
        Ok(())
    } else {
        Err(TransportError::InteractiveAttachmentFailed)
    }
}

/// Opens the remote native Codex hook-review surface through the current
/// terminal. It has no control payload: after SSH connects, the only visible
/// bytes are the provider's own terminal UI.
///
/// # Errors
///
/// Returns an error when the fixed interactive SSH review command cannot run.
pub fn review_observer_ssh(endpoint: &SshEndpoint) -> Result<(), TransportError> {
    let status = Command::new("ssh")
        .args(ssh_observer_review_arguments(endpoint))
        .stderr(Stdio::null())
        .status()
        .map_err(TransportError::Launch)?;
    if status.success() {
        Ok(())
    } else {
        Err(TransportError::InteractiveObserverReviewFailed)
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

fn ssh_probe_invocation(endpoint: &SshEndpoint) -> CommandInvocation {
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
            OsString::from("_probe"),
        ],
        stdin: Vec::new(),
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

fn local_probe_invocation(endpoint: &LocalEndpoint) -> CommandInvocation {
    CommandInvocation {
        program: endpoint.executable.clone().into_os_string(),
        arguments: vec![OsString::from("_probe")],
        stdin: Vec::new(),
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

fn ssh_observer_review_arguments(endpoint: &SshEndpoint) -> Vec<OsString> {
    vec![
        OsString::from("-tt"),
        OsString::from("-o"),
        OsString::from("BatchMode=yes"),
        OsString::from("-o"),
        OsString::from("ConnectTimeout=8"),
        OsString::from(endpoint.destination.as_str()),
        OsString::from(endpoint.executable.as_str()),
        OsString::from("_remote_observer_review"),
    ]
}

fn wait_bounded(child: &mut Child, timeout: Duration) -> Result<bool, TransportError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(TransportError::Wait)? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            terminate_process_group(child)?;
            return Err(TransportError::TimedOut);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child) -> Result<(), TransportError> {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };

    let process_group = i32::try_from(child.id()).map_err(|_| TransportError::InvalidPid)?;
    if killpg(Pid::from_raw(process_group), Signal::SIGKILL).is_err()
        && child.try_wait().map_err(TransportError::Wait)?.is_none()
    {
        child.kill().map_err(TransportError::Kill)?;
    }
    child.wait().map_err(TransportError::Wait)?;
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut Child) -> Result<(), TransportError> {
    if child.try_wait().map_err(TransportError::Wait)?.is_none() {
        child.kill().map_err(TransportError::Kill)?;
    }
    child.wait().map_err(TransportError::Wait)?;
    Ok(())
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
    #[error("host command exposed an invalid process ID")]
    InvalidPid,
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
    #[error("remote executable does not expose the required state-free release probe")]
    ReleaseProbeUnavailable,
    #[error("remote executable returned malformed state-free release metadata")]
    ReleaseProbeMalformed,
    #[error("remote native tmux attachment failed")]
    InteractiveAttachmentFailed,
    #[error("remote native observer review failed")]
    InteractiveObserverReviewFailed,
    #[error("host returned an unexpected protocol response")]
    UnexpectedResponse,
    #[error("host returned inconsistent snapshot pages")]
    InconsistentSnapshotPage,
    #[error("host snapshot exceeded the bounded page count")]
    SnapshotPageLimit,
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
    fn ssh_release_probe_uses_fixed_arguments_and_no_stateful_input() {
        let endpoint = SshEndpoint::new(
            SshDestination::parse("snap").unwrap(),
            RemoteExecutable::parse("/home/bryan/.local/bin/wsnav").unwrap(),
        );

        let invocation = ssh_probe_invocation(&endpoint);

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
                "_probe",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>(),
        );
        assert!(invocation.stdin.is_empty());
    }

    #[test]
    fn unsafe_remote_values_cannot_reach_the_ssh_command() {
        assert!(SshDestination::parse("snap; whoami").is_err());
        assert!(RemoteExecutable::parse(STANDARD_REMOTE_EXECUTABLE).is_ok());
        assert!(RemoteExecutable::parse("~/bin/wsnav").is_err());
        assert!(RemoteExecutable::parse("/tmp/wsnav $(id)").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn stalled_host_command_group_is_terminated_at_the_deadline() {
        let started = Instant::now();
        let result = SystemCommandRunner::run_with_timeout(
            &CommandInvocation {
                program: "sh".into(),
                arguments: vec!["-c".into(), "sleep 30 & wait".into()],
                stdin: Vec::new(),
            },
            Duration::from_millis(100),
        );

        assert!(matches!(result, Err(TransportError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(2));
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
    fn snapshot_pages_are_assembled_in_cursor_order() {
        let first_id = WorkstreamId::new();
        let second_id = WorkstreamId::new();
        let mut pages = std::collections::VecDeque::from([
            SnapshotResponse {
                workstreams: vec![snapshot_workstream(first_id)],
                unresolved_operation_count: 1,
                observer_status: ObserverStatus::NotInstalled,
                next_cursor: Some(1),
            },
            SnapshotResponse {
                workstreams: vec![snapshot_workstream(second_id)],
                unresolved_operation_count: 1,
                observer_status: ObserverStatus::NotInstalled,
                next_cursor: None,
            },
        ]);
        let mut cursors = Vec::new();

        let snapshot = HostClient::<RecordingRunner>::snapshot_pages(|cursor| {
            cursors.push(cursor);
            pages.pop_front().ok_or(TransportError::UnexpectedResponse)
        })
        .unwrap();

        assert_eq!(cursors, vec![None, Some(1)]);
        assert_eq!(
            snapshot
                .workstreams
                .iter()
                .map(|workstream| workstream.workstream_id)
                .collect::<Vec<_>>(),
            vec![first_id, second_id]
        );
        assert_eq!(snapshot.next_cursor, None);
        assert_eq!(snapshot.unresolved_operation_count, 1);
    }

    #[test]
    fn snapshot_pages_reject_replayed_workstream_identity() {
        let workstream_id = WorkstreamId::new();
        let mut pages = std::collections::VecDeque::from([
            SnapshotResponse {
                workstreams: vec![snapshot_workstream(workstream_id)],
                unresolved_operation_count: 0,
                observer_status: ObserverStatus::NotInstalled,
                next_cursor: Some(1),
            },
            SnapshotResponse {
                workstreams: vec![snapshot_workstream(workstream_id)],
                unresolved_operation_count: 0,
                observer_status: ObserverStatus::NotInstalled,
                next_cursor: None,
            },
        ]);

        assert!(matches!(
            HostClient::<RecordingRunner>::snapshot_pages(|_| {
                pages.pop_front().ok_or(TransportError::UnexpectedResponse)
            }),
            Err(TransportError::InconsistentSnapshotPage)
        ));
    }

    #[test]
    fn creation_accepts_only_the_dedicated_workstream_response() {
        let workstream_id = WorkstreamId::new();
        let runner = RecordingRunner {
            response: ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::WorkstreamCreated {
                    workstream_id,
                    revision: 1,
                },
            },
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let endpoint = SshEndpoint::new(
            SshDestination::parse("snap").unwrap(),
            RemoteExecutable::parse("/home/bryan/.local/bin/wsnav").unwrap(),
        );

        assert_eq!(
            HostClient::new(runner)
                .create_ssh(
                    &endpoint,
                    HostAction::NewWorkstream {
                        source_workstream_id: WorkstreamId::new(),
                        expected_revision: 1,
                        request_key: "request-key".to_owned(),
                    },
                )
                .unwrap(),
            workstream_id
        );
    }

    fn snapshot_workstream(workstream_id: WorkstreamId) -> crate::protocol::SnapshotWorkstream {
        crate::protocol::SnapshotWorkstream {
            workstream_id,
            location_id: crate::domain::LocationId::new(),
            project_display_name: "project".to_owned(),
            repository_fingerprint: None,
            remote_identity_display: None,
            display_name: "thread".to_owned(),
            runtime_id: None,
            runtime_status: crate::domain::RuntimeStatus::Idle,
            lifecycle: crate::domain::WorkstreamLifecycle::Open,
            archived: false,
            result_ready: false,
            recovery_required: false,
            attention_revision: None,
            activity_sequence: 0,
            last_activity_at_millis: None,
            revision: 1,
        }
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

    #[test]
    fn interactive_observer_review_is_a_fixed_tty_command_with_no_control_stream() {
        let endpoint = SshEndpoint::new(
            SshDestination::parse("snap").unwrap(),
            RemoteExecutable::parse("/home/bryan/.local/bin/wsnav").unwrap(),
        );

        let arguments = ssh_observer_review_arguments(&endpoint);

        assert_eq!(arguments[0], OsStr::new("-tt"));
        assert_eq!(arguments[5], OsStr::new("snap"));
        assert_eq!(arguments[6], OsStr::new("/home/bryan/.local/bin/wsnav"));
        assert_eq!(arguments[7], OsStr::new("_remote_observer_review"));
        assert!(arguments.iter().all(|argument| argument != "sh"));
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
