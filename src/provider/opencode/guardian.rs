//! Crash-surviving supervision for the short-lived `OpenCode` `serve` helper.
//!
//! A blank-session creation is a non-idempotent provider operation.  The
//! action process therefore leases the temporary server to this detached
//! helper.  Closing the anonymous stdin lease means that the action completed,
//! failed, or was killed; the guardian owns cleanup in every case.

use std::{
    env,
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use super::{
    LOOPBACK_HOST, OpenCodeEndpoint, OpenCodeError, SERVE_POLL_INTERVAL, SERVE_READY_TIMEOUT,
    SERVE_SHUTDOWN_TIMEOUT, endpoint_owned_by_process, ensure_port_available,
};
use crate::runtime::{
    LinuxProcessProbe, OwnedProcessGroup, ProcessProbe, SystemProcessGroupSignaler,
    prove_owned_process_group, terminate_preproven_process_group,
};

const GUARDIAN_COMMAND: &str = "_opencode_serve_guardian";
const BARRIER_COMMAND: &str = "_opencode_serve_barrier";
const BARRIER_RELEASE: u8 = b'R';
const READY_PREFIX: &str = "READY ";
const MAX_STATUS_BYTES: usize = 64;
const MAX_CONTROL_BYTES: usize = 64;
const CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const GUARDIAN_WAIT_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServeAuthority {
    pid: u32,
    birth: String,
    process_group_id: u32,
    session_id: u32,
}

fn ready_line(authority: &ServeAuthority) -> Result<Vec<u8>, OpenCodeError> {
    if authority.pid == 0
        || authority.birth.is_empty()
        || authority.process_group_id != authority.pid
        || authority.session_id == 0
        || authority.birth.len() > MAX_STATUS_BYTES
        || authority.birth.chars().any(char::is_whitespace)
        || authority.birth.chars().any(char::is_control)
    {
        return Err(OpenCodeError::ProcessIdentityUnavailable);
    }
    let line = format!(
        "{READY_PREFIX}{} {} {} {}\n",
        authority.pid, authority.birth, authority.process_group_id, authority.session_id
    );
    if line.len() > MAX_STATUS_BYTES {
        return Err(OpenCodeError::ProcessIdentityUnavailable);
    }
    Ok(line.into_bytes())
}

fn parse_ready_line(line: &[u8]) -> Result<ServeAuthority, OpenCodeError> {
    if line.len() > MAX_STATUS_BYTES {
        return Err(OpenCodeError::ServeCleanupFailed);
    }
    let line = std::str::from_utf8(line).map_err(|_| OpenCodeError::ServeCleanupFailed)?;
    let payload = line
        .strip_prefix(READY_PREFIX)
        .and_then(|payload| payload.strip_suffix('\n'))
        .ok_or(OpenCodeError::ServeCleanupFailed)?;
    let mut fields = payload.split(' ');
    let pid = fields.next().ok_or(OpenCodeError::ServeCleanupFailed)?;
    let birth = fields.next().ok_or(OpenCodeError::ServeCleanupFailed)?;
    let process_group_id = fields.next().ok_or(OpenCodeError::ServeCleanupFailed)?;
    let session_id = fields.next().ok_or(OpenCodeError::ServeCleanupFailed)?;
    if fields.next().is_some()
        || pid.is_empty()
        || birth.is_empty()
        || birth.chars().any(char::is_whitespace)
        || birth.chars().any(char::is_control)
    {
        return Err(OpenCodeError::ServeCleanupFailed);
    }
    let pid = pid
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)
        .ok_or(OpenCodeError::ServeCleanupFailed)?;
    let process_group_id = process_group_id
        .parse::<u32>()
        .ok()
        .filter(|group| *group == pid)
        .ok_or(OpenCodeError::ServeCleanupFailed)?;
    let session_id = session_id
        .parse::<u32>()
        .ok()
        .filter(|session| *session != 0)
        .ok_or(OpenCodeError::ServeCleanupFailed)?;
    Ok(ServeAuthority {
        pid,
        birth: birth.to_owned(),
        process_group_id,
        session_id,
    })
}

/// Runs the hidden state-free launch barrier. It reads one private release
/// byte, then `exec`s the provider in place so the captured PID/birth/group
/// identity remains exact across the provider transition.
///
/// # Errors
///
/// Returns a provider error when the release is malformed or the provider
/// cannot be replaced in this barrier process.
pub fn run_barrier(
    executable: &Path,
    project_root: &Path,
    endpoint: &OpenCodeEndpoint,
) -> Result<(), OpenCodeError> {
    if endpoint.host != LOOPBACK_HOST || endpoint.port == 0 {
        return Err(OpenCodeError::InvalidEndpoint);
    }
    let mut release = [0_u8; 1];
    let mut stdin = io::stdin().lock();
    match stdin.read(&mut release) {
        Ok(0) => Ok(()),
        Ok(1) if release[0] == BARRIER_RELEASE => {
            let mut command = Command::new(executable);
            command
                .arg("serve")
                .arg("--hostname")
                .arg(LOOPBACK_HOST)
                .arg("--port")
                .arg(endpoint.port.to_string())
                .current_dir(project_root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                Err(OpenCodeError::Io(command.exec()))
            }
            #[cfg(not(unix))]
            {
                let _ = command;
                Err(OpenCodeError::ProcessIdentityUnavailable)
            }
        }
        Ok(1) => Err(OpenCodeError::ServeCleanupFailed),
        Ok(_) => unreachable!("one-byte barrier read cannot return more than one byte"),
        Err(error) => Err(OpenCodeError::Io(error)),
    }
}

/// Runs the hidden, state-free guardian command. The command deliberately
/// does not create a `StateRoot`: it owns only the short-lived provider
/// process and communicates one bounded readiness result to its action owner.
///
/// # Errors
///
/// Returns a provider error when the helper cannot be spawned, its process
/// identity cannot be corroborated, or cleanup fails closed.
pub fn run(
    executable: &Path,
    project_root: &Path,
    endpoint: &OpenCodeEndpoint,
) -> Result<(), OpenCodeError> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let result = run_inner(executable, project_root, endpoint, &mut stdout);
    if result.is_err() {
        // A failed status write is expected when the action has already died.
        // The process-group cleanup has already run before this return.
        let _ = stdout.flush();
    }
    result
}

#[allow(clippy::too_many_lines)]
fn run_inner(
    executable: &Path,
    project_root: &Path,
    endpoint: &OpenCodeEndpoint,
    stdout: &mut impl Write,
) -> Result<(), OpenCodeError> {
    if endpoint.host != LOOPBACK_HOST || endpoint.port == 0 {
        return Err(OpenCodeError::InvalidEndpoint);
    }

    ensure_port_available(endpoint)?;
    let wsnav = env::current_exe().map_err(OpenCodeError::Io)?;
    let mut command = Command::new(wsnav);
    command
        .arg(BARRIER_COMMAND)
        .arg(executable)
        .arg(project_root)
        .arg(endpoint.port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(project_root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut helper = command.spawn().map_err(OpenCodeError::Io)?;
    let Some(release_pipe) = helper.stdin.take() else {
        let _ = helper.kill();
        let _ = helper.wait();
        return Err(OpenCodeError::ServeCleanupFailed);
    };
    let mut release = Some(release_pipe);
    let helper_pid = helper.id();
    let probe = LinuxProcessProbe;
    let helper_birth = match probe.process_birth(helper_pid) {
        Some(birth) if !birth.is_empty() => birth,
        _ => {
            // Closing the private release pipe makes the pre-exec barrier
            // exit without ever launching the provider. Do not signal an
            // identity that could not be corroborated.
            release.take();
            let _ = wait_child_bounded(&mut helper, CHILD_REAP_TIMEOUT);
            return Err(OpenCodeError::ProcessIdentityUnavailable);
        }
    };
    let Ok(group) = prove_owned_process_group(helper_pid, &helper_birth, &probe, &probe) else {
        release.take();
        let _ = wait_child_bounded(&mut helper, CHILD_REAP_TIMEOUT);
        return Err(OpenCodeError::ProcessIdentityUnavailable);
    };

    let control = io::stdin();
    let (control_tx, control_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut control = control;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 32];
        let result = loop {
            let count = match control.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => break Err(error),
            };
            if count == 0 {
                break if bytes.is_empty() {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "guardian lease contained unexpected bytes",
                    ))
                };
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > MAX_CONTROL_BYTES {
                break Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "guardian lease exceeded its bounded control size",
                ));
            }
        };
        let _ = control_tx.send(result);
    });

    // If the owner already disappeared, close the release pipe and clean up
    // only the still-blocked barrier process. No provider has been exec'd.
    match control_rx.try_recv() {
        Ok(Ok(()) | Err(_)) | Err(TryRecvError::Disconnected) => {
            return cleanup_helper(
                &mut release,
                &mut helper,
                &group,
                endpoint,
                Some(OpenCodeError::ServeCleanupFailed),
            );
        }
        Err(TryRecvError::Empty) => {}
    }

    // Recheck immediately before releasing the barrier. The captured PID,
    // birth token, process group, and session all survive the in-place exec.
    if let Err(error) = ensure_port_available(endpoint) {
        return cleanup_helper(&mut release, &mut helper, &group, endpoint, Some(error));
    }
    let Some(release_pipe) = release.as_mut() else {
        return Err(OpenCodeError::ServeCleanupFailed);
    };
    if release_pipe
        .write_all(&[BARRIER_RELEASE])
        .and_then(|()| release_pipe.flush())
        .is_err()
    {
        return cleanup_helper(
            &mut release,
            &mut helper,
            &group,
            endpoint,
            Some(OpenCodeError::ServeCleanupFailed),
        );
    }
    release.take();

    let deadline = Instant::now() + SERVE_READY_TIMEOUT;
    loop {
        match control_rx.try_recv() {
            Ok(Ok(()) | Err(_)) | Err(TryRecvError::Disconnected) => {
                return cleanup_helper(
                    &mut release,
                    &mut helper,
                    &group,
                    endpoint,
                    Some(OpenCodeError::ServeCleanupFailed),
                );
            }
            Err(TryRecvError::Empty) => {}
        }
        match helper.try_wait() {
            Ok(Some(status)) => {
                return cleanup_helper(
                    &mut release,
                    &mut helper,
                    &group,
                    endpoint,
                    Some(OpenCodeError::ServeExited(status.code())),
                );
            }
            Ok(None) => {}
            Err(_) => {
                return cleanup_helper(
                    &mut release,
                    &mut helper,
                    &group,
                    endpoint,
                    Some(OpenCodeError::ServeCleanupFailed),
                );
            }
        }
        if endpoint_owned_by_process(endpoint, helper_pid, &helper_birth) {
            break;
        }
        if Instant::now() >= deadline {
            return cleanup_helper(
                &mut release,
                &mut helper,
                &group,
                endpoint,
                Some(OpenCodeError::ServeTimedOut),
            );
        }
        thread::sleep(SERVE_POLL_INTERVAL);
    }

    match control_rx.try_recv() {
        Ok(Ok(()) | Err(_)) | Err(TryRecvError::Disconnected) => {
            return cleanup_helper(
                &mut release,
                &mut helper,
                &group,
                endpoint,
                Some(OpenCodeError::ServeCleanupFailed),
            );
        }
        Err(TryRecvError::Empty) => {}
    }

    let authority = ServeAuthority {
        pid: helper_pid,
        birth: helper_birth.clone(),
        process_group_id: group.process_group_id,
        session_id: group.session_id,
    };
    let Ok(current_group) = prove_owned_process_group(
        helper_pid,
        &helper_birth,
        &LinuxProcessProbe,
        &LinuxProcessProbe,
    ) else {
        return cleanup_helper(
            &mut release,
            &mut helper,
            &group,
            endpoint,
            Some(OpenCodeError::ProcessIdentityUnavailable),
        );
    };
    if current_group.process_group_id != group.process_group_id
        || current_group.session_id != group.session_id
    {
        return cleanup_helper(
            &mut release,
            &mut helper,
            &group,
            endpoint,
            Some(OpenCodeError::ProcessIdentityUnavailable),
        );
    }
    let ready = match ready_line(&authority) {
        Ok(ready) => ready,
        Err(error) => {
            return cleanup_helper(&mut release, &mut helper, &group, endpoint, Some(error));
        }
    };
    if stdout
        .write_all(&ready)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return cleanup_helper(&mut release, &mut helper, &group, endpoint, None);
    }

    loop {
        match control_rx.recv_timeout(SERVE_POLL_INTERVAL) {
            Ok(Ok(())) => {
                return cleanup_helper(&mut release, &mut helper, &group, endpoint, None);
            }
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => {
                return cleanup_helper(
                    &mut release,
                    &mut helper,
                    &group,
                    endpoint,
                    Some(OpenCodeError::ServeCleanupFailed),
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        match helper.try_wait() {
            Ok(Some(status)) => {
                return cleanup_helper(
                    &mut release,
                    &mut helper,
                    &group,
                    endpoint,
                    Some(OpenCodeError::ServeExited(status.code())),
                );
            }
            Ok(None) => {}
            Err(_) => {
                return cleanup_helper(
                    &mut release,
                    &mut helper,
                    &group,
                    endpoint,
                    Some(OpenCodeError::ServeCleanupFailed),
                );
            }
        }
    }
}

fn cleanup_helper(
    release: &mut Option<ChildStdin>,
    helper: &mut Child,
    group: &OwnedProcessGroup,
    endpoint: &OpenCodeEndpoint,
    original: Option<OpenCodeError>,
) -> Result<(), OpenCodeError> {
    release.take();
    let cleanup = terminate_preproven_process_group(
        group,
        &LinuxProcessProbe,
        &LinuxProcessProbe,
        &SystemProcessGroupSignaler,
        SERVE_SHUTDOWN_TIMEOUT,
        SERVE_POLL_INTERVAL,
    )
    .map(|_| ())
    .map_err(|_| OpenCodeError::ServeCleanupFailed);
    let wait = wait_child_bounded(helper, CHILD_REAP_TIMEOUT);
    let port = if cleanup.is_ok() && wait.is_ok() {
        ensure_port_available(endpoint).map_err(|_| OpenCodeError::ServeCleanupFailed)
    } else {
        Ok(())
    };
    let cleanup_error = cleanup.err().or(wait.err()).or(port.err());
    match (original, cleanup_error) {
        (Some(original), None) => Err(original),
        (Some(_) | None, Some(cleanup_error)) => Err(cleanup_error),
        (None, None) => Ok(()),
    }
}

fn wait_child_bounded(child: &mut Child, timeout: Duration) -> Result<(), OpenCodeError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(OpenCodeError::Io)? {
            Some(_) => return Ok(()),
            None if Instant::now() >= deadline => {
                return Err(OpenCodeError::ServeCleanupFailed);
            }
            None => thread::sleep(SERVE_POLL_INTERVAL),
        }
    }
}

/// The action-side lease for one detached guardian.
pub struct Lease {
    child: Child,
    owner_stdin: Option<ChildStdin>,
    status: Option<ChildStdout>,
    authority: Option<ServeAuthority>,
}

impl Lease {
    /// Spawns the current `wsnav` executable as a detached guardian.
    pub fn spawn(
        executable: impl AsRef<std::ffi::OsStr>,
        project_root: &Path,
        endpoint: &OpenCodeEndpoint,
    ) -> Result<Self, OpenCodeError> {
        let wsnav = env::current_exe().map_err(OpenCodeError::Io)?;
        let mut command = Command::new(wsnav);
        command
            .arg(GUARDIAN_COMMAND)
            .arg(executable)
            .arg(project_root)
            .arg(endpoint.port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(OpenCodeError::Io)?;
        let Some(stdin_lease) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(OpenCodeError::ServeCleanupFailed);
        };
        let Some(status) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(OpenCodeError::ServeCleanupFailed);
        };
        Ok(Self {
            child,
            owner_stdin: Some(stdin_lease),
            status: Some(status),
            authority: None,
        })
    }

    /// Waits for the exact bounded READY handshake and verifies that the
    /// guardian remains alive after publishing it.
    pub fn wait_ready(
        &mut self,
        endpoint: &OpenCodeEndpoint,
        timeout: Duration,
    ) -> Result<(), OpenCodeError> {
        let status = self
            .status
            .take()
            .ok_or(OpenCodeError::ServeCleanupFailed)?;
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let reader = BufReader::new(status);
            let mut line = Vec::new();
            let result = match reader
                .take((MAX_STATUS_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
            {
                Ok(_) => parse_ready_line(&line),
                Err(_) => Err(OpenCodeError::ServeCleanupFailed),
            };
            let _ = tx.send(result);
        });
        let deadline = Instant::now() + timeout;
        loop {
            match rx.recv_timeout(SERVE_POLL_INTERVAL) {
                Ok(authority) => {
                    self.authority = Some(authority?);
                    self.ensure_alive()?;
                    return self.ensure_endpoint_owned(endpoint);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(OpenCodeError::ServeCleanupFailed);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if let Some(status) = self.child.try_wait().map_err(OpenCodeError::Io)? {
                return Err(OpenCodeError::ServeExited(status.code()));
            }
            if Instant::now() >= deadline {
                return Err(OpenCodeError::ServeTimedOut);
            }
        }
    }

    pub fn ensure_alive(&mut self) -> Result<(), OpenCodeError> {
        if self.child.try_wait().map_err(OpenCodeError::Io)?.is_some() {
            Err(OpenCodeError::ServeCleanupFailed)
        } else {
            Ok(())
        }
    }

    pub fn ensure_endpoint_owned(&self, endpoint: &OpenCodeEndpoint) -> Result<(), OpenCodeError> {
        let authority = self
            .authority
            .as_ref()
            .ok_or(OpenCodeError::ServeCleanupFailed)?;
        let group = prove_owned_process_group(
            authority.pid,
            &authority.birth,
            &LinuxProcessProbe,
            &LinuxProcessProbe,
        )
        .map_err(|_| OpenCodeError::ServeCleanupFailed)?;
        if group.process_group_id != authority.process_group_id
            || group.session_id != authority.session_id
        {
            return Err(OpenCodeError::ServeCleanupFailed);
        }
        endpoint_owned_by_process(endpoint, authority.pid, &authority.birth)
            .then_some(())
            .ok_or(OpenCodeError::ServeCleanupFailed)
    }

    /// Closes the owner lease and waits for guardian cleanup to complete.
    pub fn close_and_wait(mut self) -> Result<(), OpenCodeError> {
        self.owner_stdin.take();
        // The guardian may spend two bounded shutdown phases (TERM and KILL)
        // before reaping its direct child. Keep this owner-side wait longer
        // than that total so a healthy cleanup is not misclassified as a
        // guardian failure.
        let deadline = Instant::now() + GUARDIAN_WAIT_TIMEOUT;
        loop {
            match self.child.try_wait().map_err(OpenCodeError::Io)? {
                Some(status) if status.success() => return Ok(()),
                Some(_) => return Err(OpenCodeError::ServeCleanupFailed),
                None if Instant::now() >= deadline => {
                    return Err(OpenCodeError::ServeCleanupFailed);
                }
                None => thread::sleep(SERVE_POLL_INTERVAL),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_are_exact_and_bounded() {
        let authority = ServeAuthority {
            pid: 42,
            birth: "123".to_owned(),
            process_group_id: 42,
            session_id: 7,
        };
        let line = ready_line(&authority).unwrap();
        assert_eq!(parse_ready_line(&line).unwrap(), authority);
        assert!(line.len() <= MAX_STATUS_BYTES);
    }

    #[test]
    fn ready_parser_rejects_ambiguous_or_oversized_authority() {
        assert!(parse_ready_line(b"READY 42 123 42 7 456\n").is_err());
        assert!(parse_ready_line(b"READY 0 123 0 7\n").is_err());
        assert!(parse_ready_line(&[b'x'; MAX_STATUS_BYTES + 1]).is_err());
        assert!(
            ready_line(&ServeAuthority {
                pid: 42,
                birth: "bad value".to_owned(),
                process_group_id: 42,
                session_id: 7,
            })
            .is_err()
        );
    }
}
