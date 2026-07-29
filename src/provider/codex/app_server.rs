//! One-shot, stdio-only Codex App Server metadata operations.

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use thiserror::Error;

const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const HOOK_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// The only persisted fields extracted from an exact Codex thread summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadMetadata {
    pub name: Option<String>,
}

/// Runs one fresh stdio server action and terminates the server before return.
#[derive(Clone, Debug)]
pub struct EphemeralAppServer {
    executable: String,
}

impl Default for EphemeralAppServer {
    fn default() -> Self {
        Self {
            executable: "codex".to_owned(),
        }
    }
}

impl EphemeralAppServer {
    /// Creates an adapter for a fixed Codex executable path.
    #[must_use]
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// Reads an exact thread without requesting its turns, items, or preview.
    ///
    /// # Errors
    ///
    /// Returns an error if the short-lived server cannot complete the bounded
    /// request or does not return a usable exact thread summary.
    pub fn read_thread(&self, thread_id: &str) -> Result<ThreadMetadata, AppServerError> {
        self.read_thread_with_timeout(thread_id, RESPONSE_TIMEOUT)
    }

    /// Reads an exact thread within the shorter budget available to a passive
    /// lifecycle hook. A timeout means no observation is committed.
    ///
    /// # Errors
    ///
    /// Returns an error if the provider-side thread cannot be corroborated
    /// before the hook's bounded execution deadline.
    pub fn read_thread_for_hook(&self, thread_id: &str) -> Result<ThreadMetadata, AppServerError> {
        self.read_thread_with_timeout(thread_id, HOOK_RESPONSE_TIMEOUT)
    }

    fn read_thread_with_timeout(
        &self,
        thread_id: &str,
        response_timeout: Duration,
    ) -> Result<ThreadMetadata, AppServerError> {
        let result = self.request_with_timeout(
            "thread/read",
            &json!({"threadId": thread_id, "includeTurns": false}),
            response_timeout,
        )?;
        thread_metadata_from_result(&result, thread_id)
    }

    /// Sets the canonical Codex-owned name of an exact managed thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the name is unsafe or the bounded one-shot request
    /// is rejected by Codex.
    pub fn set_thread_name(&self, thread_id: &str, name: &str) -> Result<(), AppServerError> {
        if name.trim().is_empty() || name.len() > 512 || name.contains(['\n', '\r']) {
            return Err(AppServerError::InvalidName);
        }
        let _ = self.request_with_timeout(
            "thread/name/set",
            &json!({"threadId": thread_id, "name": name}),
            RESPONSE_TIMEOUT,
        )?;
        Ok(())
    }

    fn request_with_timeout(
        &self,
        method: &str,
        params: &Value,
        response_timeout: Duration,
    ) -> Result<Value, AppServerError> {
        let mut child = Command::new(&self.executable)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(AppServerError::Launch)?;
        let mut stdin = child.stdin.take().ok_or(AppServerError::PipesUnavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(AppServerError::PipesUnavailable)?;
        let initialize = json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "wsnav", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}});
        let initialized = json!({"method": "initialized", "params": {}});
        let action = json!({"id": 2, "method": method, "params": params});
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = sender.send(read_action_result(stdout));
        });
        for message in [initialize, initialized, action] {
            if let Err(error) = serde_json::to_writer(&mut stdin, &message) {
                kill_and_reap(&mut child);
                return Err(AppServerError::Encode(error));
            }
            if let Err(error) = stdin.write_all(b"\n") {
                kill_and_reap(&mut child);
                return Err(AppServerError::Write(error));
            }
        }
        let action_result = match receiver.recv_timeout(response_timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                kill_and_reap(&mut child);
                return Err(AppServerError::Timeout);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                kill_and_reap(&mut child);
                return Err(AppServerError::Closed);
            }
        };
        // Keep stdin open until the action result arrives. Current Codex can
        // observe EOF before dispatching a queued request if the client closes
        // it immediately after writing JSONL.
        drop(stdin);
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while child.try_wait().map_err(AppServerError::Launch)?.is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        if child.try_wait().map_err(AppServerError::Launch)?.is_none() {
            kill_and_reap(&mut child);
        }
        action_result
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn thread_metadata_from_result(
    result: &Value,
    thread_id: &str,
) -> Result<ThreadMetadata, AppServerError> {
    let thread = result
        .get("thread")
        .and_then(Value::as_object)
        .ok_or(AppServerError::InvalidResponse)?;
    if thread.get("id").and_then(Value::as_str) != Some(thread_id) {
        return Err(AppServerError::ThreadIdentityMismatch);
    }
    let name = match thread.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if name.len() <= 512 && !name.contains(['\n', '\r']) => {
            Some(name.clone())
        }
        _ => return Err(AppServerError::InvalidResponse),
    };
    Ok(ThreadMetadata { name })
}

fn read_action_result(stdout: impl Read) -> Result<Value, AppServerError> {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::with_capacity(4096);
    let mut total = 0_usize;
    loop {
        line.clear();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(AppServerError::Read)?;
        if count == 0 {
            return Err(AppServerError::Closed);
        }
        total = total.saturating_add(count);
        if total > MAX_OUTPUT_BYTES {
            return Err(AppServerError::OutputTooLarge);
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let message: Value = serde_json::from_slice(&line).map_err(AppServerError::InvalidJson)?;
        if let Some(result) = action_result_from_message(&message) {
            return result;
        }
    }
}

fn action_result_from_message(message: &Value) -> Option<Result<Value, AppServerError>> {
    if message.get("id") != Some(&json!(2)) {
        return None;
    }
    Some(if message.get("error").is_some() {
        Err(AppServerError::Rejected)
    } else {
        message
            .get("result")
            .cloned()
            .ok_or(AppServerError::InvalidResponse)
    })
}

/// Bounded App Server failures; provider output and raw diagnostics are discarded.
#[derive(Debug, Error)]
pub enum AppServerError {
    #[error("Codex App Server closed unexpectedly")]
    Closed,
    #[error("could not encode App Server request")]
    Encode(serde_json::Error),
    #[error("App Server response was invalid JSON")]
    InvalidJson(serde_json::Error),
    #[error("App Server response did not contain an approved result")]
    InvalidResponse,
    #[error("thread name is empty or unsafe")]
    InvalidName,
    #[error("App Server did not corroborate the requested exact thread")]
    ThreadIdentityMismatch,
    #[error("could not launch or inspect App Server")]
    Launch(std::io::Error),
    #[error("App Server response exceeded the output bound")]
    OutputTooLarge,
    #[error("App Server pipes were unavailable")]
    PipesUnavailable,
    #[error("App Server rejected the request")]
    Rejected,
    #[error("App Server response timed out")]
    Timeout,
    #[error("could not read App Server output")]
    Read(std::io::Error),
    #[error("could not write App Server input")]
    Write(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_action_result_is_accepted_without_waiting_for_eof() {
        let output =
            b"{\"id\":1,\"result\":{}}\n{\"id\":2,\"result\":{\"thread\":{\"name\":\"name\"}}}\n";
        assert_eq!(
            read_action_result(&output[..]).unwrap()["thread"]["name"],
            "name"
        );
    }

    #[test]
    fn missing_action_result_fails_closed_after_stream_end() {
        assert!(matches!(
            read_action_result(&b"{\"id\":1,\"result\":{}}\n"[..]),
            Err(AppServerError::Closed)
        ));
    }

    #[test]
    fn action_error_is_not_treated_as_a_successful_result() {
        assert!(matches!(
            read_action_result(&b"{\"id\":2,\"error\":{\"code\":-1}}\n"[..]),
            Err(AppServerError::Rejected)
        ));
    }

    #[test]
    fn exact_thread_read_rejects_a_mismatched_provider_identity() {
        let result = json!({"thread": {"id": "different", "name": null}});
        assert!(matches!(
            thread_metadata_from_result(&result, "expected"),
            Err(AppServerError::ThreadIdentityMismatch)
        ));
    }

    #[test]
    fn exact_thread_read_keeps_only_the_bounded_name_field() {
        let result = json!({
            "thread": {
                "id": "expected",
                "name": "Native name",
                "preview": "discarded",
                "cwd": "/discarded"
            }
        });
        assert_eq!(
            thread_metadata_from_result(&result, "expected").unwrap(),
            ThreadMetadata {
                name: Some("Native name".to_owned())
            }
        );
    }
}
