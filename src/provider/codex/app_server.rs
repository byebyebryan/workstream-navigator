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

#[cfg(unix)]
use crate::process::capture_process_group;
use crate::process::{
    ChildCleanup, ChildCleanupError, OwnedProcessGroup, ProcessGroupError, cleanup_child,
};

const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_FORK_RECOVERY_CANDIDATES: usize = 20;
const RECOVERY_THREAD_SOURCES: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

/// The only persisted fields extracted from an exact Codex thread summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadMetadata {
    pub name: Option<String>,
}

/// The one provider identifier retained from a confirmed settled-prefix fork.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkedThread {
    pub native_session_id: String,
}

/// A deliberately narrow result of reconciling an interrupted fork request.
/// No provider preview, cwd, name, prompt, response, or turn contents leave
/// the App Server adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForkReconciliation {
    Found(ForkedThread),
    Absent,
    Ambiguous,
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
    pub fn read_thread_for_hook(
        &self,
        thread_id: &str,
        deadline: Instant,
    ) -> Result<ThreadMetadata, AppServerError> {
        let result = self.request_with_deadline(
            "thread/read",
            &json!({"threadId": thread_id, "includeTurns": false}),
            deadline,
        )?;
        thread_metadata_from_result(&result, thread_id)
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

    /// Creates one destination conversation from an exact completed source
    /// turn. This is intentionally non-idempotent: callers must persist their
    /// recovery plan before invoking it and must never retry an ambiguous call.
    ///
    /// The registered project root is passed only as the provider's requested
    /// working directory. Native `codex -C … resume` remains authoritative for
    /// the destination TUI cwd.
    ///
    /// # Errors
    ///
    /// Returns an error when an input is unsafe, Codex rejects the exact fork,
    /// or the short-lived App Server cannot return one bounded destination ID.
    pub fn fork_thread(
        &self,
        source_thread_id: &str,
        last_settled_turn_id: &str,
        destination_cwd: &std::path::Path,
    ) -> Result<ForkedThread, AppServerError> {
        validate_provider_id(source_thread_id)?;
        validate_provider_id(last_settled_turn_id)?;
        let cwd = destination_cwd
            .to_str()
            .filter(|cwd| !cwd.is_empty() && cwd.len() <= 4096 && !cwd.contains(['\n', '\r', '\0']))
            .ok_or(AppServerError::InvalidForkInput)?;
        let result = self.request_with_timeout(
            "thread/fork",
            &json!({
                "threadId": source_thread_id,
                "lastTurnId": last_settled_turn_id,
                "cwd": cwd,
                "threadSource": "appServer",
            }),
            RESPONSE_TIMEOUT,
        )?;
        forked_thread_from_result(&result)
    }

    /// Reconciles one previously-recorded, non-idempotent fork request without
    /// ever sending another fork. A candidate must be provably newer than the
    /// recorded request instant, retain the exact source lineage, and end at
    /// the exact settled source turn. Seconds-only provider timestamps make a
    /// same-second candidate ambiguous by design.
    ///
    /// # Errors
    ///
    /// Returns an error when the inputs are unsafe or a bounded App Server
    /// request cannot complete. `Absent` and `Ambiguous` are successful
    /// observations that callers must convert into explicit recovery.
    pub fn reconcile_fork(
        &self,
        source_thread_id: &str,
        last_settled_turn_id: &str,
        attempted_at_millis: i64,
    ) -> Result<ForkReconciliation, AppServerError> {
        validate_provider_id(source_thread_id)?;
        validate_provider_id(last_settled_turn_id)?;
        if attempted_at_millis < 0 {
            return Err(AppServerError::InvalidForkInput);
        }
        let result = self.request_with_timeout(
            "thread/list",
            &json!({
                "archived": false,
                "limit": MAX_FORK_RECOVERY_CANDIDATES,
                "sortDirection": "desc",
                "sortKey": "created_at",
                "useStateDbOnly": true,
                "sourceKinds": RECOVERY_THREAD_SOURCES,
            }),
            RESPONSE_TIMEOUT,
        )?;
        let candidates = recovery_candidates_from_list(&result, attempted_at_millis)?;
        let mut matches = Vec::new();
        for candidate in candidates {
            let result = self.request_with_timeout(
                "thread/read",
                &json!({"threadId": candidate, "includeTurns": true}),
                RESPONSE_TIMEOUT,
            )?;
            if recovered_fork_matches(&result, source_thread_id, last_settled_turn_id, &candidate)?
            {
                matches.push(ForkedThread {
                    native_session_id: candidate,
                });
            }
        }
        match matches.len() {
            0 => Ok(ForkReconciliation::Absent),
            1 => Ok(ForkReconciliation::Found(matches.remove(0))),
            _ => Ok(ForkReconciliation::Ambiguous),
        }
    }

    fn request_with_timeout(
        &self,
        method: &str,
        params: &Value,
        response_timeout: Duration,
    ) -> Result<Value, AppServerError> {
        self.request_with_deadline_mode(method, params, Instant::now() + response_timeout, true)
    }

    fn request_with_deadline(
        &self,
        method: &str,
        params: &Value,
        deadline: Instant,
    ) -> Result<Value, AppServerError> {
        // Hook requests are the only App Server operations whose shutdown
        // cleanup is part of an already-running outer deadline. They must not
        // inherit the normal one-second graceful-exit tail.
        self.request_with_deadline_mode(method, params, deadline, false)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The bounded stdio exchange keeps deadline checks adjacent to every write, receive, and cleanup branch."
    )]
    fn request_with_deadline_mode(
        &self,
        method: &str,
        params: &Value,
        deadline: Instant,
        wait_for_exit: bool,
    ) -> Result<Value, AppServerError> {
        if deadline_expired(deadline) {
            return Err(AppServerError::Timeout);
        }
        let mut command = Command::new(&self.executable);
        command
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(AppServerError::Launch)?;
        #[cfg(unix)]
        let process_group = match capture_process_group(child.id()) {
            Ok(group) => group,
            Err(error) => {
                let _ = cleanup_child(&mut child, None);
                return Err(map_process_group_error(error));
            }
        };
        #[cfg(not(unix))]
        let process_group = None;
        if deadline_expired(deadline) {
            return cleanup_error(&mut child, process_group, AppServerError::Timeout);
        }
        let Some(mut stdin) = child.stdin.take() else {
            return cleanup_error(&mut child, process_group, AppServerError::PipesUnavailable);
        };
        let Some(stdout) = child.stdout.take() else {
            return cleanup_error(&mut child, process_group, AppServerError::PipesUnavailable);
        };
        let initialize = json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "wsnav", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {}}});
        let initialized = json!({"method": "initialized", "params": {}});
        let action = json!({"id": 2, "method": method, "params": params});
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let _ = sender.send(read_action_result(stdout));
        });
        for message in [initialize, initialized, action] {
            if deadline_expired(deadline) {
                return cleanup_with_reader(
                    &mut child,
                    process_group,
                    reader,
                    AppServerError::Timeout,
                    wait_for_exit,
                );
            }
            if let Err(error) = serde_json::to_writer(&mut stdin, &message) {
                return cleanup_with_reader(
                    &mut child,
                    process_group,
                    reader,
                    AppServerError::Encode(error),
                    wait_for_exit,
                );
            }
            if let Err(error) = stdin.write_all(b"\n") {
                return cleanup_with_reader(
                    &mut child,
                    process_group,
                    reader,
                    AppServerError::Write(error),
                    wait_for_exit,
                );
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return cleanup_with_reader(
                &mut child,
                process_group,
                reader,
                AppServerError::Timeout,
                wait_for_exit,
            );
        }
        let action_result = match receiver.recv_timeout(remaining) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return cleanup_with_reader(
                    &mut child,
                    process_group,
                    reader,
                    AppServerError::Timeout,
                    wait_for_exit,
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return cleanup_with_reader(
                    &mut child,
                    process_group,
                    reader,
                    AppServerError::Closed,
                    wait_for_exit,
                );
            }
        };
        if deadline_expired(deadline) {
            return cleanup_with_reader(
                &mut child,
                process_group,
                reader,
                AppServerError::Timeout,
                wait_for_exit,
            );
        }
        // Keep stdin open until the action result arrives. Current Codex can
        // observe EOF before dispatching a queued request if the client closes
        // it immediately after writing JSONL.
        drop(stdin);
        let wait_error = wait_for_exit.then(|| wait_for_child(&mut child)).flatten();
        let cleanup = kill_and_reap(&mut child, process_group);
        if let Err(error) = cleanup {
            drop(reader);
            return Err(error);
        }
        if wait_for_exit {
            reader.join().map_err(|_| AppServerError::Closed)?;
        } else {
            // The process group has already been terminated. Do not join a
            // hook reader after its outer deadline; joining would turn a
            // bounded observer operation into an unbounded cleanup tail.
            drop(reader);
        }
        if let Some(error) = wait_error {
            return Err(error);
        }
        action_result
    }
}

fn deadline_expired(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

fn wait_for_child(child: &mut Child) -> Option<AppServerError> {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return None,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Some(AppServerError::Launch(error)),
        }
    }
}

fn cleanup_error(
    child: &mut Child,
    process_group: Option<OwnedProcessGroup>,
    original: AppServerError,
) -> Result<Value, AppServerError> {
    kill_and_reap(child, process_group)?;
    Err(original)
}

fn cleanup_with_reader(
    child: &mut Child,
    process_group: Option<OwnedProcessGroup>,
    reader: thread::JoinHandle<()>,
    original: AppServerError,
    join_reader: bool,
) -> Result<Value, AppServerError> {
    let cleanup = kill_and_reap(child, process_group);
    if let Err(error) = cleanup {
        drop(reader);
        return Err(error);
    }
    if join_reader {
        reader.join().map_err(|_| AppServerError::Closed)?;
    } else {
        drop(reader);
    }
    Err(original)
}

fn kill_and_reap(
    child: &mut Child,
    process_group: Option<OwnedProcessGroup>,
) -> Result<(), AppServerError> {
    let cleanup = cleanup_child(child, process_group).map_err(map_child_cleanup_error)?;
    map_app_server_cleanup(cleanup)
}

fn map_app_server_cleanup(cleanup: ChildCleanup) -> Result<(), AppServerError> {
    // Preserve the App Server contract: direct-kill errors outrank wait,
    // followed by process-group proof/signal errors.
    if let Some(error) = cleanup.direct_kill {
        return Err(AppServerError::Cleanup(error));
    }
    if let Some(error) = cleanup.wait {
        return Err(AppServerError::Cleanup(error));
    }
    if let Some(error) = cleanup.process_group {
        return Err(map_process_group_error(error));
    }
    Ok(())
}

fn map_child_cleanup_error(error: ChildCleanupError) -> AppServerError {
    match error {
        ChildCleanupError::Wait(error) => AppServerError::Cleanup(error),
        #[cfg(not(unix))]
        ChildCleanupError::Kill(error) => AppServerError::Cleanup(error),
    }
}

fn map_process_group_error(error: ProcessGroupError) -> AppServerError {
    match error {
        ProcessGroupError::InvalidPid => {
            AppServerError::Cleanup(std::io::Error::other("invalid process ID"))
        }
        ProcessGroupError::Probe(error) | ProcessGroupError::Signal(error) => {
            AppServerError::Cleanup(error)
        }
    }
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

fn forked_thread_from_result(result: &Value) -> Result<ForkedThread, AppServerError> {
    let native_session_id = result
        .get("thread")
        .and_then(Value::as_object)
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .ok_or(AppServerError::InvalidResponse)?;
    validate_provider_id(native_session_id)?;
    Ok(ForkedThread {
        native_session_id: native_session_id.to_owned(),
    })
}

fn validate_provider_id(value: &str) -> Result<(), AppServerError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\n', '\r']) {
        return Err(AppServerError::InvalidForkInput);
    }
    Ok(())
}

fn recovery_candidates_from_list(
    result: &Value,
    attempted_at_millis: i64,
) -> Result<Vec<String>, AppServerError> {
    let threads = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or(AppServerError::InvalidResponse)?;
    if threads.len() > MAX_FORK_RECOVERY_CANDIDATES {
        return Err(AppServerError::InvalidResponse);
    }
    threads
        .iter()
        .filter_map(|thread| {
            let created_at = thread.get("createdAt").and_then(Value::as_i64)?;
            // Codex supplies whole seconds. A same-second candidate cannot be
            // ordered safely around our millisecond request marker, so leave
            // it out and require operator recovery instead of guessing.
            (created_at.saturating_mul(1000) > attempted_at_millis)
                .then(|| thread.get("id").and_then(Value::as_str))
                .flatten()
        })
        .map(|id| {
            validate_provider_id(id)?;
            Ok(id.to_owned())
        })
        .collect()
}

fn recovered_fork_matches(
    result: &Value,
    source_thread_id: &str,
    last_settled_turn_id: &str,
    candidate_id: &str,
) -> Result<bool, AppServerError> {
    let thread = result
        .get("thread")
        .and_then(Value::as_object)
        .ok_or(AppServerError::InvalidResponse)?;
    if thread.get("id").and_then(Value::as_str) != Some(candidate_id)
        || thread.get("forkedFromId").and_then(Value::as_str) != Some(source_thread_id)
    {
        return Ok(false);
    }
    let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
        return Err(AppServerError::InvalidResponse);
    };
    let Some(last_turn) = turns.last().and_then(Value::as_object) else {
        return Ok(false);
    };
    Ok(
        last_turn.get("id").and_then(Value::as_str) == Some(last_settled_turn_id)
            && last_turn.get("status").and_then(Value::as_str) == Some("completed"),
    )
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
    #[error("fork input is empty, unsafe, or exceeds its bound")]
    InvalidForkInput,
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
    #[error("could not clean up App Server process group")]
    Cleanup(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_server_cleanup_mapping_preserves_precedence_and_categories() {
        let result = map_app_server_cleanup(ChildCleanup {
            direct_kill: Some(std::io::Error::other("direct")),
            wait: Some(std::io::Error::other("wait")),
            process_group: Some(ProcessGroupError::Probe(std::io::Error::other("probe"))),
        });
        assert!(matches!(
            result,
            Err(AppServerError::Cleanup(error)) if error.to_string() == "direct"
        ));

        let result = map_app_server_cleanup(ChildCleanup {
            direct_kill: None,
            wait: Some(std::io::Error::other("wait")),
            process_group: Some(ProcessGroupError::Probe(std::io::Error::other("probe"))),
        });
        assert!(matches!(
            result,
            Err(AppServerError::Cleanup(error)) if error.to_string() == "wait"
        ));

        for process_group in [
            ProcessGroupError::Probe(std::io::Error::other("probe")),
            ProcessGroupError::Signal(std::io::Error::other("signal")),
        ] {
            assert!(matches!(
                map_app_server_cleanup(ChildCleanup {
                    direct_kill: None,
                    wait: None,
                    process_group: Some(process_group),
                }),
                Err(AppServerError::Cleanup(_))
            ));
        }
        assert!(matches!(
            map_app_server_cleanup(ChildCleanup {
                direct_kill: None,
                wait: None,
                process_group: Some(ProcessGroupError::InvalidPid),
            }),
            Err(AppServerError::Cleanup(error))
                if error.to_string() == "invalid process ID"
        ));
    }

    #[test]
    fn hook_deadline_is_checked_before_launching_an_app_server() {
        let server = EphemeralAppServer::new("/definitely/not-a-codex-executable");
        let deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("one-second prior instant");
        assert!(matches!(
            server.read_thread_for_hook("thread", deadline),
            Err(AppServerError::Timeout)
        ));
    }

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

    #[test]
    fn fork_result_keeps_only_the_exact_destination_identifier() {
        let result = json!({
            "thread": {
                "id": "forked-session",
                "preview": "discarded",
                "cwd": "/discarded",
                "turns": ["discarded"]
            }
        });

        assert_eq!(
            forked_thread_from_result(&result).unwrap(),
            ForkedThread {
                native_session_id: "forked-session".to_owned()
            }
        );
    }

    #[test]
    fn fork_result_rejects_missing_or_unsafe_destination_identifiers() {
        assert!(matches!(
            forked_thread_from_result(&json!({"thread": {"id": ""}})),
            Err(AppServerError::InvalidForkInput)
        ));
        assert!(matches!(
            forked_thread_from_result(&json!({"thread": {}})),
            Err(AppServerError::InvalidResponse)
        ));
    }

    #[test]
    fn fork_recovery_keeps_only_definitely_newer_bounded_identifiers() {
        let result = json!({
            "data": [
                {"id": "same-second", "createdAt": 7, "preview": "discarded"},
                {"id": "new", "createdAt": 8, "preview": "discarded"}
            ]
        });

        assert_eq!(
            recovery_candidates_from_list(&result, 7_500).unwrap(),
            vec!["new"]
        );
    }

    #[test]
    fn fork_recovery_requires_exact_lineage_and_settled_boundary() {
        let result = json!({
            "thread": {
                "id": "destination",
                "forkedFromId": "source",
                "preview": "discarded",
                "turns": [
                    {"id": "older", "status": "completed", "items": ["discarded"]},
                    {"id": "settled", "status": "completed", "items": ["discarded"]}
                ]
            }
        });
        assert!(recovered_fork_matches(&result, "source", "settled", "destination").unwrap());
        assert!(!recovered_fork_matches(&result, "other", "settled", "destination").unwrap());
        assert!(!recovered_fork_matches(&result, "source", "older", "destination").unwrap());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn app_server_cleanup_terminates_descendants_in_its_private_group() {
        use std::os::unix::process::CommandExt;
        use std::{fs, thread};

        let temporary = tempfile::tempdir().unwrap();
        let pid_path = temporary.path().join("descendant.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "sleep 30 & echo $! > '{}'",
                pid_path.to_string_lossy().replace('\'', "'\\''")
            ))
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let process_group = capture_process_group(child.id()).unwrap();
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
        assert!(kill_and_reap(&mut child, process_group).is_ok());
        assert!(
            !crate::process::linux_process_is_running(pid),
            "owned App Server descendant {pid} survived cleanup"
        );
    }
}
