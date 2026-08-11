//! Bounded local child-process output for non-provider control commands.
//!
//! Provider-terminal attachment deliberately remains a direct terminal stream.
//! This helper is only for `WSNav`'s finite tmux, Git, and child-CLI control
//! commands, where retaining unbounded output would violate the state and
//! diagnostics boundary.

use std::{
    io::{self, Read},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const WAIT_INTERVAL: Duration = Duration::from_millis(20);

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
    let process_group = match process_group_identity(child.id()) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stdout = child.stdout.take().ok_or_else(|| {
        let error = terminate_process_group(&mut child, process_group).err();
        error.unwrap_or(BoundedProcessError::MissingPipe)
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        let error = terminate_process_group(&mut child, process_group).err();
        error.unwrap_or(BoundedProcessError::MissingPipe)
    })?;
    let stdout_reader = thread::spawn(move || read_capped(stdout, max_stdout_bytes));
    let stderr_reader = thread::spawn(move || read_capped(stderr, max_stderr_bytes));
    let status = wait_bounded(&mut child, timeout);
    // Always stop and reap the whole owned process group before joining either
    // reader. This applies to timeout, wait failure, and successful direct
    // child exit: a successful parent can still leave a descendant retaining a
    // pipe open.
    let cleanup = terminate_process_group(&mut child, process_group);
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

fn wait_bounded(child: &mut Child, timeout: Duration) -> Result<ExitStatus, BoundedProcessError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(BoundedProcessError::Wait)? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(BoundedProcessError::TimedOut);
        }
        thread::sleep(WAIT_INTERVAL);
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct OwnedProcessGroup {
    process_group_id: i32,
    session_id: i32,
}

#[cfg(unix)]
fn process_group_identity(pid: u32) -> Result<Option<OwnedProcessGroup>, BoundedProcessError> {
    use nix::{errno::Errno, unistd::Pid};

    let pid = i32::try_from(pid).map_err(|_| BoundedProcessError::InvalidPid)?;
    let pid = Pid::from_raw(pid);
    let process_group_id = match nix::unistd::getpgid(Some(pid)) {
        Ok(value) => value.as_raw(),
        Err(Errno::ESRCH) => return Ok(None),
        Err(error) => {
            return Err(BoundedProcessError::ProcessGroup(
                std::io::Error::from_raw_os_error(error as i32),
            ));
        }
    };
    let session_id = nix::unistd::getsid(Some(pid)).map_err(|error| {
        BoundedProcessError::ProcessGroup(std::io::Error::from_raw_os_error(error as i32))
    })?;
    Ok(Some(OwnedProcessGroup {
        process_group_id,
        session_id: session_id.as_raw(),
    }))
}

#[cfg(unix)]
fn process_group_has_member(group: OwnedProcessGroup) -> Result<bool, BoundedProcessError> {
    let entries = std::fs::read_dir("/proc").map_err(|error| {
        BoundedProcessError::ProcessGroup(std::io::Error::new(error.kind(), error.to_string()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            BoundedProcessError::ProcessGroup(std::io::Error::new(error.kind(), error.to_string()))
        })?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(BoundedProcessError::ProcessGroup(std::io::Error::new(
                    error.kind(),
                    error.to_string(),
                )));
            }
        };
        let Some(close_paren) = stat.rfind(')') else {
            return Err(BoundedProcessError::ProcessGroup(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "malformed process stat",
            )));
        };
        let fields = stat
            .get(close_paren + 2..)
            .ok_or_else(|| {
                BoundedProcessError::ProcessGroup(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "malformed process stat",
                ))
            })?
            .split_whitespace()
            .collect::<Vec<_>>();
        let process_group_id = fields
            .get(2)
            .ok_or_else(|| {
                BoundedProcessError::ProcessGroup(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "malformed process stat",
                ))
            })?
            .parse::<i32>()
            .map_err(|_| {
                BoundedProcessError::ProcessGroup(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "malformed process stat",
                ))
            })?;
        let session_id = fields
            .get(3)
            .ok_or_else(|| {
                BoundedProcessError::ProcessGroup(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "malformed process stat",
                ))
            })?
            .parse::<i32>()
            .map_err(|_| {
                BoundedProcessError::ProcessGroup(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "malformed process stat",
                ))
            })?;
        if process_group_id == group.process_group_id && session_id == group.session_id {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn terminate_process_group(
    child: &mut Child,
    process_group: Option<OwnedProcessGroup>,
) -> Result<(), BoundedProcessError> {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };

    // The child may already have exited and been reaped by try_wait. Only
    // signal the numeric PGID when proc evidence still proves a member in the
    // original session; this avoids targeting a newly reused group ID.
    let (signal_error, group_probe_error) = if let Some(group) = process_group {
        match process_group_has_member(group) {
            Ok(true) => (
                Some(killpg(
                    Pid::from_raw(group.process_group_id),
                    Signal::SIGKILL,
                )),
                None,
            ),
            Ok(false) => (None, None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let direct_error = if child
        .try_wait()
        .map_err(BoundedProcessError::Wait)?
        .is_none()
    {
        // `Child` remains exact authority for the direct process even when
        // the captured group has become empty or changed unexpectedly. Never
        // enter an unbounded wait merely because the group scan found no
        // member.
        child.kill().err()
    } else {
        None
    };
    child.wait().map_err(BoundedProcessError::Wait)?;
    if let Some(error) = direct_error {
        return Err(BoundedProcessError::Kill(error));
    }
    if let Some(error) = signal_error.and_then(Result::err) {
        if error == nix::errno::Errno::ESRCH {
            return group_probe_error.map_or(Ok(()), Err);
        }
        return Err(BoundedProcessError::Kill(
            std::io::Error::from_raw_os_error(error as i32),
        ));
    }
    if let Some(error) = group_probe_error {
        return Err(error);
    }
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process_group(
    child: &mut Child,
    _process_group: Option<()>,
) -> Result<(), BoundedProcessError> {
    let still_live = child
        .try_wait()
        .map_err(BoundedProcessError::Wait)?
        .is_none();
    if still_live {
        child.kill().map_err(BoundedProcessError::Kill)?;
    }
    child.wait().map_err(BoundedProcessError::Wait)?;
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
