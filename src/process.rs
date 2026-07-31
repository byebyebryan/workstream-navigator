//! Bounded local child-process output for non-provider control commands.
//!
//! Provider-terminal attachment deliberately remains a direct terminal stream.
//! This helper is only for `WSNav`'s finite tmux, Git, and child-CLI control
//! commands, where retaining unbounded output would violate the state and
//! diagnostics boundary.

use std::{
    io::{self, Read},
    process::{Command, Output, Stdio},
    thread,
};

use thiserror::Error;

/// Runs a finite local child command while draining each output stream and
/// retaining no more than its caller-provided cap.
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
    // Wait first, then always collect both readers. In particular, do not
    // abandon a draining thread if waiting reports an operating-system error.
    let status = child.wait();
    let stdout = stdout_reader
        .join()
        .map_err(|_| BoundedProcessError::ReaderPanicked)??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| BoundedProcessError::ReaderPanicked)??;
    let status = status.map_err(BoundedProcessError::Wait)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
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
    #[error("could not read bounded child output")]
    Read(io::Error),
    #[error("bounded child output reader panicked")]
    ReaderPanicked,
    #[error("bounded child output exceeded its configured limit")]
    OutputTooLarge,
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
}
