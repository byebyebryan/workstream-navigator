//! Bounded local child-process output for non-provider control commands.
//!
//! Provider-terminal attachment deliberately remains a direct terminal stream.
//! This helper is only for `WSNav`'s finite tmux, Git, and child-CLI control
//! commands, where retaining unbounded output would violate the state and
//! diagnostics boundary.

use std::{
    fs,
    io::{self, Read},
    path::Path,
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const FALLBACK_INITIAL_WAIT: Duration = Duration::from_millis(1);
const FALLBACK_MAX_WAIT: Duration = Duration::from_millis(20);

/// Runs a finite local child command while draining each output stream and
/// retaining no more than its caller-provided cap. The child and, on Unix, its
/// process group are terminated if the finite control deadline expires.
///
/// # Errors
///
/// Returns an error when the child cannot be spawned, waited for, or read, or
/// when either output stream exceeds its retained bound. Oversized streams are
/// still drained to EOF before the error is returned, so the child cannot block
/// on a full pipe.
pub fn output_bounded(
    command: &mut Command,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<Output, BoundedProcessError> {
    output_bounded_with_timeout(command, max_stdout_bytes, max_stderr_bytes, CONTROL_TIMEOUT)
}

/// Isolates a long-lived helper from a finite control command's process group.
///
/// [`output_bounded`] always terminates its owned process group after the
/// direct child exits, including on success. A deliberately disconnected
/// helper must therefore enter its own group before it can outlive that child.
pub(crate) fn isolate_long_lived_helper(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

fn output_bounded_with_timeout(
    command: &mut Command,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
    timeout: Duration,
) -> Result<Output, BoundedProcessError> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        // Match `Command::output`: control commands must never consume the
        // caller's terminal input while their output is being collected.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(BoundedProcessError::Launch)?;
    #[cfg(unix)]
    let process_group = match capture_process_group(child.id()).map_err(map_process_group_error) {
        Ok(group) => group,
        Err(error) => {
            force_cleanup_child(&mut child);
            return Err(error);
        }
    };
    #[cfg(not(unix))]
    let process_group = None;
    let stdout = child.stdout.take().ok_or_else(|| {
        let error = cleanup_bounded_child(&mut child, process_group).err();
        error.unwrap_or(BoundedProcessError::MissingPipe)
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        let error = cleanup_bounded_child(&mut child, process_group).err();
        error.unwrap_or(BoundedProcessError::MissingPipe)
    })?;
    let stdout_reader = thread::spawn(move || read_capped(stdout, max_stdout_bytes));
    let stderr_reader = thread::spawn(move || read_capped(stderr, max_stderr_bytes));
    let status = wait_bounded(&mut child, timeout);
    // Always stop and reap the whole owned process group before joining either
    // reader. This applies to timeout, wait failure, and successful direct
    // child exit: a successful parent can still leave a descendant retaining a
    // pipe open.
    let cleanup = cleanup_bounded_child(&mut child, process_group);
    if cleanup.is_err() {
        // A failed cleanup is already a fail-closed result. Do not join a
        // reader that could be held open by an unkillable descendant.
        drop(stdout_reader);
        drop(stderr_reader);
        return Err(cleanup.expect_err("checked cleanup error"));
    }
    // The process group has exited, so collecting both readers is bounded and
    // cannot leave an unbounded background task holding either pipe.
    let stdout = stdout_reader
        .join()
        .map_err(|_| BoundedProcessError::ReaderPanicked)??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| BoundedProcessError::ReaderPanicked)??;
    let status = status?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(target_os = "linux")]
fn wait_bounded(child: &mut Child, timeout: Duration) -> Result<ExitStatus, BoundedProcessError> {
    wait_bounded_with_opener(child, timeout, open_pidfd)
}

#[cfg(not(target_os = "linux"))]
fn wait_bounded(child: &mut Child, timeout: Duration) -> Result<ExitStatus, BoundedProcessError> {
    wait_with_adaptive_polling(child, Instant::now() + timeout)
}

#[cfg(target_os = "linux")]
fn wait_bounded_with_opener<F>(
    child: &mut Child,
    timeout: Duration,
    open_pidfd: F,
) -> Result<ExitStatus, BoundedProcessError>
where
    F: FnMut(&Child) -> Result<rustix::fd::OwnedFd, rustix::io::Errno>,
{
    let deadline = Instant::now() + timeout;
    match wait_with_pidfd(child, deadline, open_pidfd)? {
        PidfdWait::Completed(status) => Ok(status),
        PidfdWait::Unavailable => wait_with_adaptive_polling(child, deadline),
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
enum PidfdWait {
    Completed(ExitStatus),
    Unavailable,
}

#[cfg(target_os = "linux")]
fn wait_with_pidfd<F>(
    child: &mut Child,
    deadline: Instant,
    mut open_pidfd: F,
) -> Result<PidfdWait, BoundedProcessError>
where
    F: FnMut(&Child) -> Result<rustix::fd::OwnedFd, rustix::io::Errno>,
{
    // A pidfd is only an optional notification optimization. Any open
    // failure keeps `Child::try_wait` as the completion authority.
    let Ok(pidfd) = open_pidfd(child) else {
        return Ok(PidfdWait::Unavailable);
    };
    let mut poll_fds = [rustix::event::PollFd::new(
        &pidfd,
        rustix::event::PollFlags::IN,
    )];
    loop {
        if let Some(status) = child.try_wait().map_err(BoundedProcessError::Wait)? {
            return Ok(PidfdWait::Completed(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(BoundedProcessError::TimedOut);
        }
        let timeout = poll_timeout(remaining);
        match rustix::event::poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => {}
            Ok(_) => {
                let ready = poll_fds[0].revents();
                if ready.is_empty()
                    || ready
                        .intersects(rustix::event::PollFlags::ERR | rustix::event::PollFlags::NVAL)
                {
                    return Ok(PidfdWait::Unavailable);
                }
                // Readiness is only a notification. Re-check the direct child
                // and fall back if the notification was spurious, avoiding a
                // sticky-ready busy loop while preserving the deadline.
                if let Some(status) = child.try_wait().map_err(BoundedProcessError::Wait)? {
                    return Ok(PidfdWait::Completed(status));
                }
                return Ok(PidfdWait::Unavailable);
            }
            // EINTR and every other poll failure use the bounded fallback. A
            // notification failure must not become a new public wait error.
            Err(_) => return Ok(PidfdWait::Unavailable),
        }
    }
}

#[cfg(target_os = "linux")]
fn open_pidfd(child: &Child) -> Result<rustix::fd::OwnedFd, rustix::io::Errno> {
    rustix::process::pidfd_open(
        rustix::process::Pid::from_child(child),
        rustix::process::PidfdFlags::empty(),
    )
}

#[cfg(target_os = "linux")]
fn poll_timeout(duration: Duration) -> rustix::event::Timespec {
    let seconds = duration.as_secs();
    if seconds >= i64::MAX as u64 {
        return rustix::event::Timespec {
            tv_sec: i64::MAX,
            tv_nsec: 0,
        };
    }
    rustix::event::Timespec {
        tv_sec: i64::try_from(seconds).unwrap_or(i64::MAX),
        tv_nsec: duration.subsec_nanos().into(),
    }
}

fn wait_with_adaptive_polling(
    child: &mut Child,
    deadline: Instant,
) -> Result<ExitStatus, BoundedProcessError> {
    let mut interval = FALLBACK_INITIAL_WAIT;
    loop {
        if let Some(status) = child.try_wait().map_err(BoundedProcessError::Wait)? {
            return Ok(status);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(BoundedProcessError::TimedOut);
        }
        thread::sleep(interval.min(remaining));
        interval = next_fallback_interval(interval);
    }
}

fn next_fallback_interval(interval: Duration) -> Duration {
    interval
        .checked_mul(2)
        .unwrap_or(FALLBACK_MAX_WAIT)
        .min(FALLBACK_MAX_WAIT)
}

#[derive(Debug)]
pub(crate) enum ProcessGroupError {
    InvalidPid,
    Probe(io::Error),
    Signal(io::Error),
}

#[derive(Debug)]
pub(crate) enum ChildCleanupError {
    Wait(io::Error),
    #[cfg(not(unix))]
    Kill(io::Error),
}

#[derive(Debug)]
pub(crate) struct ChildCleanup {
    pub(crate) direct_kill: Option<io::Error>,
    pub(crate) wait: Option<io::Error>,
    pub(crate) process_group: Option<ProcessGroupError>,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct OwnedProcessGroup {
    process_group_id: i32,
    session_id: i32,
}

#[cfg(not(unix))]
pub(crate) type OwnedProcessGroup = ();

/// Captures the direct child's process-group and session identity once, before
/// any cleanup signals can be sent. An exited child is represented by `None`.
#[cfg(unix)]
pub(crate) fn capture_process_group(
    pid: u32,
) -> Result<Option<OwnedProcessGroup>, ProcessGroupError> {
    let pid = i32::try_from(pid).map_err(|_| ProcessGroupError::InvalidPid)?;
    #[cfg(target_os = "linux")]
    {
        let Some((process_group_id, session_id)) =
            read_process_group_snapshot(Path::new("/proc"), pid)?
        else {
            return Ok(None);
        };
        Ok(Some(OwnedProcessGroup {
            process_group_id,
            session_id,
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        use nix::{errno::Errno, unistd::Pid};

        let pid = Pid::from_raw(pid);
        let process_group_id = match nix::unistd::getpgid(Some(pid)) {
            Ok(value) => value.as_raw(),
            Err(Errno::ESRCH) => return Ok(None),
            Err(error) => {
                return Err(ProcessGroupError::Probe(io::Error::from_raw_os_error(
                    error as i32,
                )));
            }
        };
        let session_id = nix::unistd::getsid(Some(pid)).map_err(|error| {
            ProcessGroupError::Probe(io::Error::from_raw_os_error(error as i32))
        })?;
        Ok(Some(OwnedProcessGroup {
            process_group_id,
            session_id: session_id.as_raw(),
        }))
    }
}

#[cfg(target_os = "linux")]
fn read_process_group_snapshot(
    proc_root: &Path,
    pid: i32,
) -> Result<Option<(i32, i32)>, ProcessGroupError> {
    let metadata =
        fs::metadata(proc_root).map_err(|error| ProcessGroupError::Probe(copy_io_error(&error)))?;
    if !metadata.is_dir() {
        return Err(ProcessGroupError::Probe(io::Error::new(
            io::ErrorKind::NotADirectory,
            "process table root is not a directory",
        )));
    }
    let stat_path = proc_root.join(pid.to_string()).join("stat");
    let stat = match fs::read_to_string(stat_path) {
        Ok(stat) => stat,
        Err(error) if process_entry_missing(&error) => return Ok(None),
        Err(error) => {
            return Err(ProcessGroupError::Probe(copy_io_error(&error)));
        }
    };
    parse_process_stat(&stat)
        .map(Some)
        .map_err(ProcessGroupError::Probe)
}

#[cfg(unix)]
fn malformed_process_stat() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "malformed process stat")
}

#[cfg(unix)]
fn parse_process_stat(stat: &str) -> Result<(i32, i32), io::Error> {
    let Some(close_paren) = stat.rfind(')') else {
        return Err(malformed_process_stat());
    };
    let fields = stat
        .get(close_paren + 2..)
        .ok_or_else(malformed_process_stat)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let parse_field = |index: usize| -> Result<i32, io::Error> {
        fields
            .get(index)
            .ok_or_else(malformed_process_stat)?
            .parse::<i32>()
            .map_err(|_| malformed_process_stat())
    };
    Ok((parse_field(2)?, parse_field(3)?))
}

#[cfg(unix)]
fn process_group_has_member(group: OwnedProcessGroup) -> Result<bool, ProcessGroupError> {
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| ProcessGroupError::Probe(copy_io_error(&error)))?;
    for entry in entries {
        let entry = entry.map_err(|error| ProcessGroupError::Probe(copy_io_error(&error)))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(error) if process_entry_missing(&error) => continue,
            Err(error) => return Err(ProcessGroupError::Probe(copy_io_error(&error))),
        };
        let (process_group_id, session_id) =
            parse_process_stat(&stat).map_err(ProcessGroupError::Probe)?;
        if process_group_matches(group, process_group_id, session_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn copy_io_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

#[cfg(unix)]
fn process_entry_missing(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(3)
}

#[cfg(unix)]
fn process_group_matches(group: OwnedProcessGroup, process_group_id: i32, session_id: i32) -> bool {
    group.process_group_id == process_group_id && group.session_id == session_id
}

/// Probes whether the captured process group still exists without sending a
/// signal. `EPERM` and every other probe failure remain errors: they cannot be
/// treated as proof that cleanup is complete.
#[cfg(unix)]
fn process_group_exists(group: OwnedProcessGroup) -> Result<bool, ProcessGroupError> {
    use nix::{errno::Errno, sys::signal::killpg, unistd::Pid};

    match killpg(Pid::from_raw(group.process_group_id), None) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => Err(ProcessGroupError::Probe(io::Error::from_raw_os_error(
            error as i32,
        ))),
    }
}

#[cfg(unix)]
fn kill_process_group(group: OwnedProcessGroup) -> Result<(), ProcessGroupError> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };

    match killpg(Pid::from_raw(group.process_group_id), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(ProcessGroupError::Signal(io::Error::from_raw_os_error(
            error as i32,
        ))),
    }
}

/// Proves the captured group still belongs to the original session before
/// sending SIGKILL. An empty group is the only fast-path success. When the
/// existence probe cannot establish absence, the captured PGID+session scan
/// remains authoritative and any probe error is retained fail-closed.
#[cfg(unix)]
fn signal_process_group(group: OwnedProcessGroup) -> Result<(), ProcessGroupError> {
    signal_process_group_with(
        group,
        process_group_exists,
        process_group_has_member,
        kill_process_group,
    )
}

#[cfg(unix)]
fn signal_process_group_with<P, S, K>(
    group: OwnedProcessGroup,
    mut probe: P,
    mut has_member: S,
    mut kill: K,
) -> Result<(), ProcessGroupError>
where
    P: FnMut(OwnedProcessGroup) -> Result<bool, ProcessGroupError>,
    S: FnMut(OwnedProcessGroup) -> Result<bool, ProcessGroupError>,
    K: FnMut(OwnedProcessGroup) -> Result<(), ProcessGroupError>,
{
    let probe_error = match probe(group) {
        Ok(false) => return Ok(()),
        Ok(true) => None,
        Err(error) => Some(error),
    };
    if !has_member(group)? {
        // A probe error is not converted into success merely because the
        // scan found no member; fail closed on the uncertainty.
        return probe_error.map_or(Ok(()), Err);
    }
    match kill(group) {
        Ok(()) => probe_error.map_or(Ok(()), Err),
        Err(error) => Err(error),
    }
}

/// Performs the common direct-child and process-group cleanup mechanics while
/// leaving error precedence to each caller's public error contract.
pub(crate) fn cleanup_child(
    child: &mut Child,
    process_group: Option<OwnedProcessGroup>,
) -> Result<ChildCleanup, ChildCleanupError> {
    #[cfg(unix)]
    {
        let process_group = process_group.and_then(|group| signal_process_group(group).err());
        let direct_kill = if child.try_wait().map_err(ChildCleanupError::Wait)?.is_none() {
            // `Child` remains exact authority for the direct process even
            // when the captured group has become empty or changed.
            child.kill().err()
        } else {
            None
        };
        let wait = child.wait().err();
        Ok(ChildCleanup {
            direct_kill,
            wait,
            process_group,
        })
    }
    #[cfg(not(unix))]
    {
        if child.try_wait().map_err(ChildCleanupError::Wait)?.is_none() {
            child.kill().map_err(ChildCleanupError::Kill)?;
        }
        child.wait().map_err(ChildCleanupError::Wait)?;
        Ok(ChildCleanup {
            direct_kill: None,
            wait: None,
            process_group: None,
        })
    }
}

/// Best-effort fallback used when process-group identity capture itself fails.
/// This preserves the historical direct-child cleanup path without relying on
/// the same identity evidence that already failed.
pub(crate) fn force_cleanup_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn map_process_group_error(error: ProcessGroupError) -> BoundedProcessError {
    match error {
        ProcessGroupError::InvalidPid => BoundedProcessError::InvalidPid,
        ProcessGroupError::Probe(error) => BoundedProcessError::ProcessGroup(error),
        ProcessGroupError::Signal(error) => BoundedProcessError::Kill(error),
    }
}

fn cleanup_bounded_child(
    child: &mut Child,
    process_group: Option<OwnedProcessGroup>,
) -> Result<(), BoundedProcessError> {
    let cleanup = cleanup_child(child, process_group).map_err(|error| match error {
        ChildCleanupError::Wait(error) => BoundedProcessError::Wait(error),
        #[cfg(not(unix))]
        ChildCleanupError::Kill(error) => BoundedProcessError::Kill(error),
    })?;
    map_bounded_cleanup(cleanup)
}

fn map_bounded_cleanup(cleanup: ChildCleanup) -> Result<(), BoundedProcessError> {
    // Preserve the existing finite-child contract: wait errors take
    // precedence over captured direct-kill and process-group errors.
    if let Some(error) = cleanup.wait {
        return Err(BoundedProcessError::Wait(error));
    }
    if let Some(error) = cleanup.direct_kill {
        return Err(BoundedProcessError::Kill(error));
    }
    if let Some(error) = cleanup.process_group {
        return Err(map_process_group_error(error));
    }
    Ok(())
}

fn read_capped(mut reader: impl Read, maximum: usize) -> Result<Vec<u8>, BoundedProcessError> {
    let mut retained = Vec::with_capacity(maximum.min(4096));
    let mut buffer = [0_u8; 4096];
    let mut oversized = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(BoundedProcessError::Read)?;
        if count == 0 {
            break;
        }
        let available = maximum.saturating_sub(retained.len());
        let stored = available.min(count);
        retained.extend_from_slice(&buffer[..stored]);
        oversized |= stored != count;
    }
    if oversized {
        return Err(BoundedProcessError::OutputTooLarge);
    }
    Ok(retained)
}

/// Local bounded-process failures suitable for `WSNav` control surfaces.
#[derive(Debug, Error)]
pub enum BoundedProcessError {
    #[error("could not launch bounded child command")]
    Launch(io::Error),
    #[error("bounded child command did not expose an expected pipe")]
    MissingPipe,
    #[error("could not wait for bounded child command")]
    Wait(io::Error),
    #[error("could not stop timed-out bounded child command")]
    Kill(io::Error),
    #[error("could not verify bounded child process-group ownership")]
    ProcessGroup(io::Error),
    #[error("bounded child command exposed an invalid process ID")]
    InvalidPid,
    #[error("could not read bounded child output")]
    Read(io::Error),
    #[error("bounded child output reader panicked")]
    ReaderPanicked,
    #[error("bounded child output exceeded its configured limit")]
    OutputTooLarge,
    #[error("bounded child command exceeded its control deadline")]
    TimedOut,
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    #[cfg(unix)]
    fn process_stat_parser_uses_the_last_closing_parenthesis() {
        assert_eq!(
            parse_process_stat("42 (worker)with-parenthesis) S 1 23 17").unwrap(),
            (23, 17)
        );
    }

    #[test]
    #[cfg(unix)]
    fn process_stat_parser_rejects_missing_or_invalid_identity_fields() {
        for stat in ["42 (worker", "42 (worker) S 1", "42 (worker) S 1 bad 17"] {
            assert!(
                parse_process_stat(stat).is_err(),
                "accepted malformed stat: {stat}"
            );
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn process_group_snapshot_treats_only_missing_child_as_absence() {
        use std::fs;

        let temporary = tempfile::tempdir().unwrap();
        let proc_root = temporary.path().join("proc");
        let pid_directory = proc_root.join("42");
        fs::create_dir_all(&pid_directory).unwrap();
        assert!(matches!(
            read_process_group_snapshot(&proc_root, 42),
            Ok(None)
        ));

        fs::write(pid_directory.join("stat"), "42 (worker) S 1 bad 17").unwrap();
        assert!(matches!(
            read_process_group_snapshot(&proc_root, 42),
            Err(ProcessGroupError::Probe(error))
                if error.kind() == io::ErrorKind::InvalidData
        ));

        let proc_file = temporary.path().join("proc-file");
        fs::write(&proc_file, b"not a directory").unwrap();
        assert!(matches!(
            read_process_group_snapshot(&proc_file, 42),
            Err(ProcessGroupError::Probe(error))
                if error.kind() == io::ErrorKind::NotADirectory
        ));
    }

    #[test]
    #[cfg(unix)]
    fn process_group_identity_requires_both_group_and_session() {
        let group = OwnedProcessGroup {
            process_group_id: 23,
            session_id: 17,
        };
        assert!(process_group_matches(group, 23, 17));
        assert!(!process_group_matches(group, 23, 18));
    }

    #[test]
    #[cfg(unix)]
    fn empty_process_group_probe_skips_the_full_authority_scan() {
        let group = OwnedProcessGroup {
            process_group_id: 23,
            session_id: 17,
        };
        let mut scan_called = false;
        let mut kill_called = false;
        let result = signal_process_group_with(
            group,
            |_| Ok(false),
            |_| {
                scan_called = true;
                Ok(true)
            },
            |_| {
                kill_called = true;
                Ok(())
            },
        );
        assert!(result.is_ok());
        assert!(!scan_called);
        assert!(!kill_called);
    }

    #[test]
    #[cfg(unix)]
    fn process_group_probe_error_scans_and_fails_closed() {
        let group = OwnedProcessGroup {
            process_group_id: 23,
            session_id: 17,
        };
        let mut scan_called = false;
        let mut kill_called = false;
        let result = signal_process_group_with(
            group,
            |_| {
                Err(ProcessGroupError::Probe(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "probe denied",
                )))
            },
            |_| {
                scan_called = true;
                Ok(true)
            },
            |_| {
                kill_called = true;
                Ok(())
            },
        );
        assert!(matches!(result, Err(ProcessGroupError::Probe(_))));
        assert!(scan_called);
        assert!(kill_called);
    }

    #[test]
    fn bounded_cleanup_mapping_preserves_precedence_and_categories() {
        let result = map_bounded_cleanup(ChildCleanup {
            direct_kill: Some(std::io::Error::other("direct")),
            wait: Some(std::io::Error::other("wait")),
            process_group: Some(ProcessGroupError::Probe(std::io::Error::other("probe"))),
        });
        assert!(matches!(
            result,
            Err(BoundedProcessError::Wait(error)) if error.to_string() == "wait"
        ));

        let result = map_bounded_cleanup(ChildCleanup {
            direct_kill: Some(std::io::Error::other("direct")),
            wait: None,
            process_group: Some(ProcessGroupError::Probe(std::io::Error::other("probe"))),
        });
        assert!(matches!(
            result,
            Err(BoundedProcessError::Kill(error)) if error.to_string() == "direct"
        ));

        assert!(matches!(
            map_bounded_cleanup(ChildCleanup {
                direct_kill: None,
                wait: None,
                process_group: Some(ProcessGroupError::Probe(std::io::Error::other("probe"))),
            }),
            Err(BoundedProcessError::ProcessGroup(error)) if error.to_string() == "probe"
        ));
        assert!(matches!(
            map_bounded_cleanup(ChildCleanup {
                direct_kill: None,
                wait: None,
                process_group: Some(ProcessGroupError::Signal(std::io::Error::other("signal"))),
            }),
            Err(BoundedProcessError::Kill(error)) if error.to_string() == "signal"
        ));
        assert!(matches!(
            map_bounded_cleanup(ChildCleanup {
                direct_kill: None,
                wait: None,
                process_group: Some(ProcessGroupError::InvalidPid),
            }),
            Err(BoundedProcessError::InvalidPid)
        ));
    }

    #[test]
    fn oversized_output_is_drained_before_rejection() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 8192 /dev/zero"]);

        assert!(matches!(
            output_bounded(&mut command, 1024, 1024),
            Err(BoundedProcessError::OutputTooLarge)
        ));
    }

    #[test]
    fn stalled_process_group_is_terminated_at_the_control_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();

        assert!(matches!(
            output_bounded_with_timeout(&mut command, 1024, 1024, Duration::from_millis(100)),
            Err(BoundedProcessError::TimedOut)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn pidfd_completion_path_reaps_a_successful_child() {
        let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        let mut opener_called = false;
        let result = wait_with_pidfd(
            &mut child,
            Instant::now() + Duration::from_secs(1),
            |child| {
                opener_called = true;
                open_pidfd(child)
            },
        )
        .unwrap();
        assert!(opener_called);
        match result {
            PidfdWait::Completed(status) => assert!(status.success()),
            PidfdWait::Unavailable => {
                // Old kernels or a restrictive seccomp profile use the same
                // bounded fallback exercised below.
                assert!(child.wait().unwrap().success());
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn any_pidfd_open_error_uses_the_adaptive_fallback() {
        let mut child = Command::new("true").spawn().unwrap();
        let status = wait_bounded_with_opener(&mut child, Duration::from_secs(1), |_| {
            Err(rustix::io::Errno::ACCESS)
        })
        .unwrap();
        assert!(status.success());
    }

    #[test]
    fn adaptive_fallback_interval_is_bounded() {
        let mut interval = FALLBACK_INITIAL_WAIT;
        let mut observed = Vec::new();
        for _ in 0..8 {
            observed.push(interval);
            interval = next_fallback_interval(interval);
        }
        assert_eq!(
            observed,
            vec![
                Duration::from_millis(1),
                Duration::from_millis(2),
                Duration::from_millis(4),
                Duration::from_millis(8),
                Duration::from_millis(16),
                Duration::from_millis(20),
                Duration::from_millis(20),
                Duration::from_millis(20),
            ]
        );
        assert_eq!(
            next_fallback_interval(Duration::from_millis(19)),
            FALLBACK_MAX_WAIT
        );
    }

    #[test]
    #[cfg(unix)]
    fn successful_parent_does_not_leave_a_descendant_holding_control_pipes() {
        use std::{fs, path::Path, thread};

        let temporary = tempfile::tempdir().unwrap();
        let pid_path = temporary.path().join("descendant.pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!(
                "sleep 30 & echo $! > '{}'",
                pid_path.to_string_lossy().replace('\'', "'\\''")
            ),
        ]);
        let output = output_bounded(&mut command, 1024, 1024).unwrap();
        assert!(output.status.success());

        let pid = (0..100)
            .find_map(|_| {
                fs::read_to_string(&pid_path)
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .or_else(|| {
                        thread::sleep(Duration::from_millis(10));
                        None
                    })
            })
            .expect("fake descendant wrote its PID");
        for _ in 0..100 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("owned descendant {pid} survived process-group cleanup");
    }
}
