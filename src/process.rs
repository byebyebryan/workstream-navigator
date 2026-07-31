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
    let stdout = child
        .stdout
        .take()
        .ok_or(BoundedProcessError::MissingPipe)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(BoundedProcessError::MissingPipe)?;
    let stdout_reader = thread::spawn(move || read_capped(stdout, max_stdout_bytes));
    let stderr_reader = thread::spawn(move || read_capped(stderr, max_stderr_bytes));
    let status = wait_bounded(&mut child, timeout);
    if matches!(status, Err(BoundedProcessError::TimedOut)) {
        terminate_process_group(&mut child)?;
    }
    // Always collect both readers after the whole process group has exited, so
    // a child cannot keep a pipe full or retain an unbounded background task.
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
fn terminate_process_group(child: &mut Child) -> Result<(), BoundedProcessError> {
    use nix::{
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };

    let process_group = i32::try_from(child.id()).map_err(|_| BoundedProcessError::InvalidPid)?;
    if killpg(Pid::from_raw(process_group), Signal::SIGKILL).is_err()
        && child
            .try_wait()
            .map_err(BoundedProcessError::Wait)?
            .is_none()
    {
        child.kill().map_err(BoundedProcessError::Kill)?;
    }
    child.wait().map_err(BoundedProcessError::Wait)?;
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut Child) -> Result<(), BoundedProcessError> {
    if child
        .try_wait()
        .map_err(BoundedProcessError::Wait)?
        .is_none()
    {
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
}
