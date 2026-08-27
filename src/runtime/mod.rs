//! Private tmux runtime ownership and bounded process probes.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    domain::RuntimeId,
    private_tmux::{COPY_MODE_SCROLL_BINDINGS, TERMINAL_CAPABILITY_CONFIG},
    process::{BoundedProcessError, output_bounded},
};

const RUNTIME_DIRECTORY: &str = "run";
const PROVIDER_WINDOW: &str = "provider";
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;
const LAUNCH_BARRIER_FILE: &str = "launch.ready";
const LAUNCH_BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
const LAUNCH_BARRIER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PROVIDER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PROCESS_GROUP_STAT_READ_ATTEMPTS: usize = 3;
const PROCESS_GROUP_STAT_RETRY_DELAY: Duration = Duration::from_millis(1);
const RUNTIME_TMUX_CONFIG_PREFIX: &str = concat!("set -g status off\n", "set -g mouse on\n",);

fn runtime_tmux_config() -> String {
    let copy_mode_scroll_config = crate::private_tmux::copy_mode_scroll_config();
    [
        RUNTIME_TMUX_CONFIG_PREFIX,
        TERMINAL_CAPABILITY_CONFIG,
        &copy_mode_scroll_config,
    ]
    .concat()
}

/// A private runtime server's owned paths and stable tmux session name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimePaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
    pub session_name: String,
}

impl RuntimePaths {
    /// Derives the only current private tmux path set allowed for a runtime.
    ///
    /// The complete opaque runtime identifier is used in every externally
    /// owned path and session name. An eight-character prefix is insufficient
    /// for an ownership boundary because two valid UUIDs can share it.
    #[must_use]
    pub fn for_runtime(state_root: &Path, runtime_id: RuntimeId) -> Self {
        Self::for_identifier(state_root, runtime_id.to_string())
    }

    /// Reconstructs the exact private paths for one persisted runtime record.
    ///
    /// Only the current full-ID session format and the former short-ID format
    /// are accepted. The latter is read-only compatibility for an existing
    /// runtime; every newly reserved runtime uses [`Self::for_runtime`].
    ///
    /// # Errors
    ///
    /// Returns an error when persisted session metadata does not prove which
    /// private tmux path set this Runtime owns.
    pub fn for_record(
        state_root: &Path,
        runtime_id: RuntimeId,
        recorded_session: &str,
    ) -> Result<Self, RuntimeError> {
        let current = Self::for_runtime(state_root, runtime_id);
        if recorded_session == current.session_name {
            return Ok(current);
        }
        let legacy = Self::for_identifier(state_root, runtime_id.short());
        if recorded_session == legacy.session_name {
            return Ok(legacy);
        }
        Err(RuntimeError::RuntimeSessionMismatch)
    }

    fn for_identifier(state_root: &Path, identifier: impl AsRef<str>) -> Self {
        let identifier = identifier.as_ref();
        let directory = state_root
            .join(RUNTIME_DIRECTORY)
            .join(format!("runtime-{identifier}"));
        Self {
            socket: directory.join("tmux.sock"),
            config: directory.join("tmux.conf"),
            session_name: format!("wsnav-{identifier}"),
            directory,
        }
    }

    fn launch_barrier(&self) -> PathBuf {
        self.directory.join(LAUNCH_BARRIER_FILE)
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

/// Writes fixed, private bootstrap artifacts after a Runtime directory has
/// been created but before tmux can start its native program. Bootstrap code
/// receives the exact final Runtime paths and cannot alter the launch command,
/// attach a client, or use the default tmux server.
pub(crate) trait RuntimeStartup {
    fn prepare(&self, paths: &RuntimePaths) -> Result<(), RuntimeError>;
}

struct NoRuntimeStartup;

impl RuntimeStartup for NoRuntimeStartup {
    fn prepare(&self, _paths: &RuntimePaths) -> Result<(), RuntimeError> {
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
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(RuntimeError::from_bounded_tmux)?;
        response_from_output(output.status, &output.stdout, &output.stderr)
    }
}

/// Errors from an exact process-identity probe.
#[derive(Debug, Error)]
pub enum ProcessProbeError {
    #[error("process metadata is inaccessible")]
    Inaccessible,
    #[error("process metadata read failed: {0}")]
    Io(String),
    #[error("process metadata is malformed")]
    Malformed,
}

/// Platform process metadata used only to corroborate a private tmux pane.
pub trait ProcessProbe {
    /// Returns a stable process-birth token for a live process.
    fn process_birth(&self, pid: u32) -> Option<String>;

    /// Returns a stable process-birth token while distinguishing an absent
    /// process from an ambiguous metadata probe. Existing probe fakes and
    /// callers may implement only [`Self::process_birth`]; the default adapter
    /// preserves that API while the exact shutdown path uses this richer form.
    ///
    /// # Errors
    ///
    /// Returns an error when process metadata cannot be read or parsed with
    /// enough certainty to authorize an external signal.
    fn process_birth_checked(&self, pid: u32) -> Result<Option<String>, ProcessProbeError> {
        Ok(self.process_birth(pid))
    }
}

/// Exact process-group metadata corroborated from the host process table.
///
/// The group ID is the only value used for a group signal. The session ID is
/// retained as an additional ownership fact so a same-numbered group from a
/// different terminal session is never accepted as equivalent evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessGroupInfo {
    pub process_group_id: u32,
    pub session_id: u32,
}

/// Injectable process-group evidence used by native Runtime cleanup.
pub trait ProcessGroupProbe {
    /// Reads one process's process-group and session identity.
    ///
    /// `None` means the process is absent. Ambiguous or inaccessible metadata
    /// is an error and must never authorize a group signal.
    ///
    /// # Errors
    ///
    /// Returns an error when process metadata cannot be read or parsed with
    /// enough certainty to authorize a group signal.
    fn process_group_checked(
        &self,
        pid: u32,
    ) -> Result<Option<ProcessGroupInfo>, ProcessProbeError>;

    /// Lists every currently visible member of one process group.
    ///
    /// Implementations must fail closed when the process table cannot be
    /// enumerated with enough certainty to prove that the group is empty.
    ///
    /// # Errors
    ///
    /// Returns an error when process-group membership cannot be enumerated
    /// with enough certainty to authorize cleanup.
    fn process_group_members_checked(
        &self,
        group: &ProcessGroupInfo,
    ) -> Result<Vec<u32>, ProcessProbeError>;

    /// Lists members by numeric process-group ID when the original leader is
    /// already absent and therefore no session token can be read from it.
    /// A non-empty result is evidence that cleanup cannot safely infer which
    /// historical group owns the surviving processes; callers must fail
    /// closed rather than signal this numeric ID.
    ///
    /// # Errors
    ///
    /// Returns an error when process-group membership cannot be enumerated
    /// with enough certainty to authorize cleanup.
    fn process_group_members_by_id_checked(
        &self,
        process_group_id: u32,
    ) -> Result<Vec<u32>, ProcessProbeError>;
}

/// Exact process-group ownership captured while the provider leader is live.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedProcessGroup {
    pub leader_pid: u32,
    pub leader_birth: String,
    pub process_group_id: u32,
    pub session_id: u32,
}

/// Signals supported by the exact owned-provider shutdown boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedProcessSignal {
    Term,
    Kill,
}

/// The result of bounded shutdown for one exact provider PID/birth pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedProcessTermination {
    /// The PID was already absent or had a different birth token. No signal
    /// was sent because the persisted identity was no longer exact.
    AlreadyGone,
    /// The exact process exited after SIGTERM.
    TerminatedByTerm,
    /// The exact process required SIGKILL and then exited.
    TerminatedByKill,
}

/// A narrow signal boundary so shutdown tests never touch ordinary processes.
pub trait ProcessSignaler {
    /// Sends one signal to the exact process identified by `pid` and its
    /// recorded birth token.
    ///
    /// On Linux the production implementation opens a pidfd after the
    /// identity check and sends through that stable descriptor. The birth
    /// token is checked again after opening the descriptor, so a PID reuse
    /// between the initial probe and the signal cannot redirect the signal to
    /// an unrelated process.
    ///
    /// # Errors
    ///
    /// Returns an error when the process is already gone or the signal cannot
    /// be delivered by the host boundary.
    fn signal(
        &self,
        pid: u32,
        expected_birth: &str,
        signal: OwnedProcessSignal,
    ) -> Result<(), ProcessSignalError>;
}

/// A narrow signal boundary for one previously-proven native process group.
pub trait ProcessGroupSignaler {
    /// Signals the exact process group. `allow_leader_gone` is only used for
    /// bounded KILL escalation after the original leader has exited while
    /// process-table evidence still proves that members retain the original
    /// group ID. Initial TERM always requires the leader identity to be live.
    ///
    /// # Errors
    ///
    /// Returns an error when the process-group identity is invalid, ownership
    /// cannot be corroborated, or the signal cannot be delivered safely.
    fn signal_group(
        &self,
        group: &OwnedProcessGroup,
        signal: OwnedProcessSignal,
        allow_leader_gone: bool,
    ) -> Result<(), ProcessSignalError>;
}

/// Errors returned by an injected or system process signaler.
#[derive(Debug, Error)]
pub enum ProcessSignalError {
    #[error("process already gone")]
    AlreadyGone,
    #[error("process signal failed: {0}")]
    Failed(String),
}

/// The host process-signal adapter. It never chooses a PID; callers must first
/// prove the persisted PID/birth identity through [`ProcessProbe`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProcessSignaler;

impl ProcessSignaler for SystemProcessSignaler {
    fn signal(
        &self,
        pid: u32,
        expected_birth: &str,
        signal: OwnedProcessSignal,
    ) -> Result<(), ProcessSignalError> {
        #[cfg(target_os = "linux")]
        {
            // pidfd_open binds this operation to the process instance rather
            // than a reusable numeric PID. The second birth check closes the
            // race between the caller's preflight probe and pidfd_open.
            let pid_number = pid;
            let pid = i32::try_from(pid)
                .map_err(|_| ProcessSignalError::Failed("PID is out of range".to_owned()))?;
            let pid = rustix::process::Pid::from_raw(pid)
                .ok_or_else(|| ProcessSignalError::Failed("PID is zero".to_owned()))?;
            let pidfd = match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
            {
                Ok(pidfd) => pidfd,
                Err(error) => {
                    return if error == rustix::io::Errno::SRCH {
                        Err(ProcessSignalError::AlreadyGone)
                    } else {
                        Err(ProcessSignalError::Failed(format!(
                            "pidfd_open failed: {error}"
                        )))
                    };
                }
            };
            match LinuxProcessProbe.process_birth_checked(pid_number) {
                Ok(Some(actual)) if actual == expected_birth => {}
                Ok(_) => return Err(ProcessSignalError::AlreadyGone),
                Err(error) => {
                    return Err(ProcessSignalError::Failed(format!(
                        "could not corroborate pidfd target: {error}"
                    )));
                }
            }
            let signal_kind = match signal {
                OwnedProcessSignal::Term => rustix::process::Signal::TERM,
                OwnedProcessSignal::Kill => rustix::process::Signal::KILL,
            };
            if let Err(error) = rustix::process::pidfd_send_signal(&pidfd, signal_kind) {
                if error == rustix::io::Errno::SRCH {
                    Err(ProcessSignalError::AlreadyGone)
                } else {
                    Err(ProcessSignalError::Failed(format!(
                        "pidfd_send_signal failed: {error}"
                    )))
                }
            } else {
                Ok(())
            }
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            use nix::{errno::Errno, sys::signal, unistd::Pid};

            let pid = i32::try_from(pid)
                .map_err(|_| ProcessSignalError::Failed("PID is out of range".to_owned()))?;
            let signal_kind = match signal {
                OwnedProcessSignal::Term => signal::Signal::SIGTERM,
                OwnedProcessSignal::Kill => signal::Signal::SIGKILL,
            };
            signal::kill(Pid::from_raw(pid), signal_kind).map_err(|error| {
                if error == Errno::ESRCH {
                    ProcessSignalError::AlreadyGone
                } else {
                    ProcessSignalError::Failed(error.to_string())
                }
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (pid, expected_birth, signal);
            Err(ProcessSignalError::Failed(
                "process signalling is unsupported on this platform".to_owned(),
            ))
        }
    }
}

/// The host process-group signal adapter. It reopens a pidfd for the original
/// leader and corroborates its birth plus group/session identity immediately
/// before each group signal. KILL escalation may proceed after that leader has
/// exited only when the caller has separately observed members retaining the
/// original group ID.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProcessGroupSignaler;

impl ProcessGroupSignaler for SystemProcessGroupSignaler {
    fn signal_group(
        &self,
        group: &OwnedProcessGroup,
        signal: OwnedProcessSignal,
        allow_leader_gone: bool,
    ) -> Result<(), ProcessSignalError> {
        if group.leader_pid == 0
            || group.process_group_id == 0
            || group.process_group_id != group.leader_pid
            || group.session_id == 0
            || group.leader_birth.is_empty()
        {
            return Err(ProcessSignalError::Failed(
                "owned process-group identity was invalid".to_owned(),
            ));
        }

        #[cfg(target_os = "linux")]
        {
            let leader_pid = i32::try_from(group.leader_pid)
                .map_err(|_| ProcessSignalError::Failed("PID is out of range".to_owned()))?;
            let leader_pid = rustix::process::Pid::from_raw(leader_pid)
                .ok_or_else(|| ProcessSignalError::Failed("PID is zero".to_owned()))?;
            let leader_pidfd =
                match rustix::process::pidfd_open(leader_pid, rustix::process::PidfdFlags::empty())
                {
                    Ok(pidfd) => Some(pidfd),
                    Err(error) if error == rustix::io::Errno::SRCH && allow_leader_gone => None,
                    Err(error) if error == rustix::io::Errno::SRCH => {
                        return Err(ProcessSignalError::AlreadyGone);
                    }
                    Err(error) => {
                        return Err(ProcessSignalError::Failed(format!(
                            "pidfd_open failed: {error}"
                        )));
                    }
                };

            let leader_stopped = leader_pidfd.as_ref().map_or(Ok(false), |pidfd| {
                stop_and_verify_linux_group_leader(pidfd, group, allow_leader_gone)
            })?;
            send_linux_group_signal(group, signal, leader_pidfd.as_ref(), leader_stopped)?;
            Ok(())
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            let process_group_id = i32::try_from(group.process_group_id).map_err(|_| {
                ProcessSignalError::Failed("process-group ID is out of range".to_owned())
            })?;
            let signal_kind = match signal {
                OwnedProcessSignal::Term => nix::sys::signal::Signal::SIGTERM,
                OwnedProcessSignal::Kill => nix::sys::signal::Signal::SIGKILL,
            };
            nix::sys::signal::killpg(nix::unistd::Pid::from_raw(process_group_id), signal_kind)
                .map_err(|error| {
                    if error == nix::errno::Errno::ESRCH {
                        ProcessSignalError::AlreadyGone
                    } else {
                        ProcessSignalError::Failed(error.to_string())
                    }
                })
        }
        #[cfg(not(unix))]
        {
            let _ = (group, signal, allow_leader_gone);
            Err(ProcessSignalError::Failed(
                "process-group signalling is unsupported on this platform".to_owned(),
            ))
        }
    }
}

#[cfg(target_os = "linux")]
fn verify_linux_group_leader(group: &OwnedProcessGroup) -> Result<(), ProcessSignalError> {
    match LinuxProcessProbe.process_birth_checked(group.leader_pid) {
        Ok(Some(actual)) if actual == group.leader_birth => {}
        Ok(Some(_)) => {
            return Err(ProcessSignalError::Failed(
                "process-group leader birth changed".to_owned(),
            ));
        }
        Ok(None) => return Err(ProcessSignalError::AlreadyGone),
        Err(error) => {
            return Err(ProcessSignalError::Failed(format!(
                "could not corroborate process-group leader: {error}"
            )));
        }
    }
    match LinuxProcessProbe.process_group_checked(group.leader_pid) {
        Ok(Some(actual))
            if actual.process_group_id == group.process_group_id
                && actual.session_id == group.session_id =>
        {
            Ok(())
        }
        Ok(Some(_)) => Err(ProcessSignalError::Failed(
            "process-group leader changed groups".to_owned(),
        )),
        Ok(None) => Err(ProcessSignalError::AlreadyGone),
        Err(error) => Err(ProcessSignalError::Failed(format!(
            "could not corroborate process-group membership: {error}"
        ))),
    }
}

#[cfg(target_os = "linux")]
fn stop_and_verify_linux_group_leader(
    pidfd: &rustix::fd::OwnedFd,
    group: &OwnedProcessGroup,
    allow_leader_gone: bool,
) -> Result<bool, ProcessSignalError> {
    if let Err(error) = verify_linux_group_leader(group) {
        if allow_leader_gone && matches!(error, ProcessSignalError::AlreadyGone) {
            // The member scan performed by the caller is the only remaining
            // ownership evidence for KILL escalation.
            return Ok(false);
        }
        return Err(error);
    }
    match rustix::process::pidfd_send_signal(pidfd, rustix::process::Signal::STOP) {
        Ok(()) => {}
        Err(error) if error == rustix::io::Errno::SRCH && allow_leader_gone => return Ok(false),
        Err(error) if error == rustix::io::Errno::SRCH => {
            return Err(ProcessSignalError::AlreadyGone);
        }
        Err(error) => {
            return Err(ProcessSignalError::Failed(format!(
                "pidfd_stop failed: {error}"
            )));
        }
    }
    if let Err(error) = verify_linux_group_leader(group) {
        let _ = rustix::process::pidfd_send_signal(pidfd, rustix::process::Signal::CONT);
        return Err(error);
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
fn send_linux_group_signal(
    group: &OwnedProcessGroup,
    owned_signal: OwnedProcessSignal,
    pidfd: Option<&rustix::fd::OwnedFd>,
    leader_stopped: bool,
) -> Result<(), ProcessSignalError> {
    let process_group_id = i32::try_from(group.process_group_id)
        .map_err(|_| ProcessSignalError::Failed("process-group ID is out of range".to_owned()))?;
    let process_group_id = rustix::process::Pid::from_raw(process_group_id)
        .ok_or_else(|| ProcessSignalError::Failed("process-group ID is zero".to_owned()))?;
    let signal = match owned_signal {
        OwnedProcessSignal::Term => rustix::process::Signal::TERM,
        OwnedProcessSignal::Kill => rustix::process::Signal::KILL,
    };
    let group_result =
        rustix::process::kill_process_group(process_group_id, signal).map_err(|error| {
            if error == rustix::io::Errno::SRCH {
                ProcessSignalError::AlreadyGone
            } else {
                ProcessSignalError::Failed(format!("process-group signal failed: {error}"))
            }
        });
    if leader_stopped {
        let pidfd = pidfd.expect("stopped leader always has a pidfd");
        let continue_result =
            rustix::process::pidfd_send_signal(pidfd, rustix::process::Signal::CONT).map_err(
                |error| {
                    if error == rustix::io::Errno::SRCH {
                        ProcessSignalError::AlreadyGone
                    } else {
                        ProcessSignalError::Failed(format!("pidfd_continue failed: {error}"))
                    }
                },
            );
        if let Err(ProcessSignalError::Failed(message)) = continue_result {
            return Err(ProcessSignalError::Failed(message));
        }
    }
    group_result
}

/// Terminates one exact helper process after its owner has stopped exposing
/// it to user-facing work. The process birth token is checked before every
/// signal and before the KILL fallback, so a reused PID is never signalled.
///
/// A missing or changed birth token is safe evidence that the recorded helper
/// is already gone; signal failures and a process that survives both bounded
/// phases remain errors for the caller to reconcile. Native provider Runtime
/// groups are stopped before their private tmux server through
/// [`terminate_owned_provider_process`].
///
/// # Errors
///
/// Returns an error when the persisted identity is invalid, signalling fails,
/// or the exact process survives both bounded shutdown phases.
pub fn terminate_owned_process(
    provider_pid: u32,
    expected_birth: &str,
    process_probe: &dyn ProcessProbe,
    process_signaler: &dyn ProcessSignaler,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OwnedProcessTermination, RuntimeError> {
    if provider_pid == 0 || expected_birth.is_empty() {
        return Err(RuntimeError::InvalidProcessIdentity);
    }
    if !exact_process_identity(process_probe, provider_pid, expected_birth)? {
        return Ok(OwnedProcessTermination::AlreadyGone);
    }

    match process_signaler.signal(provider_pid, expected_birth, OwnedProcessSignal::Term) {
        Ok(()) => {}
        Err(ProcessSignalError::AlreadyGone) => {
            return Ok(OwnedProcessTermination::AlreadyGone);
        }
        Err(error) => return Err(RuntimeError::ProcessSignal(error)),
    }
    if wait_for_process_exit(
        provider_pid,
        expected_birth,
        process_probe,
        timeout,
        poll_interval,
    )? {
        return Ok(OwnedProcessTermination::TerminatedByTerm);
    }

    // Re-check the exact identity at the escalation boundary. If the PID was
    // reused, treating it as gone is the only safe action; never send KILL.
    if !exact_process_identity(process_probe, provider_pid, expected_birth)? {
        return Ok(OwnedProcessTermination::AlreadyGone);
    }
    match process_signaler.signal(provider_pid, expected_birth, OwnedProcessSignal::Kill) {
        Ok(()) | Err(ProcessSignalError::AlreadyGone) => {}
        Err(error) => return Err(RuntimeError::ProcessSignal(error)),
    }
    if wait_for_process_exit(
        provider_pid,
        expected_birth,
        process_probe,
        timeout,
        poll_interval,
    )? {
        Ok(OwnedProcessTermination::TerminatedByKill)
    } else {
        Err(RuntimeError::ProcessShutdownTimedOut)
    }
}

/// Proves that the exact live provider process is the leader of its own
/// process group. The returned evidence is captured before the Runtime's
/// private tmux server is stopped, while the leader PID plus birth token can
/// still corroborate the group ownership.
///
/// # Errors
///
/// Returns an error when process metadata is absent, ambiguous, malformed, or
/// does not show `provider_pid == process_group_id` with a visible group
/// member for that leader.
pub fn prove_owned_process_group(
    provider_pid: u32,
    expected_birth: &str,
    process_probe: &dyn ProcessProbe,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<OwnedProcessGroup, RuntimeError> {
    if provider_pid == 0 || expected_birth.is_empty() {
        return Err(RuntimeError::InvalidProcessIdentity);
    }
    if !exact_process_identity(process_probe, provider_pid, expected_birth)? {
        return Err(RuntimeError::ProcessIdentityChanged);
    }
    prove_owned_process_group_after_identity(provider_pid, expected_birth, process_group_probe)
}

fn prove_owned_process_group_after_identity(
    provider_pid: u32,
    expected_birth: &str,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<OwnedProcessGroup, RuntimeError> {
    let Some(group) = process_group_probe
        .process_group_checked(provider_pid)
        .map_err(RuntimeError::ProcessGroupProbe)?
    else {
        return Err(RuntimeError::ProcessGroupIdentityMismatch);
    };
    if group.process_group_id != provider_pid || group.session_id == 0 {
        return Err(RuntimeError::ProcessGroupIdentityMismatch);
    }
    let members = process_group_probe
        .process_group_members_checked(&ProcessGroupInfo {
            process_group_id: group.process_group_id,
            session_id: group.session_id,
        })
        .map_err(RuntimeError::ProcessGroupProbe)?;
    if !members.contains(&provider_pid) {
        return Err(RuntimeError::ProcessGroupIdentityMismatch);
    }
    Ok(OwnedProcessGroup {
        leader_pid: provider_pid,
        leader_birth: expected_birth.to_owned(),
        process_group_id: group.process_group_id,
        session_id: group.session_id,
    })
}

/// Terminates the complete process group owned by one exact provider leader.
/// TERM and KILL each retain bounded deadlines. KILL may proceed after the
/// leader exits only when a process-table scan still proves members retain the
/// group ID captured while the leader was exact; an empty group is treated as
/// already gone. A changed/reused leader PID never authorizes a group signal.
///
/// # Errors
///
/// Returns an error when the persisted process identity or group ownership is
/// invalid, signalling fails, or the group survives both bounded shutdown
/// phases.
pub fn terminate_owned_process_group(
    provider_pid: u32,
    expected_birth: &str,
    process_probe: &dyn ProcessProbe,
    process_group_probe: &dyn ProcessGroupProbe,
    process_group_signaler: &dyn ProcessGroupSignaler,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OwnedProcessTermination, RuntimeError> {
    if provider_pid == 0 || expected_birth.is_empty() {
        return Err(RuntimeError::InvalidProcessIdentity);
    }
    let group = match process_probe
        .process_birth_checked(provider_pid)
        .map_err(RuntimeError::ProcessProbe)?
    {
        Some(actual) if actual == expected_birth => prove_owned_process_group_after_identity(
            provider_pid,
            expected_birth,
            process_group_probe,
        )?,
        Some(_) => return Err(RuntimeError::ProcessIdentityChanged),
        None => {
            let members = process_group_probe
                .process_group_members_by_id_checked(provider_pid)
                .map_err(RuntimeError::ProcessGroupProbe)?;
            if members.is_empty() {
                return Ok(OwnedProcessTermination::AlreadyGone);
            }
            return Err(RuntimeError::ProcessGroupIdentityMismatch);
        }
    };

    terminate_live_proven_process_group(
        &group,
        process_probe,
        process_group_probe,
        process_group_signaler,
        timeout,
        poll_interval,
    )
}

/// Terminates a process group whose leader PID/birth/group/session identity
/// was already proven by the caller.  This is used by crash-surviving
/// guardians which retain the exact group authority after their action owner
/// has disappeared.  A live leader is revalidated before TERM/KILL.  If the
/// leader has exited, KILL is allowed only while an exact captured
/// group/session member scan still proves that the original group is occupied;
/// an empty group is treated as already gone.
///
/// # Errors
///
/// Returns an error when the captured identity no longer corroborates, group
/// probing/signalling is unavailable, or surviving members ignore both
/// bounded shutdown phases.
pub fn terminate_preproven_process_group(
    group: &OwnedProcessGroup,
    process_probe: &dyn ProcessProbe,
    process_group_probe: &dyn ProcessGroupProbe,
    process_group_signaler: &dyn ProcessGroupSignaler,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OwnedProcessTermination, RuntimeError> {
    if group.leader_pid == 0
        || group.leader_birth.is_empty()
        || group.process_group_id == 0
        || group.process_group_id != group.leader_pid
        || group.session_id == 0
    {
        return Err(RuntimeError::InvalidProcessIdentity);
    }

    let leader_gone = match process_probe
        .process_birth_checked(group.leader_pid)
        .map_err(RuntimeError::ProcessProbe)?
    {
        Some(actual) if actual == group.leader_birth => {
            let actual_group = process_group_probe
                .process_group_checked(group.leader_pid)
                .map_err(RuntimeError::ProcessGroupProbe)?
                .ok_or(RuntimeError::ProcessGroupIdentityMismatch)?;
            if actual_group.process_group_id != group.process_group_id
                || actual_group.session_id != group.session_id
            {
                return Err(RuntimeError::ProcessGroupIdentityMismatch);
            }
            false
        }
        Some(_) => return Err(RuntimeError::ProcessIdentityChanged),
        None => {
            let members = process_group_probe
                .process_group_members_checked(&ProcessGroupInfo {
                    process_group_id: group.process_group_id,
                    session_id: group.session_id,
                })
                .map_err(RuntimeError::ProcessGroupProbe)?;
            if members.is_empty() {
                return Ok(OwnedProcessTermination::AlreadyGone);
            }
            true
        }
    };

    if leader_gone {
        match process_group_signaler.signal_group(group, OwnedProcessSignal::Kill, true) {
            Ok(()) | Err(ProcessSignalError::AlreadyGone) => {}
            Err(error) => return Err(RuntimeError::ProcessGroupSignal(error)),
        }
        return if wait_for_process_group_exit(group, process_group_probe, timeout, poll_interval)? {
            Ok(OwnedProcessTermination::TerminatedByKill)
        } else {
            Err(RuntimeError::ProcessShutdownTimedOut)
        };
    }

    terminate_live_proven_process_group(
        group,
        process_probe,
        process_group_probe,
        process_group_signaler,
        timeout,
        poll_interval,
    )
}

/// Terminates a group whose leader was just proven live and exact by the
/// caller. The initial identity proof is intentionally outside this helper so
/// callers that already performed it do not consume a second probe sample
/// before the first TERM signal.
fn terminate_live_proven_process_group(
    group: &OwnedProcessGroup,
    process_probe: &dyn ProcessProbe,
    process_group_probe: &dyn ProcessGroupProbe,
    process_group_signaler: &dyn ProcessGroupSignaler,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OwnedProcessTermination, RuntimeError> {
    match process_group_signaler.signal_group(group, OwnedProcessSignal::Term, false) {
        Ok(()) | Err(ProcessSignalError::AlreadyGone) => {}
        Err(error) => return Err(RuntimeError::ProcessGroupSignal(error)),
    }
    if wait_for_process_group_exit(group, process_group_probe, timeout, poll_interval)? {
        return Ok(OwnedProcessTermination::TerminatedByTerm);
    }

    // Revalidate the provider leader before KILL. A changed birth token is a
    // PID-reuse event and therefore fails closed even if the numeric group ID
    // happens to match. A missing leader is allowed only because the group
    // member scan below still proves that this previously captured group has
    // surviving members.
    let leader_gone = match process_probe
        .process_birth_checked(group.leader_pid)
        .map_err(RuntimeError::ProcessProbe)?
    {
        Some(actual) if actual == group.leader_birth => {
            let actual_group = process_group_probe
                .process_group_checked(group.leader_pid)
                .map_err(RuntimeError::ProcessGroupProbe)?
                .ok_or(RuntimeError::ProcessGroupIdentityMismatch)?;
            if actual_group.process_group_id != group.process_group_id
                || actual_group.session_id != group.session_id
            {
                return Err(RuntimeError::ProcessGroupIdentityMismatch);
            }
            false
        }
        Some(_) => return Err(RuntimeError::ProcessIdentityChanged),
        None => true,
    };
    if leader_gone
        && process_group_probe
            .process_group_members_checked(&ProcessGroupInfo {
                process_group_id: group.process_group_id,
                session_id: group.session_id,
            })
            .map_err(RuntimeError::ProcessGroupProbe)?
            .is_empty()
    {
        return Ok(OwnedProcessTermination::TerminatedByTerm);
    }
    match process_group_signaler.signal_group(group, OwnedProcessSignal::Kill, leader_gone) {
        Ok(()) | Err(ProcessSignalError::AlreadyGone) => {}
        Err(error) => return Err(RuntimeError::ProcessGroupSignal(error)),
    }
    if wait_for_process_group_exit(group, process_group_probe, timeout, poll_interval)? {
        Ok(OwnedProcessTermination::TerminatedByKill)
    } else {
        Err(RuntimeError::ProcessShutdownTimedOut)
    }
}

fn wait_for_process_group_exit(
    group: &OwnedProcessGroup,
    process_group_probe: &dyn ProcessGroupProbe,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<bool, RuntimeError> {
    let deadline = Instant::now() + timeout;
    loop {
        if process_group_probe
            .process_group_members_checked(&ProcessGroupInfo {
                process_group_id: group.process_group_id,
                session_id: group.session_id,
            })
            .map_err(RuntimeError::ProcessGroupProbe)?
            .is_empty()
        {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(poll_interval.max(Duration::from_millis(1)));
    }
}

/// Production wrapper using the host's process identity and signal adapters.
///
/// # Errors
///
/// Returns an error when the persisted identity is invalid, signalling fails,
/// or the exact process survives both bounded shutdown phases.
pub fn terminate_owned_provider_process(
    provider_pid: u32,
    expected_birth: &str,
    timeout: Duration,
) -> Result<OwnedProcessTermination, RuntimeError> {
    terminate_owned_process_group(
        provider_pid,
        expected_birth,
        &LinuxProcessProbe,
        &LinuxProcessProbe,
        &SystemProcessGroupSignaler,
        timeout,
        PROVIDER_SHUTDOWN_POLL_INTERVAL,
    )
}

/// Production wrapper for helper processes that are not guaranteed to own a
/// process group (for example the `OpenCode` observer sidecar). Native provider
/// Runtime cleanup uses [`terminate_owned_provider_process`] instead.
///
/// # Errors
///
/// Returns an error when the persisted helper identity is invalid, signalling
/// fails, or the helper survives both bounded shutdown phases.
pub fn terminate_owned_observer_process(
    observer_pid: u32,
    expected_birth: &str,
    timeout: Duration,
) -> Result<OwnedProcessTermination, RuntimeError> {
    terminate_owned_process(
        observer_pid,
        expected_birth,
        &LinuxProcessProbe,
        &SystemProcessSignaler,
        timeout,
        PROVIDER_SHUTDOWN_POLL_INTERVAL,
    )
}

fn exact_process_identity(
    probe: &dyn ProcessProbe,
    pid: u32,
    expected_birth: &str,
) -> Result<bool, RuntimeError> {
    probe
        .process_birth_checked(pid)
        .map(|birth| birth.as_deref() == Some(expected_birth))
        .map_err(RuntimeError::ProcessProbe)
}

fn wait_for_process_exit(
    pid: u32,
    expected_birth: &str,
    process_probe: &dyn ProcessProbe,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<bool, RuntimeError> {
    let deadline = Instant::now() + timeout;
    loop {
        if !exact_process_identity(process_probe, pid, expected_birth)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(poll_interval.max(Duration::from_millis(1)));
    }
}

/// Linux process-birth probe backed by the process stat file.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxProcessProbe;

impl ProcessProbe for LinuxProcessProbe {
    fn process_birth(&self, pid: u32) -> Option<String> {
        self.process_birth_checked(pid).ok().flatten()
    }

    fn process_birth_checked(&self, pid: u32) -> Result<Option<String>, ProcessProbeError> {
        Ok(read_linux_process_stat(pid)?.map(|stat| stat.birth))
    }
}

impl ProcessGroupProbe for LinuxProcessProbe {
    fn process_group_checked(
        &self,
        pid: u32,
    ) -> Result<Option<ProcessGroupInfo>, ProcessProbeError> {
        Ok(read_linux_process_stat(pid)?.map(|stat| ProcessGroupInfo {
            process_group_id: stat.process_group_id,
            session_id: stat.session_id,
        }))
    }

    fn process_group_members_checked(
        &self,
        group: &ProcessGroupInfo,
    ) -> Result<Vec<u32>, ProcessProbeError> {
        if group.process_group_id == 0 || group.session_id == 0 {
            return Err(ProcessProbeError::Malformed);
        }
        linux_process_group_members(group.process_group_id, Some(group.session_id))
    }

    fn process_group_members_by_id_checked(
        &self,
        process_group_id: u32,
    ) -> Result<Vec<u32>, ProcessProbeError> {
        if process_group_id == 0 {
            return Err(ProcessProbeError::Malformed);
        }
        linux_process_group_members(process_group_id, None)
    }
}

fn linux_process_group_members(
    process_group_id: u32,
    session_id: Option<u32>,
) -> Result<Vec<u32>, ProcessProbeError> {
    if process_group_id == 0 {
        return Err(ProcessProbeError::Malformed);
    }
    let entries = fs::read_dir("/proc").map_err(|error| match error.kind() {
        std::io::ErrorKind::PermissionDenied => ProcessProbeError::Inaccessible,
        _ => ProcessProbeError::Io(error.to_string()),
    })?;
    let mut members = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| ProcessProbeError::Io(error.to_string()))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        match read_linux_process_stat_for_group(pid)? {
            Some(stat)
                if stat.process_group_id == process_group_id
                    && session_id.is_none_or(|session| stat.session_id == session)
                    && stat.state != 'Z' =>
            {
                members.push(pid);
            }
            Some(_) | None => {}
        }
    }
    members.sort_unstable();
    Ok(members)
}

fn read_linux_process_stat_for_group(
    pid: u32,
) -> Result<Option<LinuxProcessStat>, ProcessProbeError> {
    // Enumeration may observe a transient malformed stat while an unrelated
    // process is exiting. Direct identity reads remain strict and never use
    // this retry boundary.
    read_linux_process_stat_for_group_with(pid, read_linux_process_stat, || {
        thread::sleep(PROCESS_GROUP_STAT_RETRY_DELAY);
    })
}

fn read_linux_process_stat_for_group_with<R, W>(
    pid: u32,
    mut read: R,
    mut wait: W,
) -> Result<Option<LinuxProcessStat>, ProcessProbeError>
where
    R: FnMut(u32) -> Result<Option<LinuxProcessStat>, ProcessProbeError>,
    W: FnMut(),
{
    let mut attempts = 0;
    loop {
        attempts += 1;
        match read(pid) {
            Err(ProcessProbeError::Malformed) if attempts < PROCESS_GROUP_STAT_READ_ATTEMPTS => {
                wait();
            }
            result => return result,
        }
    }
}

#[derive(Debug)]
struct LinuxProcessStat {
    state: char,
    birth: String,
    process_group_id: u32,
    session_id: u32,
}

fn read_linux_process_stat(pid: u32) -> Result<Option<LinuxProcessStat>, ProcessProbeError> {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(ProcessProbeError::Inaccessible);
        }
        Err(error) => return Err(ProcessProbeError::Io(error.to_string())),
    };
    let close_paren = stat.rfind(')').ok_or(ProcessProbeError::Malformed)?;
    let fields = stat
        .get(close_paren + 2..)
        .ok_or(ProcessProbeError::Malformed)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let birth = fields
        .get(19)
        .ok_or(ProcessProbeError::Malformed)?
        .to_string();
    let state = fields
        .first()
        .and_then(|field| field.chars().next())
        .ok_or(ProcessProbeError::Malformed)?;
    let process_group_id = fields
        .get(2)
        .ok_or(ProcessProbeError::Malformed)?
        .parse()
        .map_err(|_| ProcessProbeError::Malformed)?;
    let session_id = fields
        .get(3)
        .ok_or(ProcessProbeError::Malformed)?
        .parse()
        .map_err(|_| ProcessProbeError::Malformed)?;
    Ok(Some(LinuxProcessStat {
        state,
        birth,
        process_group_id,
        session_id,
    }))
}

/// Confirms that this hook process is a direct child of the current provider.
///
/// Codex 0.146.0 uses this shape for native command hooks. A shell-wrapper
/// allowance would let an agent tool shell invoke the hook handler and forge a
/// lifecycle payload, so an unknown process topology fails closed.
#[must_use]
pub fn is_direct_provider_hook(provider_pid: u32, expected_birth: &str) -> bool {
    let probe = LinuxProcessProbe;
    if probe.process_birth(provider_pid).as_deref() != Some(expected_birth) {
        return false;
    }
    let Some(parent) = process_parent(std::process::id()) else {
        return false;
    };
    has_direct_provider_parent(parent, provider_pid)
}

fn has_direct_provider_parent(hook_parent: u32, provider_pid: u32) -> bool {
    hook_parent == provider_pid
}

fn process_parent(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = stat.rfind(')')?;
    stat.get(close_paren + 2..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
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
        self.start_with_startup(launch, &NoRuntimeStartup)
    }

    /// Creates the private tmux server after the supplied exact bootstrap has
    /// prepared only that Runtime's new directory. The startup hook runs after
    /// directory ownership exists and before configuration or tmux invocation;
    /// a failure leaves the owned directory for conservative reconciliation.
    pub(crate) fn start_with_startup(
        &self,
        launch: &NativeLaunch,
        startup: &dyn RuntimeStartup,
    ) -> Result<(), RuntimeError> {
        launch.validate()?;
        if self.paths.directory.exists() {
            return Err(RuntimeError::RuntimeAlreadyOwned(
                self.paths.directory.clone(),
            ));
        }
        create_private_runtime_directory(&self.paths.directory)?;
        startup.prepare(&self.paths)?;
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

    /// Releases the already-started pane process to replace itself with the
    /// native provider after its exact process birth has been persisted.
    ///
    /// # Errors
    ///
    /// Returns an error unless this owner can create the one fresh private
    /// barrier file with mode `0600`.
    pub fn release_launch(&self) -> Result<(), RuntimeError> {
        create_launch_barrier(&self.paths.launch_barrier())
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
            let diagnostic = trim_diagnostic(&exists.stderr);
            return Ok(
                if !self.paths.socket.exists() || is_missing_server(&exists.stderr) {
                    RuntimeProbe::Missing
                } else {
                    RuntimeProbe::Unknown {
                        diagnostic: if diagnostic.is_empty() {
                            "private tmux session probe was unavailable".to_owned()
                        } else {
                            diagnostic
                        },
                    }
                },
            );
        }

        let pane_target = OsString::from(format!("{}:0.0", self.paths.session_name));
        let pane_reference = match read_pane_field(
            self.tmux,
            &self.paths.socket,
            &pane_target,
            "#{pane_id}",
            "pane ID",
        )? {
            Ok(value) => value,
            Err(diagnostic) => return Ok(RuntimeProbe::Unknown { diagnostic }),
        };
        let process_id_text = match read_pane_field(
            self.tmux,
            &self.paths.socket,
            &pane_target,
            "#{pane_pid}",
            "pane PID",
        )? {
            Ok(value) => value,
            Err(diagnostic) => return Ok(RuntimeProbe::Unknown { diagnostic }),
        };
        let cwd = match read_pane_field(
            self.tmux,
            &self.paths.socket,
            &pane_target,
            "#{pane_current_path}",
            "pane current path",
        )? {
            Ok(value) => value,
            Err(diagnostic) => return Ok(RuntimeProbe::Unknown { diagnostic }),
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
            // A Runtime is created detached, before a terminal client exists.
            // Explicitly mark the eventual attach client UTF-8 capable so tmux
            // preserves the native Codex glyphs through an SSH/nested-tmux path.
            .arg("-u")
            .arg("-S")
            .arg(&self.paths.socket)
            .arg("attach-session")
            .arg("-t")
            .arg(&self.paths.session_name);
        command
    }

    /// Pre-sizes the exact private Runtime window from the invoking terminal,
    /// then returns tmux to its native `latest` sizing policy.
    ///
    /// The Runtime is created detached, so tmux otherwise gives its first
    /// provider client the server's default geometry. This handshake is
    /// intentionally transient and must complete before the direct attach
    /// command is spawned.
    ///
    /// # Errors
    ///
    /// Returns an error when the invoking terminal has no valid geometry or
    /// any exact private tmux command is rejected.
    pub(crate) fn prepare_attach(&self) -> Result<(), RuntimeError> {
        let (columns, rows) =
            crossterm::terminal::size().map_err(|_| RuntimeError::TerminalGeometryUnavailable)?;
        self.prepare_attach_with_size(columns, rows)
    }

    pub(crate) fn prepare_attach_with_size(
        &self,
        columns: u16,
        rows: u16,
    ) -> Result<(), RuntimeError> {
        let (columns, rows) = validate_terminal_geometry(columns, rows)?;
        self.reconcile_copy_mode_scroll_bindings()?;
        let target = OsString::from(format!("{}:{PROVIDER_WINDOW}", self.paths.session_name));
        let resized = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: None,
            arguments: vec![
                OsString::from("resize-window"),
                OsString::from("-t"),
                target.clone(),
                OsString::from("-x"),
                OsString::from(columns.to_string()),
                OsString::from("-y"),
                OsString::from(rows.to_string()),
            ],
        })?;
        if !resized.success {
            return Err(RuntimeError::TmuxRejected(trim_diagnostic(&resized.stderr)));
        }
        let latest = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: None,
            arguments: vec![
                OsString::from("set-window-option"),
                OsString::from("-t"),
                target,
                OsString::from("window-size"),
                OsString::from("latest"),
            ],
        })?;
        if !latest.success {
            return Err(RuntimeError::TmuxRejected(trim_diagnostic(&latest.stderr)));
        }
        Ok(())
    }

    /// Reapplies the owned copy-mode wheel profile to an existing Runtime.
    ///
    /// Runtime servers outlive individual `wsnav attach` processes. Binding
    /// the exact four entries on every attach converges servers created by an
    /// older binary without restarting the native provider. Repeated binds
    /// replace the same keys and are therefore idempotent.
    fn reconcile_copy_mode_scroll_bindings(&self) -> Result<(), RuntimeError> {
        for binding in COPY_MODE_SCROLL_BINDINGS {
            let arguments = binding
                .arguments()
                .into_iter()
                .map(OsString::from)
                .collect();
            let response = self.tmux.invoke(&TmuxInvocation {
                socket: self.paths.socket.clone(),
                config: None,
                arguments,
            })?;
            if !response.success {
                return Err(RuntimeError::TmuxRejected(trim_diagnostic(
                    &response.stderr,
                )));
            }
        }
        Ok(())
    }

    /// Delivers exactly one literal C-b to the owned provider pane through
    /// this Runtime's private tmux server. This bypasses the nested client's
    /// prefix table entirely; callers must complete authoritative attachment
    /// preflight before invoking it.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private Runtime tmux server rejects
    /// the bounded input action.
    pub fn send_literal_ctrl_b(&self) -> Result<(), RuntimeError> {
        let response = self.tmux.invoke(&TmuxInvocation {
            socket: self.paths.socket.clone(),
            config: None,
            arguments: vec![
                OsString::from("send-keys"),
                OsString::from("-t"),
                OsString::from(format!("{}:{PROVIDER_WINDOW}.0", self.paths.session_name)),
                OsString::from("C-b"),
            ],
        })?;
        if response.success {
            Ok(())
        } else {
            Err(RuntimeError::TmuxRejected(trim_diagnostic(
                &response.stderr,
            )))
        }
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

/// Waits for the owning action to persist launch authority, then consumes its
/// one-shot signal before the caller replaces itself with the native provider.
///
/// # Errors
///
/// Returns an error when the barrier is malformed, inaccessible, or not
/// released within the bounded startup interval.
pub fn await_launch_release(paths: &RuntimePaths) -> Result<(), RuntimeError> {
    await_launch_release_with_timeout(paths, LAUNCH_BARRIER_TIMEOUT)
}

fn await_launch_release_with_timeout(
    paths: &RuntimePaths,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let barrier = paths.launch_barrier();
    let deadline = Instant::now() + timeout;
    loop {
        match fs::symlink_metadata(&barrier) {
            Ok(metadata) if metadata.file_type().is_file() => {
                fs::remove_file(&barrier).map_err(|source| RuntimeError::Io {
                    path: barrier,
                    source,
                })?;
                return Ok(());
            }
            Ok(_) => return Err(RuntimeError::InvalidLaunchBarrier(barrier)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RuntimeError::Io {
                    path: barrier,
                    source,
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(RuntimeError::LaunchBarrierTimedOut);
        }
        thread::sleep(LAUNCH_BARRIER_POLL_INTERVAL);
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
    fs::write(path, runtime_tmux_config()).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    set_mode(path, 0o600)
}

fn validate_terminal_geometry(columns: u16, rows: u16) -> Result<(u16, u16), RuntimeError> {
    if columns == 0 || rows == 0 {
        return Err(RuntimeError::InvalidTerminalGeometry);
    }
    Ok((columns, rows))
}

fn create_launch_barrier(path: &Path) -> Result<(), RuntimeError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|source| RuntimeError::Io {
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

/// Reads one tmux pane fact without relying on a separator in tmux format output.
///
/// tmux 3.7b normalizes literal tab separators in format output, so a combined
/// record cannot be parsed reliably across supported hosts. Separate bounded
/// queries also let the caller reject ambiguous line-based output fail-closed.
fn read_pane_field(
    tmux: &dyn TmuxClient,
    socket: &Path,
    pane_target: &OsString,
    format: &str,
    label: &str,
) -> Result<Result<String, String>, RuntimeError> {
    let response = tmux.invoke(&TmuxInvocation {
        socket: socket.to_path_buf(),
        config: None,
        arguments: vec![
            OsString::from("display-message"),
            OsString::from("-p"),
            OsString::from("-t"),
            pane_target.clone(),
            OsString::from(format),
        ],
    })?;
    if !response.success {
        return Ok(Err(trim_diagnostic(&response.stderr)));
    }
    let Some(value) = parse_single_pane_fact(&response.stdout) else {
        return Ok(Err(format!("private tmux {label} was malformed")));
    };
    Ok(Ok(value.to_owned()))
}

fn parse_single_pane_fact(output: &str) -> Option<&str> {
    let output = output.trim_end_matches(['\r', '\n']);
    if output.is_empty() || output.contains(['\r', '\n']) {
        return None;
    }
    Some(output)
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
    #[error("private Runtime startup is unavailable")]
    StartupUnavailable,
    #[error("invalid working directory {0}")]
    InvalidWorkingDirectory(PathBuf),
    #[error("invalid private runtime path {0}")]
    InvalidRuntimePath(PathBuf),
    #[error("invalid private runtime launch barrier {0}")]
    InvalidLaunchBarrier(PathBuf),
    #[error("private runtime launch authority was not released in time")]
    LaunchBarrierTimedOut,
    #[error("private runtime session identity did not match its persisted record")]
    RuntimeSessionMismatch,
    #[error("invoking terminal geometry is unavailable")]
    TerminalGeometryUnavailable,
    #[error("invoking terminal geometry is invalid")]
    InvalidTerminalGeometry,
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
    #[error("could not execute bounded private tmux control command")]
    TmuxOutput(#[source] BoundedProcessError),
    #[error("provider process identity is invalid")]
    InvalidProcessIdentity,
    #[error("provider process shutdown timed out")]
    ProcessShutdownTimedOut,
    #[error("could not verify provider process identity: {0}")]
    ProcessProbe(#[source] ProcessProbeError),
    #[error("could not verify provider process-group ownership: {0}")]
    ProcessGroupProbe(#[source] ProcessProbeError),
    #[error("provider process was not proven to lead its private process group")]
    ProcessGroupIdentityMismatch,
    #[error("provider process identity changed during process-group shutdown")]
    ProcessIdentityChanged,
    #[error("could not signal the exact provider process: {0}")]
    ProcessSignal(#[source] ProcessSignalError),
    #[error("could not signal the exact provider process group: {0}")]
    ProcessGroupSignal(#[source] ProcessSignalError),
}

impl RuntimeError {
    fn from_bounded_tmux(source: BoundedProcessError) -> Self {
        match source {
            BoundedProcessError::OutputTooLarge => Self::OutputTooLarge,
            other => Self::TmuxOutput(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
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

    struct SequenceProcessProbe {
        births: RefCell<VecDeque<Option<String>>>,
    }

    impl SequenceProcessProbe {
        fn new(births: impl IntoIterator<Item = Option<&'static str>>) -> Self {
            Self {
                births: RefCell::new(
                    births
                        .into_iter()
                        .map(|birth| birth.map(str::to_owned))
                        .collect(),
                ),
            }
        }
    }

    impl ProcessProbe for SequenceProcessProbe {
        fn process_birth(&self, _pid: u32) -> Option<String> {
            self.births.borrow_mut().pop_front().unwrap_or(None)
        }
    }

    struct AmbiguousProcessProbe;

    impl ProcessProbe for AmbiguousProcessProbe {
        fn process_birth(&self, _pid: u32) -> Option<String> {
            Some("birth-expected".to_owned())
        }

        fn process_birth_checked(&self, _pid: u32) -> Result<Option<String>, ProcessProbeError> {
            Err(ProcessProbeError::Inaccessible)
        }
    }

    #[derive(Default)]
    struct RecordingSignaler {
        signals: RefCell<Vec<(u32, String, OwnedProcessSignal)>>,
        failure: Option<ProcessSignalError>,
    }

    impl ProcessSignaler for RecordingSignaler {
        fn signal(
            &self,
            pid: u32,
            expected_birth: &str,
            signal: OwnedProcessSignal,
        ) -> Result<(), ProcessSignalError> {
            self.signals
                .borrow_mut()
                .push((pid, expected_birth.to_owned(), signal));
            if let Some(error) = &self.failure {
                return Err(match error {
                    ProcessSignalError::AlreadyGone => ProcessSignalError::AlreadyGone,
                    ProcessSignalError::Failed(message) => {
                        ProcessSignalError::Failed(message.clone())
                    }
                });
            }
            Ok(())
        }
    }

    struct FakeGroupProbe {
        group: Option<ProcessGroupInfo>,
        group_error: bool,
        members: RefCell<VecDeque<Vec<u32>>>,
        members_error: bool,
    }

    impl FakeGroupProbe {
        fn new(
            group: Option<ProcessGroupInfo>,
            members: impl IntoIterator<Item = Vec<u32>>,
        ) -> Self {
            Self {
                group,
                group_error: false,
                members: RefCell::new(members.into_iter().collect()),
                members_error: false,
            }
        }

        fn group_error() -> Self {
            Self {
                group: None,
                group_error: true,
                members: RefCell::default(),
                members_error: false,
            }
        }

        fn members_error() -> Self {
            Self {
                group: Some(ProcessGroupInfo {
                    process_group_id: 77,
                    session_id: 11,
                }),
                group_error: false,
                members: RefCell::default(),
                members_error: true,
            }
        }
    }

    impl ProcessGroupProbe for FakeGroupProbe {
        fn process_group_checked(
            &self,
            _pid: u32,
        ) -> Result<Option<ProcessGroupInfo>, ProcessProbeError> {
            if self.group_error {
                Err(ProcessProbeError::Inaccessible)
            } else {
                Ok(self.group)
            }
        }

        fn process_group_members_checked(
            &self,
            _group: &ProcessGroupInfo,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            if self.members_error {
                return Err(ProcessProbeError::Malformed);
            }
            Ok(self.members.borrow_mut().pop_front().unwrap_or_default())
        }

        fn process_group_members_by_id_checked(
            &self,
            _process_group_id: u32,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            self.process_group_members_checked(&ProcessGroupInfo {
                process_group_id: 1,
                session_id: 1,
            })
        }
    }

    #[derive(Default)]
    struct RecordingGroupSignaler {
        signals: RefCell<Vec<(u32, OwnedProcessSignal, bool)>>,
        failure: Option<ProcessSignalError>,
    }

    impl ProcessGroupSignaler for RecordingGroupSignaler {
        fn signal_group(
            &self,
            group: &OwnedProcessGroup,
            signal: OwnedProcessSignal,
            allow_leader_gone: bool,
        ) -> Result<(), ProcessSignalError> {
            self.signals
                .borrow_mut()
                .push((group.process_group_id, signal, allow_leader_gone));
            if let Some(error) = &self.failure {
                return Err(match error {
                    ProcessSignalError::AlreadyGone => ProcessSignalError::AlreadyGone,
                    ProcessSignalError::Failed(message) => {
                        ProcessSignalError::Failed(message.clone())
                    }
                });
            }
            Ok(())
        }
    }

    fn successful() -> TmuxResponse {
        TmuxResponse {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn parsed_process_stat(process_group_id: u32, session_id: u32) -> LinuxProcessStat {
        LinuxProcessStat {
            state: 'S',
            birth: "birth".to_owned(),
            process_group_id,
            session_id,
        }
    }

    #[test]
    fn process_group_stat_retry_accepts_a_later_parsed_record() {
        let reads = RefCell::new(VecDeque::from([
            Err(ProcessProbeError::Malformed),
            Ok(Some(parsed_process_stat(77, 11))),
        ]));
        let waits = RefCell::new(0);

        let result = read_linux_process_stat_for_group_with(
            77,
            |pid| {
                assert_eq!(pid, 77);
                reads.borrow_mut().pop_front().unwrap()
            },
            || *waits.borrow_mut() += 1,
        )
        .unwrap();

        let result = result.expect("later parsed process metadata");
        assert_eq!(result.process_group_id, 77);
        assert_eq!(result.session_id, 11);
        assert!(reads.borrow().is_empty());
        assert_eq!(*waits.borrow(), 1);
    }

    #[test]
    fn process_group_stat_retry_accepts_a_later_vanished_record() {
        let reads = RefCell::new(VecDeque::from([
            Err(ProcessProbeError::Malformed),
            Ok(None),
        ]));
        let waits = RefCell::new(0);

        let result = read_linux_process_stat_for_group_with(
            77,
            |_pid| reads.borrow_mut().pop_front().unwrap(),
            || *waits.borrow_mut() += 1,
        )
        .unwrap();

        assert!(result.is_none());
        assert!(reads.borrow().is_empty());
        assert_eq!(*waits.borrow(), 1);
    }

    #[test]
    fn process_group_stat_retry_propagates_persistent_malformed_metadata() {
        let reads = RefCell::new(VecDeque::from([
            Err(ProcessProbeError::Malformed),
            Err(ProcessProbeError::Malformed),
            Err(ProcessProbeError::Malformed),
        ]));
        let waits = RefCell::new(0);

        let result = read_linux_process_stat_for_group_with(
            77,
            |_pid| reads.borrow_mut().pop_front().unwrap(),
            || *waits.borrow_mut() += 1,
        );

        assert!(matches!(result, Err(ProcessProbeError::Malformed)));
        assert!(reads.borrow().is_empty());
        assert_eq!(*waits.borrow(), 2);
    }

    #[test]
    fn process_group_stat_retry_does_not_retry_inaccessible_or_io() {
        for error in [
            ProcessProbeError::Inaccessible,
            ProcessProbeError::Io("read failed".to_owned()),
        ] {
            let reads = RefCell::new(VecDeque::from([Err(error)]));
            let waits = RefCell::new(0);

            let result = read_linux_process_stat_for_group_with(
                77,
                |_pid| reads.borrow_mut().pop_front().unwrap(),
                || *waits.borrow_mut() += 1,
            );

            assert!(matches!(
                result,
                Err(ProcessProbeError::Inaccessible | ProcessProbeError::Io(_))
            ));
            assert!(reads.borrow().is_empty());
            assert_eq!(*waits.borrow(), 0);
        }
    }

    #[test]
    fn owned_process_shutdown_does_not_signal_an_absent_or_reused_pid() {
        for probe in [
            SequenceProcessProbe::new([None]),
            SequenceProcessProbe::new([Some("birth-other")]),
        ] {
            let signaler = RecordingSignaler::default();
            assert_eq!(
                terminate_owned_process(
                    77,
                    "birth-expected",
                    &probe,
                    &signaler,
                    Duration::ZERO,
                    Duration::ZERO,
                )
                .unwrap(),
                OwnedProcessTermination::AlreadyGone
            );
            assert!(signaler.signals.borrow().is_empty());
        }
    }

    #[test]
    fn owned_process_shutdown_refuses_ambiguous_probe_without_signalling() {
        let probe = AmbiguousProcessProbe;
        let signaler = RecordingSignaler::default();

        assert!(matches!(
            terminate_owned_process(
                77,
                "birth-expected",
                &probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            ),
            Err(RuntimeError::ProcessProbe(ProcessProbeError::Inaccessible))
        ));
        assert!(signaler.signals.borrow().is_empty());
    }

    #[test]
    fn owned_process_shutdown_accepts_term_exit_without_kill() {
        let probe = SequenceProcessProbe::new([Some("birth-expected"), None]);
        let signaler = RecordingSignaler::default();

        assert_eq!(
            terminate_owned_process(
                77,
                "birth-expected",
                &probe,
                &signaler,
                Duration::from_millis(20),
                Duration::from_millis(1),
            )
            .unwrap(),
            OwnedProcessTermination::TerminatedByTerm
        );
        assert_eq!(
            &*signaler.signals.borrow(),
            &[(77, "birth-expected".to_owned(), OwnedProcessSignal::Term)]
        );
    }

    #[test]
    fn owned_process_shutdown_escalates_to_kill_only_while_identity_matches() {
        let probe = SequenceProcessProbe::new([
            Some("birth-expected"),
            Some("birth-expected"),
            Some("birth-expected"),
            None,
        ]);
        let signaler = RecordingSignaler::default();

        assert_eq!(
            terminate_owned_process(
                77,
                "birth-expected",
                &probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap(),
            OwnedProcessTermination::TerminatedByKill
        );
        assert_eq!(
            &*signaler.signals.borrow(),
            &[
                (77, "birth-expected".to_owned(), OwnedProcessSignal::Term),
                (77, "birth-expected".to_owned(), OwnedProcessSignal::Kill),
            ]
        );
    }

    #[test]
    fn owned_process_shutdown_refuses_kill_after_pid_reuse() {
        let probe = SequenceProcessProbe::new([
            Some("birth-expected"),
            Some("birth-expected"),
            Some("birth-reused"),
        ]);
        let signaler = RecordingSignaler::default();

        assert_eq!(
            terminate_owned_process(
                77,
                "birth-expected",
                &probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap(),
            OwnedProcessTermination::AlreadyGone
        );
        assert_eq!(
            &*signaler.signals.borrow(),
            &[(77, "birth-expected".to_owned(), OwnedProcessSignal::Term)]
        );
    }

    #[test]
    fn owned_group_requires_the_exact_provider_to_lead_the_group() {
        let process_probe = SequenceProcessProbe::new([Some("birth-expected")]);
        let group_probe = FakeGroupProbe::new(
            Some(ProcessGroupInfo {
                process_group_id: 88,
                session_id: 11,
            }),
            [vec![88]],
        );
        let signaler = RecordingGroupSignaler::default();

        assert!(matches!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            ),
            Err(RuntimeError::ProcessGroupIdentityMismatch)
        ));
        assert!(signaler.signals.borrow().is_empty());
    }

    #[test]
    fn owned_group_refuses_ambiguous_group_metadata() {
        let process_probe = SequenceProcessProbe::new([Some("birth-expected")]);
        let group_probe = FakeGroupProbe::group_error();
        let signaler = RecordingGroupSignaler::default();

        assert!(matches!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            ),
            Err(RuntimeError::ProcessGroupProbe(
                ProcessProbeError::Inaccessible
            ))
        ));
        assert!(signaler.signals.borrow().is_empty());
    }

    #[test]
    fn owned_group_refuses_ambiguous_membership_without_signalling() {
        let process_probe = SequenceProcessProbe::new([Some("birth-expected")]);
        let group_probe = FakeGroupProbe::members_error();
        let signaler = RecordingGroupSignaler::default();

        assert!(matches!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            ),
            Err(RuntimeError::ProcessGroupProbe(
                ProcessProbeError::Malformed
            ))
        ));
        assert!(signaler.signals.borrow().is_empty());
    }

    #[test]
    fn owned_group_recovery_accepts_an_absent_leader_and_empty_group() {
        let process_probe = SequenceProcessProbe::new([None]);
        let group_probe = FakeGroupProbe::new(None, [vec![]]);
        let signaler = RecordingGroupSignaler::default();

        assert_eq!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap(),
            OwnedProcessTermination::AlreadyGone
        );
        assert!(signaler.signals.borrow().is_empty());
    }

    #[test]
    fn owned_group_recovery_fails_closed_when_absent_leader_has_members() {
        let process_probe = SequenceProcessProbe::new([None]);
        let group_probe = FakeGroupProbe::new(None, [vec![88]]);
        let signaler = RecordingGroupSignaler::default();

        assert!(matches!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            ),
            Err(RuntimeError::ProcessGroupIdentityMismatch)
        ));
        assert!(signaler.signals.borrow().is_empty());
    }

    #[test]
    fn preproven_group_kills_captured_members_after_leader_exit() {
        let process_probe = SequenceProcessProbe::new([None]);
        let group_probe = FakeGroupProbe::new(None, [vec![88], vec![]]);
        let signaler = RecordingGroupSignaler::default();
        let group = OwnedProcessGroup {
            leader_pid: 77,
            leader_birth: "birth-expected".to_owned(),
            process_group_id: 77,
            session_id: 11,
        };

        assert_eq!(
            terminate_preproven_process_group(
                &group,
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap(),
            OwnedProcessTermination::TerminatedByKill
        );
        assert_eq!(
            &*signaler.signals.borrow(),
            &[(77, OwnedProcessSignal::Kill, true)]
        );
    }

    #[test]
    fn owned_group_escalates_to_kill_for_surviving_descendants() {
        let process_probe =
            SequenceProcessProbe::new([Some("birth-expected"), Some("birth-expected")]);
        let group_probe = FakeGroupProbe::new(
            Some(ProcessGroupInfo {
                process_group_id: 77,
                session_id: 11,
            }),
            [vec![77, 88], vec![77, 88], vec![]],
        );
        let signaler = RecordingGroupSignaler::default();

        assert_eq!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap(),
            OwnedProcessTermination::TerminatedByKill
        );
        assert_eq!(
            &*signaler.signals.borrow(),
            &[
                (77, OwnedProcessSignal::Term, false),
                (77, OwnedProcessSignal::Kill, false),
            ]
        );
    }

    #[test]
    fn owned_group_escalates_after_leader_exit_when_members_retain_group() {
        let process_probe = SequenceProcessProbe::new([Some("birth-expected"), None]);
        let group_probe = FakeGroupProbe::new(
            Some(ProcessGroupInfo {
                process_group_id: 77,
                session_id: 11,
            }),
            [vec![77, 88], vec![77, 88], vec![88], vec![]],
        );
        let signaler = RecordingGroupSignaler::default();

        assert_eq!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap(),
            OwnedProcessTermination::TerminatedByKill
        );
        assert_eq!(
            &*signaler.signals.borrow(),
            &[
                (77, OwnedProcessSignal::Term, false),
                (77, OwnedProcessSignal::Kill, true),
            ]
        );
    }

    #[test]
    fn owned_group_refuses_pid_birth_change_before_kill() {
        let process_probe =
            SequenceProcessProbe::new([Some("birth-expected"), Some("birth-reused")]);
        let group_probe = FakeGroupProbe::new(
            Some(ProcessGroupInfo {
                process_group_id: 77,
                session_id: 11,
            }),
            [vec![77, 88], vec![77, 88]],
        );
        let signaler = RecordingGroupSignaler::default();

        assert!(matches!(
            terminate_owned_process_group(
                77,
                "birth-expected",
                &process_probe,
                &group_probe,
                &signaler,
                Duration::ZERO,
                Duration::ZERO,
            ),
            Err(RuntimeError::ProcessIdentityChanged)
        ));
        assert_eq!(
            &*signaler.signals.borrow(),
            &[(77, OwnedProcessSignal::Term, false)]
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn system_group_signaler_refuses_a_live_reused_birth_before_group_signal() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("sleep");
        command.arg("30").process_group(0);
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        let probe = LinuxProcessProbe;
        let group_info = probe
            .process_group_checked(pid)
            .unwrap()
            .expect("spawned leader group evidence");
        let group = OwnedProcessGroup {
            leader_pid: pid,
            leader_birth: "birth-reused".to_owned(),
            process_group_id: group_info.process_group_id,
            session_id: group_info.session_id,
        };

        assert!(matches!(
            SystemProcessGroupSignaler.signal_group(&group, OwnedProcessSignal::Term, false),
            Err(ProcessSignalError::Failed(_))
        ));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn hook_ancestry_requires_the_exact_provider_parent() {
        assert!(has_direct_provider_parent(42, 42));
        assert!(!has_direct_provider_parent(43, 42));
    }

    #[test]
    #[cfg(unix)]
    fn launch_barrier_is_private_and_consumed_once() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        fs::create_dir_all(&paths.directory).unwrap();
        set_mode(&paths.directory, 0o700).unwrap();
        let tmux = FakeTmux::default();
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());

        runtime.release_launch().unwrap();

        let barrier = paths.launch_barrier();
        assert_eq!(
            fs::metadata(&barrier).unwrap().permissions().mode() & 0o777,
            0o600
        );
        await_launch_release_with_timeout(&paths, Duration::from_millis(100)).unwrap();
        assert!(!barrier.exists());
    }

    #[test]
    fn launch_barrier_wait_is_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        fs::create_dir_all(&paths.directory).unwrap();

        assert!(matches!(
            await_launch_release_with_timeout(&paths, Duration::from_millis(20)),
            Err(RuntimeError::LaunchBarrierTimedOut)
        ));
    }

    #[test]
    fn private_paths_use_the_complete_runtime_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let first =
            RuntimeId::from(uuid::Uuid::parse_str("01234567-0000-0000-0000-000000000001").unwrap());
        let second =
            RuntimeId::from(uuid::Uuid::parse_str("01234567-0000-0000-0000-000000000002").unwrap());

        let first_paths = RuntimePaths::for_runtime(temporary.path(), first);
        let second_paths = RuntimePaths::for_runtime(temporary.path(), second);

        assert_ne!(first_paths.directory, second_paths.directory);
        assert_ne!(first_paths.session_name, second_paths.session_name);
        assert_eq!(first_paths.session_name, format!("wsnav-{first}"));
        assert!(first_paths.directory.ends_with(format!("runtime-{first}")));
    }

    #[test]
    fn persisted_legacy_session_selects_only_the_legacy_private_path() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime_id =
            RuntimeId::from(uuid::Uuid::parse_str("01234567-0000-0000-0000-000000000001").unwrap());
        let paths = RuntimePaths::for_record(
            temporary.path(),
            runtime_id,
            &format!("wsnav-{}", runtime_id.short()),
        )
        .unwrap();

        assert_eq!(paths.session_name, "wsnav-01234567");
        assert!(paths.directory.ends_with("runtime-01234567"));
        assert!(matches!(
            RuntimePaths::for_record(temporary.path(), runtime_id, "wsnav-foreign"),
            Err(RuntimeError::RuntimeSessionMismatch)
        ));
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

    struct PrivateStartup;

    impl RuntimeStartup for PrivateStartup {
        fn prepare(&self, paths: &RuntimePaths) -> Result<(), RuntimeError> {
            let bootstrap = paths.directory.join("bootstrap-proof");
            fs::write(&bootstrap, b"prepared").map_err(|source| RuntimeError::Io {
                path: bootstrap,
                source,
            })
        }
    }

    struct RefusingStartup;

    impl RuntimeStartup for RefusingStartup {
        fn prepare(&self, _paths: &RuntimePaths) -> Result<(), RuntimeError> {
            Err(RuntimeError::StartupUnavailable)
        }
    }

    #[test]
    fn startup_prepares_only_the_new_private_runtime_before_tmux_launch() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([successful()]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());
        let launch = NativeLaunch {
            cwd: temporary.path().to_path_buf(),
            program: vec![OsString::from("synthetic-shell")],
            environment: BTreeMap::new(),
        };

        runtime
            .start_with_startup(&launch, &PrivateStartup)
            .unwrap();

        assert_eq!(
            fs::read(paths.directory.join("bootstrap-proof")).unwrap(),
            b"prepared"
        );
        assert_eq!(tmux.calls.borrow().len(), 1);
    }

    #[test]
    fn startup_refusal_leaves_only_the_new_private_runtime_directory_for_reconciliation() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::default();
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());
        let launch = NativeLaunch {
            cwd: temporary.path().to_path_buf(),
            program: vec![OsString::from("synthetic-shell")],
            environment: BTreeMap::new(),
        };

        assert!(matches!(
            runtime.start_with_startup(&launch, &RefusingStartup),
            Err(RuntimeError::StartupUnavailable)
        ));
        assert!(paths.directory.is_dir());
        assert!(!paths.config.exists());
        assert!(tmux.calls.borrow().is_empty());
    }

    #[test]
    fn attach_marks_the_client_as_utf8_capable() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::default();
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());

        let command = runtime.attach_command();
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            arguments,
            vec![
                "-u".to_owned(),
                "-S".to_owned(),
                paths.socket.display().to_string(),
                "attach-session".to_owned(),
                "-t".to_owned(),
                paths.session_name,
            ]
        );
    }

    #[test]
    fn attach_geometry_targets_exact_window_and_restores_latest() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([
            successful(),
            successful(),
            successful(),
            successful(),
            successful(),
            successful(),
        ]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());

        runtime.prepare_attach_with_size(150, 40).unwrap();

        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 6);
        assert_eq!(calls[0].socket, paths.socket);
        assert_eq!(calls[0].config, None);
        for (call, binding) in calls.iter().zip(COPY_MODE_SCROLL_BINDINGS).take(4) {
            assert_eq!(
                call.arguments,
                binding
                    .arguments()
                    .into_iter()
                    .map(OsString::from)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            calls[4].arguments,
            vec![
                OsString::from("resize-window"),
                OsString::from("-t"),
                OsString::from(format!("{}:provider", paths.session_name)),
                OsString::from("-x"),
                OsString::from("150"),
                OsString::from("-y"),
                OsString::from("40"),
            ]
        );
        assert_eq!(
            calls[5].arguments,
            vec![
                OsString::from("set-window-option"),
                OsString::from("-t"),
                OsString::from(format!("{}:provider", paths.session_name)),
                OsString::from("window-size"),
                OsString::from("latest"),
            ]
        );
    }

    #[test]
    fn attach_geometry_rejection_stops_before_attach_or_restore() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let rejected = TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "resize rejected".to_owned(),
        };
        let tmux = FakeTmux::with_responses([
            successful(),
            successful(),
            successful(),
            successful(),
            rejected,
        ]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.prepare_attach_with_size(150, 40),
            Err(RuntimeError::TmuxRejected(message)) if message == "resize rejected"
        ));
        assert_eq!(tmux.calls.borrow().len(), 5);
    }

    #[test]
    fn attach_geometry_latest_rejection_stops_before_native_attach() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let rejected = TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "latest rejected".to_owned(),
        };
        let tmux = FakeTmux::with_responses([
            successful(),
            successful(),
            successful(),
            successful(),
            successful(),
            rejected,
        ]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.prepare_attach_with_size(150, 40),
            Err(RuntimeError::TmuxRejected(message)) if message == "latest rejected"
        ));
        assert_eq!(tmux.calls.borrow().len(), 6);
    }

    #[test]
    fn attach_reconciles_copy_mode_scroll_bindings_idempotently() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses((0..12).map(|_| successful()));
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        runtime.prepare_attach_with_size(150, 40).unwrap();
        runtime.prepare_attach_with_size(150, 40).unwrap();

        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 12);
        assert_eq!(&calls[0..4], &calls[6..10]);
        assert_eq!(calls[4].arguments[0], "resize-window");
        assert_eq!(calls[5].arguments[0], "set-window-option");
        assert_eq!(calls[10].arguments[0], "resize-window");
        assert_eq!(calls[11].arguments[0], "set-window-option");
    }

    #[test]
    fn copy_mode_binding_rejection_stops_before_geometry_or_attach() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let rejected = TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "binding rejected".to_owned(),
        };
        let tmux = FakeTmux::with_responses([successful(), rejected]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.prepare_attach_with_size(150, 40),
            Err(RuntimeError::TmuxRejected(message)) if message == "binding rejected"
        ));
        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments[0], "bind-key");
        assert_eq!(calls[1].arguments[0], "bind-key");
        assert!(calls.iter().all(|call| {
            call.arguments[0] != "resize-window" && call.arguments[0] != "set-window-option"
        }));
    }

    #[test]
    fn attach_geometry_rejects_zero_dimensions_without_tmux_access() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::default();
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.prepare_attach_with_size(0, 40),
            Err(RuntimeError::InvalidTerminalGeometry)
        ));
        assert!(tmux.calls.borrow().is_empty());
    }

    #[test]
    fn literal_ctrl_b_targets_only_the_exact_private_provider_pane() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([successful()]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths.clone());

        runtime.send_literal_ctrl_b().unwrap();

        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].socket, paths.socket);
        assert_eq!(calls[0].config, None);
        assert_eq!(
            calls[0].arguments,
            vec![
                OsString::from("send-keys"),
                OsString::from("-t"),
                OsString::from(format!("{}:provider.0", paths.session_name)),
                OsString::from("C-b"),
            ]
        );
    }

    #[test]
    fn runtime_config_matches_the_current_owned_profile() {
        assert_eq!(
            runtime_tmux_config(),
            concat!(
                "set -g status off\n",
                "set -g mouse on\n",
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
            )
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
                stdout: "%1\n".to_owned(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: "bad-pid\n".to_owned(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: "/tmp\n".to_owned(),
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
    fn failed_session_probe_is_missing_only_with_conclusive_server_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        fs::create_dir_all(&paths.directory).unwrap();
        fs::write(&paths.socket, []).unwrap();
        let tmux = FakeTmux::with_responses([TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "can't find session: expected".to_owned(),
        }]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.probe().unwrap(),
            RuntimeProbe::Unknown { .. }
        ));
    }

    #[test]
    fn no_server_diagnostic_is_conclusive_runtime_absence() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        fs::create_dir_all(&paths.directory).unwrap();
        fs::write(&paths.socket, []).unwrap();
        let tmux = FakeTmux::with_responses([TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "no server running on the private socket".to_owned(),
        }]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert_eq!(runtime.probe().unwrap(), RuntimeProbe::Missing);
    }

    #[test]
    fn probe_uses_separator_free_single_field_queries() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::for_runtime(temporary.path(), RuntimeId::new());
        let tmux = FakeTmux::with_responses([
            successful(),
            TmuxResponse {
                success: true,
                stdout: "%1\n".to_owned(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: "42\n".to_owned(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: "/tmp\n".to_owned(),
                stderr: String::new(),
            },
        ]);
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);

        assert!(matches!(
            runtime.probe().unwrap(),
            RuntimeProbe::Live {
                pane_id,
                pane_pid: 42,
                cwd,
                process_birth: Some(_),
            } if pane_id == "%1" && cwd == Path::new("/tmp")
        ));

        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 4);
        assert!(calls[1..].iter().all(|call| {
            call.arguments
                .last()
                .is_some_and(|format| !format.to_string_lossy().contains('\t'))
        }));
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
