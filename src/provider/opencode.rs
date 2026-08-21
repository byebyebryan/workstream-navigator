//! The bounded `OpenCode` provider adapter.
//!
//! `OpenCode` is deliberately kept behind a small, concrete adapter.  The
//! adapter owns only the loopback HTTP calls needed to create and corroborate
//! a session; it never reads `OpenCode` configuration, credentials, messages,
//! or terminal output.
use std::{
    ffi::{OsStr, OsString},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use serde_json::Value;
use thiserror::Error;

use crate::domain::ProviderSessionId;

pub use crate::provider::lifecycle::LifecycleHint;
#[cfg(test)]
use std::process::{Child, Stdio};

mod observer;
pub use observer::{
    OpenCodeObserverContext, OpenCodeObserverError, OpenCodeObserverMode, StandbyEventBuffer,
    mark_unknown_handle, run_observer, run_standby,
};
mod guardian;
pub use guardian::{run as run_guardian, run_barrier};
mod events;
pub use events::{
    OpenCodeEvent, OpenCodeEventParseError, parse_event_for_project, parse_event_hint,
    parse_event_hint_for_project, parse_event_strict,
};

pub const LOOPBACK_HOST: &str = "127.0.0.1";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const SERVE_READY_TIMEOUT: Duration = Duration::from_secs(10);
const SERVE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const SERVE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_VERSION_BYTES: usize = 4096;
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 128 * 1024;
const MAX_EVENT_BYTES: usize = 64 * 1024;

/// Read-only result of the fixed executable probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallationProbe {
    NotInstalled,
    Available,
    ProbeFailed,
}

impl InstallationProbe {
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Runs `opencode --version` with bounded output and timeout. The reported
/// release is diagnostic evidence only: compatibility is governed by the
/// bounded HTTP/SSE contract exercised by each Runtime. Missing executables
/// remain distinct from malformed or timed-out probes.
#[must_use]
pub fn probe_installation() -> InstallationProbe {
    let mut command = Command::new("opencode");
    command.arg("--version");
    let output =
        match crate::process::output_bounded(&mut command, MAX_VERSION_BYTES, MAX_VERSION_BYTES) {
            Ok(output) if output.status.success() => output,
            Ok(output) => {
                let combined = [output.stdout, output.stderr].concat();
                return classify_installation_output(&combined, false);
            }
            Err(crate::process::BoundedProcessError::Launch(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return InstallationProbe::NotInstalled;
            }
            Err(_) => return InstallationProbe::ProbeFailed,
        };
    classify_installation_output(&[output.stdout, output.stderr].concat(), true)
}

/// Deterministic seam for capability tests.  The supplied runner receives the
/// fixed executable and argument vector and returns bounded stdout/stderr and
/// an exit-success bit; no shell is involved.
pub fn probe_installation_with<F>(runner: F) -> InstallationProbe
where
    F: FnOnce(&OsStr, &[OsString]) -> Result<(bool, Vec<u8>, Vec<u8>), InstallationProbe>,
{
    let executable = OsStr::new("opencode");
    let args = [OsString::from("--version")];
    let (success, stdout, stderr) = match runner(executable, &args) {
        Ok(output) => output,
        Err(outcome) => return outcome,
    };
    if stdout.len() > MAX_VERSION_BYTES || stderr.len() > MAX_VERSION_BYTES {
        return InstallationProbe::ProbeFailed;
    }
    classify_installation_output(&[stdout, stderr].concat(), success)
}

fn classify_installation_output(output: &[u8], success: bool) -> InstallationProbe {
    let Ok(text) = std::str::from_utf8(output) else {
        return InstallationProbe::ProbeFailed;
    };
    let text = text.trim();
    if !success || text.is_empty() || text.chars().any(char::is_control) {
        return InstallationProbe::ProbeFailed;
    }
    InstallationProbe::Available
}

/// A fixed loopback endpoint for one native `OpenCode` generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeEndpoint {
    pub host: String,
    pub port: u16,
}

impl OpenCodeEndpoint {
    /// Constructs an endpoint only for the loopback host.
    ///
    /// # Errors
    ///
    /// Returns an error when `port` is zero.
    pub fn loopback(port: u16) -> Result<Self, OpenCodeError> {
        if port == 0 {
            return Err(OpenCodeError::InvalidEndpoint);
        }
        Ok(Self {
            host: LOOPBACK_HOST.to_owned(),
            port,
        })
    }

    fn address(&self) -> Result<SocketAddr, OpenCodeError> {
        if self.host != LOOPBACK_HOST || self.port == 0 {
            return Err(OpenCodeError::InvalidEndpoint);
        }
        format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|_| OpenCodeError::InvalidEndpoint)
    }
}

/// A short-lived HTTP client for one exact `OpenCode` server endpoint.
#[derive(Clone, Debug)]
pub struct OpenCodeClient {
    endpoint: OpenCodeEndpoint,
}

/// Bounded health metadata from one exact owned endpoint. The release string
/// is an opaque Runtime-generation fingerprint, not compatibility authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeHealth {
    pub version: String,
}

/// Exact status returned by the bounded `/session/status` map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenCodeSessionStatus {
    Busy,
    Idle,
    Unknown,
}

impl OpenCodeClient {
    #[must_use]
    pub fn new(endpoint: OpenCodeEndpoint) -> Self {
        Self { endpoint }
    }

    #[must_use]
    pub fn endpoint(&self) -> &OpenCodeEndpoint {
        &self.endpoint
    }

    /// Corroborates the server's bounded health contract and returns its
    /// opaque release fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint, response, or health metadata is not
    /// exact and bounded.
    pub fn health(&self) -> Result<OpenCodeHealth, OpenCodeError> {
        let body = self.json_request("GET", "/global/health", None)?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| OpenCodeError::MalformedResponse)?;
        let version = value
            .get("version")
            .and_then(Value::as_str)
            .ok_or(OpenCodeError::MalformedResponse)?;
        let healthy = value.get("healthy").and_then(Value::as_bool);
        if healthy != Some(true) {
            return Err(OpenCodeError::HealthContractMismatch);
        }
        let version = bounded_health_version(version)?;
        Ok(OpenCodeHealth { version })
    }

    /// Creates one blank session and returns only its bounded opaque ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the response is malformed or the session ID is
    /// invalid.
    pub fn create_session(&self) -> Result<ProviderSessionId, OpenCodeError> {
        let body = self.json_request("POST", "/session", Some(b"{}"))?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| OpenCodeError::MalformedResponse)?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| value.get("sessionID").and_then(Value::as_str))
            .ok_or(OpenCodeError::MalformedResponse)?;
        ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, id.to_owned())
            .map_err(|_| OpenCodeError::MalformedResponse)
    }

    /// Forks one exact settled root session at the supplied assistant message
    /// boundary.  The provider call is intentionally kept to the narrow
    /// endpoint contract: only the destination session ID is returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the source/session identity, settled message ID,
    /// HTTP response, or bounded destination payload is not exact.
    pub fn fork_session(
        &self,
        source: &ProviderSessionId,
        settled_message_id: &str,
    ) -> Result<ProviderSessionId, OpenCodeError> {
        if source.provider() != crate::domain::ProviderKind::OpenCode {
            return Err(OpenCodeError::InvalidRequest);
        }
        let source_id = url_segment(source.native_id())?;
        let settled_message_id = bounded_metadata(settled_message_id)?;
        let body = serde_json::to_vec(&serde_json::json!({
            "messageID": settled_message_id,
        }))
        .map_err(|_| OpenCodeError::InvalidRequest)?;
        let path = format!("/session/{source_id}/fork");
        let response = self.json_request("POST", &path, Some(&body))?;
        let value: Value =
            serde_json::from_slice(&response).map_err(|_| OpenCodeError::MalformedResponse)?;
        let destination = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(OpenCodeError::MalformedResponse)?;
        let destination = bounded_url_segment(destination)?;
        if destination == source.native_id() {
            return Err(OpenCodeError::SessionIdentityMismatch);
        }
        ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, destination)
            .map_err(|_| OpenCodeError::MalformedResponse)
    }

    /// Verifies that a newly-created session has no messages.  The response is
    /// discarded immediately and is never persisted or returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint response is malformed or non-empty.
    pub fn verify_blank_session(&self, session: &ProviderSessionId) -> Result<(), OpenCodeError> {
        let path = format!("/session/{}/message", url_segment(session.native_id())?);
        let body = self.json_request("GET", &path, None)?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| OpenCodeError::MalformedResponse)?;
        if value.as_array().is_some_and(Vec::is_empty) {
            Ok(())
        } else {
            Err(OpenCodeError::SessionNotBlank)
        }
    }

    /// Reads the bounded status map and returns only the exact entry for the
    /// supplied session.  This parser does not prove that the session is the
    /// expected project root, so callers that make lifecycle decisions must
    /// use [`Self::session_status_with_root`].
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint response is malformed or violates
    /// the bounded JSON contract.
    #[cfg(test)]
    fn session_status(
        &self,
        session: &ProviderSessionId,
    ) -> Result<OpenCodeSessionStatus, OpenCodeError> {
        self.session_status_map(session, OpenCodeSessionStatus::Unknown)
    }

    /// Corroborates the exact root-session metadata before interpreting the
    /// `/session/status` map.  `OpenCode` 1.18.11 omits an idle root from that
    /// map, so an absent entry is treated as idle only after the metadata
    /// proves the exact session ID, directory, and root (no parent).
    ///
    /// The metadata response is consumed locally and no provider payload is
    /// returned or persisted.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact root metadata or bounded status map is
    /// missing, malformed, or identifies another session/project.
    pub fn session_status_with_root(
        &self,
        session: &ProviderSessionId,
        expected_directory: &Path,
    ) -> Result<OpenCodeSessionStatus, OpenCodeError> {
        self.verify_root_session(session, expected_directory)?;
        self.session_status_map(session, OpenCodeSessionStatus::Idle)
    }

    pub(crate) fn verify_root_session(
        &self,
        session: &ProviderSessionId,
        expected_directory: &Path,
    ) -> Result<(), OpenCodeError> {
        let path = format!("/session/{}", url_segment(session.native_id())?);
        let body = self.json_request("GET", &path, None)?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| OpenCodeError::MalformedResponse)?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or(OpenCodeError::MalformedResponse)?;
        if id != session.native_id() {
            return Err(OpenCodeError::SessionIdentityMismatch);
        }
        let expected_directory = expected_directory
            .to_str()
            .ok_or(OpenCodeError::MalformedResponse)?;
        let directory = value
            .get("directory")
            .and_then(Value::as_str)
            .ok_or(OpenCodeError::MalformedResponse)?;
        if directory != expected_directory {
            return Err(OpenCodeError::SessionIdentityMismatch);
        }
        if value
            .get("parentID")
            .is_some_and(|parent_id| !parent_id.is_null())
        {
            return Err(OpenCodeError::SessionIdentityMismatch);
        }
        Ok(())
    }

    fn session_status_map(
        &self,
        session: &ProviderSessionId,
        absent_status: OpenCodeSessionStatus,
    ) -> Result<OpenCodeSessionStatus, OpenCodeError> {
        let body = self.json_request("GET", "/session/status", None)?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| OpenCodeError::MalformedResponse)?;
        if !value.is_object() {
            return Err(OpenCodeError::MalformedResponse);
        }
        let Some(entry) = value.get(session.native_id()) else {
            return Ok(absent_status);
        };
        if !entry.is_object() {
            return Err(OpenCodeError::MalformedResponse);
        }
        let status = entry
            .get("type")
            .and_then(Value::as_str)
            .ok_or(OpenCodeError::MalformedResponse)?;
        Ok(match status {
            // `retry` was observed as an active provider state in the accepted
            // contract. Its action/reason fields are intentionally discarded;
            // WSNav needs only the bounded fact that the exact session is not
            // idle yet.
            "busy" | "retry" => OpenCodeSessionStatus::Busy,
            "idle" => OpenCodeSessionStatus::Idle,
            _ => OpenCodeSessionStatus::Unknown,
        })
    }

    /// Opens the bounded global SSE stream.  The stream is intentionally
    /// disconnected from stdin/stdout and callers must filter exact root
    /// session/project identity before interpreting any event.
    ///
    /// # Errors
    ///
    /// Returns an error when the endpoint or bounded HTTP framing is invalid.
    pub fn event_stream(&self) -> Result<OpenCodeEventStream, OpenCodeError> {
        let address = self.endpoint.address()?;
        let mut stream =
            TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(OpenCodeError::Io)?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
            .map_err(OpenCodeError::Io)?;
        stream
            .write_all(
                b"GET /global/event HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
            )
            .map_err(OpenCodeError::Io)?;
        let mut headers = Vec::new();
        let mut byte = [0_u8; 1];
        while !headers.windows(4).any(|window| window == b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(OpenCodeError::Io)?;
            headers.push(byte[0]);
            if headers.len() > MAX_HTTP_HEADER_BYTES {
                return Err(OpenCodeError::ResponseTooLarge);
            }
        }
        let headers =
            std::str::from_utf8(&headers).map_err(|_| OpenCodeError::MalformedResponse)?;
        if !headers.lines().next().is_some_and(http_success)
            || !header_value(headers, "content-type")
                .is_some_and(|value| content_type_matches(value, "text/event-stream"))
        {
            return Err(OpenCodeError::HttpStatus);
        }
        let chunked = header_value(headers, "transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
        });
        Ok(OpenCodeEventStream {
            stream,
            event: Vec::new(),
            framing: if chunked {
                SseBodyFraming::Chunked {
                    remaining: 0,
                    need_crlf: false,
                    finished: false,
                }
            } else {
                SseBodyFraming::Direct
            },
        })
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, OpenCodeError> {
        let address = self.endpoint.address()?;
        if !path.starts_with('/') || path.len() > 4096 || path.contains(['\r', '\n']) {
            return Err(OpenCodeError::InvalidRequest);
        }
        let mut stream =
            TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(OpenCodeError::Io)?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
            .map_err(OpenCodeError::Io)?;
        let body = body.unwrap_or_default();
        if body.len() > MAX_HTTP_BODY_BYTES {
            return Err(OpenCodeError::ResponseTooLarge);
        }
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {LOOPBACK_HOST}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .and_then(|()| stream.write_all(body))
            .map_err(OpenCodeError::Io)?;
        read_http_response(&mut stream)
    }

    fn json_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>, OpenCodeError> {
        let response = self.request(method, path, body)?;
        if !response
            .content_type
            .is_some_and(|value| content_type_matches(&value, "application/json"))
        {
            return Err(OpenCodeError::ContentTypeMismatch);
        }
        Ok(response.body)
    }
}

/// Bounded line-oriented SSE reader.  It returns only one data record at a
/// time and drops comments, event names, IDs, and all other provider fields.
pub struct OpenCodeEventStream {
    stream: TcpStream,
    event: Vec<u8>,
    framing: SseBodyFraming,
}

enum SseBodyFraming {
    Direct,
    Chunked {
        remaining: usize,
        need_crlf: bool,
        finished: bool,
    },
}

impl OpenCodeEventStream {
    /// Reads one complete SSE data event.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, oversized, timed-out, or disconnected
    /// stream input.
    pub fn next_data(&mut self) -> Result<Option<Vec<u8>>, OpenCodeError> {
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match self.read_body_byte(&mut byte) {
                Ok(true) => {}
                Ok(false) => {
                    if self.event.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(std::mem::take(&mut self.event)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                    return Err(OpenCodeError::IdleTimeout);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(OpenCodeError::IdleTimeout);
                }
                Err(error) => return Err(OpenCodeError::Io(error)),
            }
            line.push(byte[0]);
            if line.len() > MAX_EVENT_BYTES {
                return Err(OpenCodeError::ResponseTooLarge);
            }
            if byte[0] != b'\n' {
                continue;
            }
            let line_bytes = line.strip_suffix(b"\n").unwrap_or(&line);
            let line_bytes = line_bytes.strip_suffix(b"\r").unwrap_or(line_bytes);
            if line_bytes.is_empty() {
                if self.event.is_empty() {
                    continue;
                }
                return Ok(Some(std::mem::take(&mut self.event)));
            }
            if let Some(data) = line_bytes.strip_prefix(b"data:") {
                if !self.event.is_empty() {
                    self.event.push(b'\n');
                }
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if self.event.len().saturating_add(data.len()) > MAX_EVENT_BYTES {
                    return Err(OpenCodeError::ResponseTooLarge);
                }
                self.event.extend_from_slice(data);
            }
            line.clear();
        }
    }

    fn read_body_byte(&mut self, byte: &mut [u8; 1]) -> Result<bool, std::io::Error> {
        match &mut self.framing {
            SseBodyFraming::Direct => match self.stream.read_exact(byte) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
                Err(error) => Err(error),
            },
            SseBodyFraming::Chunked {
                remaining,
                need_crlf,
                finished,
            } => loop {
                if *finished {
                    return Ok(false);
                }
                if *remaining > 0 {
                    self.stream.read_exact(byte)?;
                    *remaining -= 1;
                    if *remaining == 0 {
                        *need_crlf = true;
                    }
                    return Ok(true);
                }
                if *need_crlf {
                    let mut crlf = [0_u8; 2];
                    self.stream.read_exact(&mut crlf)?;
                    if crlf != *b"\r\n" {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid chunk terminator",
                        ));
                    }
                    *need_crlf = false;
                }
                let line = read_sse_chunk_line(&mut self.stream)?;
                let size_text = line.split(';').next().unwrap_or_default().trim();
                let size = usize::from_str_radix(size_text, 16).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid chunk size")
                })?;
                if size > MAX_HTTP_BODY_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "chunk exceeds bounded body",
                    ));
                }
                if size == 0 {
                    let mut trailer_bytes = 0_usize;
                    loop {
                        let trailer = read_sse_chunk_line(&mut self.stream)?;
                        trailer_bytes = trailer_bytes.saturating_add(trailer.len());
                        if trailer_bytes > MAX_HTTP_HEADER_BYTES {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "chunk trailers exceed bounded header",
                            ));
                        }
                        if trailer.is_empty() {
                            break;
                        }
                    }
                    *finished = true;
                    return Ok(false);
                }
                *remaining = size;
            },
        }
    }
}

/// Allocates one ephemeral loopback port.  The listener is dropped before the
/// native server starts; ownership is corroborated after launch, so callers
/// never search for or adopt an arbitrary existing endpoint.
///
/// # Errors
///
/// Returns an error when the operating system cannot allocate a loopback port.
pub fn reserve_loopback_port() -> Result<u16, OpenCodeError> {
    let listener = TcpListener::bind((LOOPBACK_HOST, 0)).map_err(OpenCodeError::Io)?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(OpenCodeError::Io)
}

/// Fails explicitly when a chosen loopback port is already occupied.  The
/// caller must not search for a different existing endpoint or adopt it.
///
/// # Errors
///
/// Returns an error when the endpoint is invalid or occupied.
pub fn ensure_port_available(endpoint: &OpenCodeEndpoint) -> Result<(), OpenCodeError> {
    if endpoint.host != LOOPBACK_HOST || endpoint.port == 0 {
        return Err(OpenCodeError::InvalidEndpoint);
    }
    TcpListener::bind((endpoint.host.as_str(), endpoint.port))
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                OpenCodeError::PortCollision
            } else {
                OpenCodeError::Io(error)
            }
        })
}

/// Corroborates that a loopback listener belongs to the exact provider pane
/// process (or one of its descendants).  A listener on the right port without
/// this ownership evidence is never adopted.
#[must_use]
pub fn endpoint_owned_by_process(
    endpoint: &OpenCodeEndpoint,
    pane_pid: u32,
    expected_birth: &str,
) -> bool {
    if endpoint.host != LOOPBACK_HOST || pane_pid == 0 || expected_birth.is_empty() {
        return false;
    }
    let probe = crate::runtime::LinuxProcessProbe;
    if crate::runtime::ProcessProbe::process_birth(&probe, pane_pid).as_deref()
        != Some(expected_birth)
    {
        return false;
    }
    let Some(inodes) = listener_inodes(endpoint.port) else {
        return false;
    };
    process_tree(pane_pid).into_iter().any(|pid| {
        let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let Ok(target) = std::fs::read_link(entry.path()) else {
                return false;
            };
            let target = target.to_string_lossy();
            target
                .strip_prefix("socket:[")
                .and_then(|value| value.strip_suffix(']'))
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|inode| inodes.contains(&inode))
        })
    })
}

fn listener_inodes(port: u16) -> Option<std::collections::BTreeSet<u64>> {
    let wanted = format!("0100007F:{port:04X}");
    let mut found = std::collections::BTreeSet::new();
    let Ok(text) = std::fs::read_to_string("/proc/net/tcp") else {
        return None;
    };
    for line in text.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() <= 9 || fields[3] != "0A" || fields[1] != wanted {
            continue;
        }
        if let Ok(inode) = fields[9].parse::<u64>() {
            found.insert(inode);
        }
    }
    (!found.is_empty()).then_some(found)
}

fn process_tree(root: u32) -> Vec<u32> {
    let mut tree = vec![root];
    let mut index = 0;
    while index < tree.len() {
        let parent = tree[index];
        index += 1;
        let Ok(entries) = std::fs::read_dir("/proc") else {
            break;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            if tree.contains(&pid) {
                continue;
            }
            if process_parent(pid) == Some(parent) {
                tree.push(pid);
            }
        }
    }
    tree
}

fn process_parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    stat.get(close + 2..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Builds the exact native launch vector.  No `WSNav` prompt, model, agent,
/// pure-mode, or shell expansion is permitted.
#[must_use]
pub fn native_command(
    executable: impl Into<OsString>,
    project_root: &Path,
    endpoint: &OpenCodeEndpoint,
    session: &ProviderSessionId,
) -> Vec<OsString> {
    vec![
        executable.into(),
        project_root.as_os_str().to_owned(),
        OsString::from("--hostname"),
        OsString::from(LOOPBACK_HOST),
        OsString::from("--port"),
        OsString::from(endpoint.port.to_string()),
        OsString::from("--session"),
        OsString::from(session.native_id()),
    ]
}

/// Runs the short-lived `opencode serve` precreation transaction.
///
/// # Errors
///
/// Returns an error when serving, health, session creation, blankness, or
/// conclusive process shutdown fails.
pub fn create_blank_session(
    executable: impl AsRef<OsStr>,
    project_root: &Path,
    endpoint: OpenCodeEndpoint,
) -> Result<ProviderSessionId, OpenCodeError> {
    create_blank_session_with_before_create(executable, project_root, endpoint, || Ok(()))
}

/// Runs the short-lived `opencode serve` precreation transaction and invokes
/// one typed callback after health corroboration and immediately before the
/// non-idempotent `POST /session`. The callback may durably journal the
/// provider boundary; if it fails, the provider process group is still always
/// stopped and reaped before the error is returned.
///
/// # Errors
///
/// Returns an error when serving, health, callback, session creation,
/// blankness, or conclusive process shutdown fails.
pub fn create_blank_session_with_before_create<E, F>(
    executable: impl AsRef<OsStr>,
    project_root: &Path,
    endpoint: OpenCodeEndpoint,
    before_create: F,
) -> Result<ProviderSessionId, E>
where
    E: From<OpenCodeError>,
    F: FnOnce() -> Result<(), E>,
{
    create_blank_session_with_lease(
        executable,
        project_root,
        endpoint,
        before_create,
        |executable, project_root, endpoint| {
            guardian::Lease::spawn(executable, project_root, endpoint).map_err(E::from)
        },
    )
}

trait ServeLease: Sized {
    fn wait_ready(
        &mut self,
        endpoint: &OpenCodeEndpoint,
        timeout: Duration,
    ) -> Result<(), OpenCodeError>;
    fn ensure_alive(&mut self) -> Result<(), OpenCodeError>;
    fn ensure_endpoint_owned(&self, endpoint: &OpenCodeEndpoint) -> Result<(), OpenCodeError>;
    fn close_and_wait(self) -> Result<(), OpenCodeError>;
}

impl ServeLease for guardian::Lease {
    fn wait_ready(
        &mut self,
        endpoint: &OpenCodeEndpoint,
        timeout: Duration,
    ) -> Result<(), OpenCodeError> {
        Self::wait_ready(self, endpoint, timeout)
    }

    fn ensure_alive(&mut self) -> Result<(), OpenCodeError> {
        Self::ensure_alive(self)
    }

    fn ensure_endpoint_owned(&self, endpoint: &OpenCodeEndpoint) -> Result<(), OpenCodeError> {
        Self::ensure_endpoint_owned(self, endpoint)
    }

    fn close_and_wait(self) -> Result<(), OpenCodeError> {
        Self::close_and_wait(self)
    }
}

fn create_blank_session_with_lease<E, F, L, S>(
    executable: impl AsRef<OsStr>,
    project_root: &Path,
    endpoint: OpenCodeEndpoint,
    before_create: F,
    spawn: S,
) -> Result<ProviderSessionId, E>
where
    E: From<OpenCodeError>,
    F: FnOnce() -> Result<(), E>,
    L: ServeLease,
    S: FnOnce(&OsStr, &Path, &OpenCodeEndpoint) -> Result<L, E>,
{
    let mut lease = spawn(executable.as_ref(), project_root, &endpoint)?;
    let client = OpenCodeClient::new(endpoint);
    let result = wait_for_blank_session(&mut lease, &client, before_create);
    let stop_result = lease.close_and_wait().map_err(E::from);
    match (result, stop_result) {
        (Ok(session), Ok(())) => Ok(session),
        (Err(error), Ok(())) | (Ok(_) | Err(_), Err(error)) => Err(error),
    }
}

fn wait_for_blank_session<E, F, L>(
    lease: &mut L,
    client: &OpenCodeClient,
    before_create: F,
) -> Result<ProviderSessionId, E>
where
    E: From<OpenCodeError>,
    F: FnOnce() -> Result<(), E>,
    L: ServeLease,
{
    lease
        .wait_ready(client.endpoint(), SERVE_READY_TIMEOUT)
        .map_err(E::from)?;
    let deadline = Instant::now() + SERVE_READY_TIMEOUT;
    loop {
        lease.ensure_alive().map_err(E::from)?;
        if client.health().is_ok() {
            lease.ensure_alive().map_err(E::from)?;
            lease
                .ensure_endpoint_owned(client.endpoint())
                .map_err(E::from)?;
            before_create()?;
            lease.ensure_alive().map_err(E::from)?;
            lease
                .ensure_endpoint_owned(client.endpoint())
                .map_err(E::from)?;
            let session = client.create_session().map_err(E::from)?;
            lease.ensure_alive().map_err(E::from)?;
            lease
                .ensure_endpoint_owned(client.endpoint())
                .map_err(E::from)?;
            client.verify_blank_session(&session).map_err(E::from)?;
            lease.ensure_alive().map_err(E::from)?;
            lease
                .ensure_endpoint_owned(client.endpoint())
                .map_err(E::from)?;
            return Ok(session);
        }
        if Instant::now() >= deadline {
            return Err(E::from(OpenCodeError::ServeTimedOut));
        }
        std::thread::sleep(SERVE_POLL_INTERVAL);
    }
}

fn url_segment(value: &str) -> Result<String, OpenCodeError> {
    if value.is_empty()
        || value.len() > 512
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err(OpenCodeError::InvalidRequest);
    }
    Ok(value.to_owned())
}

fn bounded_url_segment(value: &str) -> Result<String, OpenCodeError> {
    if value.len() > 256 {
        return Err(OpenCodeError::InvalidRequest);
    }
    url_segment(value)
}

fn bounded_metadata(value: &str) -> Result<String, OpenCodeError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\n', '\r']) {
        return Err(OpenCodeError::InvalidRequest);
    }
    Ok(value.to_owned())
}

fn bounded_health_version(value: &str) -> Result<String, OpenCodeError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(OpenCodeError::MalformedResponse);
    }
    Ok(value.to_owned())
}

struct HttpResponse {
    body: Vec<u8>,
    content_type: Option<String>,
}

fn read_http_response(stream: &mut TcpStream) -> Result<HttpResponse, OpenCodeError> {
    let mut bytes = Vec::with_capacity(4096);
    let mut buffer = [0_u8; 4096];
    let header_end;
    loop {
        let count = stream.read(&mut buffer).map_err(OpenCodeError::Io)?;
        if count == 0 {
            return Err(OpenCodeError::MalformedResponse);
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_HTTP_HEADER_BYTES + MAX_HTTP_BODY_BYTES {
            return Err(OpenCodeError::ResponseTooLarge);
        }
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }
    if header_end > MAX_HTTP_HEADER_BYTES {
        return Err(OpenCodeError::ResponseTooLarge);
    }
    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| OpenCodeError::MalformedResponse)?
        .to_owned();
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("HTTP/1.1 200 ") || line.starts_with("HTTP/1.0 200 "))
    {
        return Err(OpenCodeError::HttpStatus);
    }
    let content_type = header_value(&headers, "content-type").map(str::to_owned);
    let transfer_encoding = header_value(&headers, "transfer-encoding")
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mut remainder = bytes.split_off(header_end);
    let body = if transfer_encoding
        .split(',')
        .any(|value| value.trim() == "chunked")
    {
        if header_value(&headers, "content-length").is_some() {
            return Err(OpenCodeError::MalformedResponse);
        }
        read_chunked_body(stream, &mut remainder)?
    } else {
        let content_length = header_value(&headers, "content-length")
            .and_then(|value| value.trim().parse::<usize>().ok())
            .ok_or(OpenCodeError::MalformedResponse)?;
        read_fixed_body(stream, &mut remainder, content_length)?
    };
    Ok(HttpResponse { body, content_type })
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn http_success(line: &str) -> bool {
    let mut fields = line.split_whitespace();
    matches!(fields.next(), Some("HTTP/1.1" | "HTTP/1.0")) && fields.next() == Some("200")
}

fn content_type_matches(value: &str, expected: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(expected))
}

fn read_sse_chunk_line(stream: &mut TcpStream) -> Result<String, std::io::Error> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        stream.read_exact(&mut byte)?;
        line.push(byte[0]);
        if line.len() > MAX_HTTP_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "chunk line exceeds bounded header",
            ));
        }
        if byte[0] == b'\n' {
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            return String::from_utf8(line.to_vec()).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "chunk line is not UTF-8")
            });
        }
    }
}

fn read_fixed_body(
    stream: &mut TcpStream,
    remainder: &mut Vec<u8>,
    content_length: usize,
) -> Result<Vec<u8>, OpenCodeError> {
    if content_length > MAX_HTTP_BODY_BYTES {
        return Err(OpenCodeError::ResponseTooLarge);
    }
    while remainder.len() < content_length {
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer).map_err(OpenCodeError::Io)?;
        if count == 0 {
            return Err(OpenCodeError::MalformedResponse);
        }
        remainder.extend_from_slice(&buffer[..count]);
        if remainder.len() > content_length {
            return Err(OpenCodeError::MalformedResponse);
        }
    }
    Ok(remainder[..content_length].to_vec())
}

fn read_chunked_body(
    stream: &mut TcpStream,
    remainder: &mut Vec<u8>,
) -> Result<Vec<u8>, OpenCodeError> {
    let mut body = Vec::new();
    loop {
        let line = read_crlf_line(stream, remainder)?;
        let size_text = line
            .split(';')
            .next()
            .ok_or(OpenCodeError::MalformedResponse)?
            .trim();
        let size =
            usize::from_str_radix(size_text, 16).map_err(|_| OpenCodeError::MalformedResponse)?;
        if size == 0 {
            let mut trailer_bytes = 0_usize;
            loop {
                let trailer = read_crlf_line(stream, remainder)?;
                trailer_bytes = trailer_bytes.saturating_add(trailer.len());
                if trailer_bytes > MAX_HTTP_HEADER_BYTES {
                    return Err(OpenCodeError::ResponseTooLarge);
                }
                if trailer.is_empty() {
                    break;
                }
            }
            return Ok(body);
        }
        if size > MAX_HTTP_BODY_BYTES || body.len().saturating_add(size) > MAX_HTTP_BODY_BYTES {
            return Err(OpenCodeError::ResponseTooLarge);
        }
        let chunk = take_bytes(stream, remainder, size)?;
        body.extend_from_slice(&chunk);
        let crlf = take_bytes(stream, remainder, 2)?;
        if crlf.as_slice() != b"\r\n" {
            return Err(OpenCodeError::MalformedResponse);
        }
    }
}

fn read_crlf_line(
    stream: &mut TcpStream,
    remainder: &mut Vec<u8>,
) -> Result<String, OpenCodeError> {
    loop {
        if let Some(index) = remainder.windows(2).position(|window| window == b"\r\n") {
            let line = remainder.drain(..index).collect::<Vec<_>>();
            remainder.drain(..2);
            if line.len() > MAX_HTTP_HEADER_BYTES {
                return Err(OpenCodeError::ResponseTooLarge);
            }
            return String::from_utf8(line).map_err(|_| OpenCodeError::MalformedResponse);
        }
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer).map_err(OpenCodeError::Io)?;
        if count == 0 {
            return Err(OpenCodeError::MalformedResponse);
        }
        remainder.extend_from_slice(&buffer[..count]);
        if remainder.len() > MAX_HTTP_HEADER_BYTES + MAX_HTTP_BODY_BYTES {
            return Err(OpenCodeError::ResponseTooLarge);
        }
    }
}

fn take_bytes(
    stream: &mut TcpStream,
    remainder: &mut Vec<u8>,
    count: usize,
) -> Result<Vec<u8>, OpenCodeError> {
    while remainder.len() < count {
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).map_err(OpenCodeError::Io)?;
        if read == 0 {
            return Err(OpenCodeError::MalformedResponse);
        }
        remainder.extend_from_slice(&buffer[..read]);
    }
    Ok(remainder.drain(..count).collect())
}

#[derive(Debug, Error)]
pub enum OpenCodeError {
    #[error("OpenCode endpoint is invalid")]
    InvalidEndpoint,
    #[error("OpenCode request is invalid")]
    InvalidRequest,
    #[error("OpenCode response was malformed")]
    MalformedResponse,
    #[error("OpenCode response exceeded the bounded limit")]
    ResponseTooLarge,
    #[error("OpenCode returned a non-success status")]
    HttpStatus,
    #[error("OpenCode response content type was not the expected bounded type")]
    ContentTypeMismatch,
    #[error("OpenCode event stream idle read timed out")]
    IdleTimeout,
    #[error("OpenCode health response did not satisfy the adapter contract")]
    HealthContractMismatch,
    #[error("OpenCode session was not blank")]
    SessionNotBlank,
    #[error("OpenCode session identity did not match the expected root")]
    SessionIdentityMismatch,
    #[error("OpenCode loopback port is already occupied")]
    PortCollision,
    #[error("OpenCode serve exited before becoming ready ({0:?})")]
    ServeExited(Option<i32>),
    #[error("OpenCode serve did not become ready before the timeout")]
    ServeTimedOut,
    #[error("OpenCode serve process group did not terminate within the bounded timeout")]
    ServeShutdownTimedOut,
    #[error("OpenCode serve helper cleanup failed; its external state may remain active")]
    ServeCleanupFailed,
    #[error("OpenCode serve process-group identity could not be corroborated")]
    ProcessIdentityUnavailable,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ProcessGroupProbe, ProcessProbe};

    #[cfg(unix)]
    struct DirectLease {
        child: Child,
        group: crate::runtime::OwnedProcessGroup,
    }

    #[cfg(unix)]
    impl ServeLease for DirectLease {
        fn wait_ready(
            &mut self,
            _endpoint: &OpenCodeEndpoint,
            _timeout: Duration,
        ) -> Result<(), OpenCodeError> {
            Ok(())
        }

        fn ensure_alive(&mut self) -> Result<(), OpenCodeError> {
            self.child
                .try_wait()
                .map_err(OpenCodeError::Io)
                .and_then(|status| {
                    status
                        .is_none()
                        .then_some(())
                        .ok_or(OpenCodeError::ServeExited(None))
                })
        }

        fn ensure_endpoint_owned(&self, _endpoint: &OpenCodeEndpoint) -> Result<(), OpenCodeError> {
            Ok(())
        }

        fn close_and_wait(mut self) -> Result<(), OpenCodeError> {
            crate::runtime::terminate_preproven_process_group(
                &self.group,
                &crate::runtime::LinuxProcessProbe,
                &crate::runtime::LinuxProcessProbe,
                &crate::runtime::SystemProcessGroupSignaler,
                SERVE_SHUTDOWN_TIMEOUT,
                SERVE_POLL_INTERVAL,
            )
            .map_err(|_| OpenCodeError::ServeCleanupFailed)?;
            self.child.wait().map_err(OpenCodeError::Io).map(|_| ())
        }
    }

    #[cfg(unix)]
    fn direct_lease_spawn(
        executable: &OsStr,
        project_root: &Path,
        _endpoint: &OpenCodeEndpoint,
    ) -> Result<DirectLease, OpenCodeError> {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new(executable);
        command
            .arg("serve")
            .current_dir(project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().map_err(OpenCodeError::Io)?;
        let probe = crate::runtime::LinuxProcessProbe;
        let birth = probe
            .process_birth(child.id())
            .ok_or(OpenCodeError::ProcessIdentityUnavailable)?;
        let group = crate::runtime::prove_owned_process_group(child.id(), &birth, &probe, &probe)
            .map_err(|_| OpenCodeError::ProcessIdentityUnavailable)?;
        Ok(DirectLease { child, group })
    }

    #[test]
    fn installation_probe_is_version_independent_strict_and_bounded() {
        let run = |output: &str, success: bool| {
            probe_installation_with(|_, _| Ok((success, output.as_bytes().to_vec(), Vec::new())))
        };
        assert_eq!(
            run("opencode 1.18.11\n", true),
            InstallationProbe::Available
        );
        assert_eq!(run("1.19.0\n", true), InstallationProbe::Available);
        assert_eq!(
            run("development build\n", true),
            InstallationProbe::Available
        );
        assert_eq!(run("\n", true), InstallationProbe::ProbeFailed);
        assert_eq!(run("1.18.11\n", false), InstallationProbe::ProbeFailed);
    }

    #[test]
    fn native_command_contains_only_exact_resume_arguments() {
        let endpoint = OpenCodeEndpoint::loopback(4321).unwrap();
        let session =
            ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let command = native_command("opencode", Path::new("/project"), &endpoint, &session);
        assert_eq!(
            command,
            vec![
                OsString::from("opencode"),
                OsString::from("/project"),
                OsString::from("--hostname"),
                OsString::from("127.0.0.1"),
                OsString::from("--port"),
                OsString::from("4321"),
                OsString::from("--session"),
                OsString::from("root")
            ]
        );
    }

    #[test]
    fn endpoint_ownership_requires_the_exact_listener_and_process_birth() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let endpoint = OpenCodeEndpoint::loopback(port).unwrap();
        let probe = crate::runtime::LinuxProcessProbe;
        let birth =
            crate::runtime::ProcessProbe::process_birth(&probe, std::process::id()).unwrap();
        assert!(endpoint_owned_by_process(
            &endpoint,
            std::process::id(),
            &birth
        ));
        assert!(!endpoint_owned_by_process(
            &OpenCodeEndpoint::loopback(port.saturating_add(1)).unwrap(),
            std::process::id(),
            &birth,
        ));
        assert!(!endpoint_owned_by_process(
            &endpoint,
            std::process::id(),
            "stale"
        ));
        let wildcard = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let wildcard_endpoint =
            OpenCodeEndpoint::loopback(wildcard.local_addr().unwrap().port()).unwrap();
        assert!(!endpoint_owned_by_process(
            &wildcard_endpoint,
            std::process::id(),
            &birth,
        ));
    }

    #[test]
    fn bounded_http_client_accepts_health_blank_session_and_empty_messages() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                }
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                }
                let request = String::from_utf8_lossy(&request);
                let body = if request.starts_with("GET /global/health") {
                    br#"{"healthy":true,"version":"99.7.3"}"#.to_vec()
                } else if request.starts_with("POST /session") {
                    br#"{"id":"root-session"}"#.to_vec()
                } else {
                    b"[]".to_vec()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        assert_eq!(client.health().unwrap().version, "99.7.3");
        let session = client.create_session().unwrap();
        client.verify_blank_session(&session).unwrap();
        worker.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn blank_session_callback_runs_after_health_before_non_idempotent_post() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Mutex};

        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("serve.sh");
        std::fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_events = Arc::clone(&events);
        let worker = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                }
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                server_events.lock().unwrap().push(path.to_owned());
                let body = match path {
                    "/global/health" => br#"{"healthy":true,"version":"test"}"#.to_vec(),
                    "/session" => br#"{"id":"created-session"}"#.to_vec(),
                    _ => b"[]".to_vec(),
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let callback_events = Arc::clone(&events);
        let endpoint = OpenCodeEndpoint::loopback(port).unwrap();
        let session = create_blank_session_with_lease(
            &executable,
            temporary.path(),
            endpoint,
            move || {
                callback_events.lock().unwrap().push("callback".to_owned());
                Ok::<(), OpenCodeError>(())
            },
            direct_lease_spawn,
        )
        .unwrap();
        worker.join().unwrap();
        assert_eq!(session.native_id(), "created-session");
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "/global/health".to_owned(),
                "callback".to_owned(),
                "/session".to_owned(),
                "/session/created-session/message".to_owned(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn blank_session_callback_failure_still_reaps_the_private_process_group() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Mutex};

        let temporary = tempfile::tempdir().unwrap();
        let pid_path = temporary.path().join("serve.pid");
        let executable = temporary.path().join("serve.sh");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > \"{}\"\nsleep 30\n",
                pid_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let observed_identity = Arc::new(Mutex::new(None));
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..count]);
            }
            let body = br#"{"healthy":true,"version":"test"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        });
        let callback_identity = Arc::clone(&observed_identity);
        let result: Result<ProviderSessionId, OpenCodeError> = create_blank_session_with_lease(
            &executable,
            temporary.path(),
            OpenCodeEndpoint::loopback(port).unwrap(),
            move || {
                let identity = (0..100).find_map(|_| {
                    let identity = std::fs::read_to_string(&pid_path)
                        .ok()
                        .and_then(|value| value.parse::<u32>().ok())
                        .and_then(|pid| {
                            let probe = crate::runtime::LinuxProcessProbe;
                            let birth = probe.process_birth(pid)?;
                            crate::runtime::prove_owned_process_group(pid, &birth, &probe, &probe)
                                .ok()
                        });
                    identity.or_else(|| {
                        std::thread::sleep(Duration::from_millis(10));
                        None
                    })
                });
                *callback_identity.lock().unwrap() = identity;
                Err(OpenCodeError::InvalidRequest)
            },
            direct_lease_spawn,
        );
        assert!(matches!(result, Err(OpenCodeError::InvalidRequest)));
        worker.join().unwrap();
        let identity = observed_identity
            .lock()
            .unwrap()
            .clone()
            .expect("serve script recorded its process-group identity");
        let probe = crate::runtime::LinuxProcessProbe;
        assert!(
            probe
                .process_group_members_checked(&crate::runtime::ProcessGroupInfo {
                    process_group_id: identity.process_group_id,
                    session_id: identity.session_id,
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn health_contract_rejects_unhealthy_or_missing_version_metadata() {
        for body in [
            br#"{"healthy":false,"version":"99.7.3"}"#.as_slice(),
            br#"{"healthy":true,"version":""}"#.as_slice(),
            br#"{"healthy":true}"#.as_slice(),
        ] {
            let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let body = body.to_vec();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            });
            let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
            assert!(client.health().is_err());
            worker.join().unwrap();
        }
    }

    #[test]
    fn fork_session_posts_exact_message_boundary_and_returns_distinct_id() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..count]);
            }
            let headers_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap()
                + 4;
            let body_len = String::from_utf8_lossy(&request)
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while request.len() < headers_end + body_len {
                let count = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..count]);
            }
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("POST /session/root/fork HTTP/1.1"));
            let body = &request[headers_end..headers_end + body_len];
            assert_eq!(
                serde_json::from_slice::<Value>(body).unwrap(),
                serde_json::json!({"messageID": "settled-message"})
            );
            let response_body = br#"{"id":"destination"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(response_body).unwrap();
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        let source = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let destination = client.fork_session(&source, "settled-message").unwrap();
        assert_eq!(
            destination.provider(),
            crate::domain::ProviderKind::OpenCode
        );
        assert_eq!(destination.native_id(), "destination");
        worker.join().unwrap();
    }

    #[test]
    fn fork_session_rejects_same_destination_and_unsafe_boundary() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..count]);
            }
            let response_body = br#"{"id":"root"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(response_body).unwrap();
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        let source = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        assert!(matches!(
            client.fork_session(&source, "settled\nmessage"),
            Err(OpenCodeError::InvalidRequest)
        ));
        assert!(matches!(
            client.fork_session(&source, "settled-message").unwrap_err(),
            OpenCodeError::SessionIdentityMismatch
        ));
        worker.join().unwrap();
    }

    #[test]
    fn session_status_requires_the_exact_root_entry_and_ignores_other_sessions() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                }
                let body = br#"{"root":{"type":"idle"},"child":{"type":"busy"},"retrying":{"type":"retry"}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        let root = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let child = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "child").unwrap();
        let missing =
            ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "missing").unwrap();
        let retrying =
            ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "retrying").unwrap();
        assert_eq!(
            client.session_status(&root).unwrap(),
            OpenCodeSessionStatus::Idle
        );
        assert_eq!(
            client.session_status(&child).unwrap(),
            OpenCodeSessionStatus::Busy
        );
        assert_eq!(
            client.session_status(&missing).unwrap(),
            OpenCodeSessionStatus::Unknown
        );
        assert_eq!(
            client.session_status(&retrying).unwrap(),
            OpenCodeSessionStatus::Busy
        );
        worker.join().unwrap();
    }

    fn root_status_case(
        session_id: &str,
        metadata_status: u16,
        metadata_body: &str,
        status_body: &str,
        expected_directory: &Path,
    ) -> Result<OpenCodeSessionStatus, OpenCodeError> {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let metadata_body = metadata_body.as_bytes().to_vec();
        let status_body = status_body.as_bytes().to_vec();
        let expected_id = session_id.to_owned();
        let expected_directory_text = expected_directory
            .to_str()
            .expect("test directory is UTF-8")
            .to_owned();
        let expect_status = metadata_status == 200
            && serde_json::from_slice::<Value>(&metadata_body)
                .ok()
                .is_some_and(|value| {
                    value.get("id").and_then(Value::as_str) == Some(expected_id.as_str())
                        && value.get("directory").and_then(Value::as_str)
                            == Some(expected_directory_text.as_str())
                        && value.get("parentID").is_none_or(Value::is_null)
                });
        let worker = std::thread::spawn(move || {
            for _ in 0..=usize::from(expect_status) {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                }
                let request = String::from_utf8_lossy(&request);
                let (status, body) = if request.starts_with("GET /session/status") {
                    (200, status_body.as_slice())
                } else {
                    (metadata_status, metadata_body.as_slice())
                };
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        let session =
            ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, session_id).unwrap();
        let result = client.session_status_with_root(&session, expected_directory);
        worker.join().unwrap();
        result
    }

    #[test]
    fn root_session_status_requires_exact_metadata_and_fails_closed() {
        let directory = Path::new("/project");
        let metadata = r#"{"id":"root","directory":"/project"}"#;
        for (status_body, expected) in [
            (r#"{"child":{"type":"busy"}}"#, OpenCodeSessionStatus::Idle),
            (r#"{"root":{"type":"idle"}}"#, OpenCodeSessionStatus::Idle),
            (r#"{"root":{"type":"busy"}}"#, OpenCodeSessionStatus::Busy),
            (r#"{"root":{"type":"retry"}}"#, OpenCodeSessionStatus::Busy),
            (
                r#"{"root":{"type":"future-status"}}"#,
                OpenCodeSessionStatus::Unknown,
            ),
        ] {
            assert_eq!(
                root_status_case("root", 200, metadata, status_body, directory).unwrap(),
                expected
            );
        }
        assert_eq!(
            root_status_case(
                "root",
                200,
                r#"{"id":"root","directory":"/project","parentID":null}"#,
                "{}",
                directory,
            )
            .unwrap(),
            OpenCodeSessionStatus::Idle
        );
        for (metadata_status, metadata_body) in [
            (200, r#"{"id":"other","directory":"/project"}"#),
            (200, r#"{"id":"root","directory":"/other"}"#),
            (
                200,
                r#"{"id":"root","directory":"/project","parentID":"parent"}"#,
            ),
            (
                200,
                r#"{"id":"root","directory":"/project","parentID":123}"#,
            ),
            (404, "{}"),
            (200, "not-json"),
        ] {
            assert!(
                root_status_case("root", metadata_status, metadata_body, "{}", directory).is_err()
            );
        }
        for status_body in [r#"{"root":{}}"#, "[]"] {
            assert!(root_status_case("root", 200, metadata, status_body, directory).is_err());
        }
    }

    #[test]
    fn event_filter_discards_child_sessions_and_other_projects() {
        let root = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let root_event = br#"{"payload":{"type":"session.status","properties":{"sessionID":"root","status":{"type":"busy"}}}}"#;
        assert_eq!(
            parse_event_hint_for_project(root_event, &root, Some(Path::new("/project"))),
            Some(LifecycleHint::Working)
        );
        let child = br#"{"payload":{"type":"session.status","properties":{"sessionID":"child","status":{"type":"busy"}}}}"#;
        assert_eq!(parse_event_hint(child, &root), None);
        let other_project = br#"{"payload":{"type":"session.status","properties":{"sessionID":"root","directory":"/other","status":{"type":"busy"}}}}"#;
        assert_eq!(
            parse_event_hint_for_project(other_project, &root, Some(Path::new("/project"))),
            None
        );
    }

    #[test]
    fn event_parser_keeps_completed_candidate_until_exact_idle() {
        let root = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let completed = br#"{"payload":{"type":"message.updated","properties":{"sessionID":"root","info":{"id":"message-1","sessionID":"root","role":"assistant","finish":"stop","time":{"completed":1}}}}}"#;
        let candidate = parse_event_for_project(completed, &root, None).unwrap();
        assert_eq!(candidate.hint, None);
        assert_eq!(candidate.candidate_message_id.as_deref(), Some("message-1"));
        assert!(!candidate.clears_candidate);
        let busy = br#"{"payload":{"type":"session.status","properties":{"sessionID":"root","status":{"type":"busy"}}}}"#;
        let busy = parse_event_for_project(busy, &root, None).unwrap();
        assert_eq!(busy.hint, Some(LifecycleHint::Working));
        assert!(!busy.clears_candidate);
        let incomplete = br#"{"payload":{"type":"message.updated","properties":{"sessionID":"root","info":{"id":"message-2","sessionID":"root","role":"assistant"}}}}"#;
        let incomplete = parse_event_for_project(incomplete, &root, None).unwrap();
        assert_eq!(incomplete.hint, Some(LifecycleHint::Working));
        assert!(incomplete.clears_candidate);
        let retry = br#"{"payload":{"type":"session.status","properties":{"sessionID":"root","status":{"type":"retry"}}}}"#;
        assert_eq!(
            parse_event_for_project(retry, &root, None).unwrap().hint,
            Some(LifecycleHint::Working)
        );
        let idle = br#"{"payload":{"type":"session.idle","properties":{"sessionID":"root"}}}"#;
        assert_eq!(
            parse_event_for_project(idle, &root, None).unwrap().hint,
            Some(LifecycleHint::Settled { message_id: None })
        );
    }

    #[test]
    fn event_parser_requires_payload_type_and_filters_child_sessions() {
        let root = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let old_shape = br#"{"payload":{"properties":{"type":"session.idle","sessionID":"root"}}}"#;
        assert_eq!(parse_event_for_project(old_shape, &root, None), None);
        let child =
            br#"{"payload":{"type":"session.idle","properties":{"info":{"sessionID":"child"}}}}"#;
        assert_eq!(parse_event_for_project(child, &root, None), None);
        let oversized = vec![b'x'; 64 * 1024 + 1];
        assert_eq!(parse_event_for_project(&oversized, &root, None), None);
    }

    #[test]
    fn strict_parser_rejects_recognized_malformed_events_but_ignores_global_events() {
        let root = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        let malformed =
            br#"{"payload":{"type":"session.status","properties":{"sessionID":"root"}}}"#;
        assert!(parse_event_strict(malformed, &root, None).is_err());
        let global = br#"{"payload":{"type":"server.connected","properties":{}}}"#;
        assert_eq!(parse_event_strict(global, &root, None).unwrap(), None);
    }

    #[test]
    fn occupied_port_is_reported_without_endpoint_adoption() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let endpoint = OpenCodeEndpoint::loopback(listener.local_addr().unwrap().port()).unwrap();
        assert!(matches!(
            ensure_port_available(&endpoint),
            Err(OpenCodeError::PortCollision)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn preproven_group_terminates_and_reaps_the_private_process_group() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & wait"])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let probe = crate::runtime::LinuxProcessProbe;
        let birth = probe.process_birth(child.id()).unwrap();
        let identity =
            crate::runtime::prove_owned_process_group(child.id(), &birth, &probe, &probe).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        crate::runtime::terminate_preproven_process_group(
            &identity,
            &probe,
            &probe,
            &crate::runtime::SystemProcessGroupSignaler,
            SERVE_SHUTDOWN_TIMEOUT,
            SERVE_POLL_INTERVAL,
        )
        .unwrap();
        child.wait().unwrap();
        assert!(
            probe
                .process_group_members_checked(&crate::runtime::ProcessGroupInfo {
                    process_group_id: identity.process_group_id,
                    session_id: identity.session_id,
                })
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_group_membership_requires_the_captured_session() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("sleep");
        command.arg("30").process_group(0);
        let mut child = command.spawn().unwrap();
        let probe = crate::runtime::LinuxProcessProbe;
        let pid = child.id();
        let birth = probe.process_birth(pid).unwrap();
        let identity =
            crate::runtime::prove_owned_process_group(pid, &birth, &probe, &probe).unwrap();
        assert!(
            !probe
                .process_group_members_checked(&crate::runtime::ProcessGroupInfo {
                    process_group_id: identity.process_group_id,
                    session_id: identity.session_id,
                })
                .unwrap()
                .is_empty()
        );
        let different_session = crate::runtime::ProcessGroupInfo {
            process_group_id: identity.process_group_id,
            session_id: identity.session_id.wrapping_add(1),
        };
        assert!(
            probe
                .process_group_members_checked(&different_session)
                .unwrap()
                .is_empty()
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn sse_reader_discards_non_data_lines_and_bounds_records() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: keepalive\ndata: {\"sessionID\":\"root\",\"type\":\"message.updated\"}\n\n",
                )
                .unwrap();
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        let mut stream = client.event_stream().unwrap();
        assert_eq!(
            stream.next_data().unwrap(),
            Some(br#"{"sessionID":"root","type":"message.updated"}"#.to_vec())
        );
        worker.join().unwrap();
    }

    #[test]
    fn sse_reader_decodes_chunked_transfer_framing() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            let event = b"data: {\"payload\":{\"properties\":{\"sessionID\":\"root\"}}}\n\n";
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:X}\r\n",
                event.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(event).unwrap();
            stream
                .write_all(b"\r\n0\r\nTrailer: bounded\r\n\r\n")
                .unwrap();
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        let mut stream = client.event_stream().unwrap();
        assert_eq!(
            stream.next_data().unwrap(),
            Some(br#"{"payload":{"properties":{"sessionID":"root"}}}"#.to_vec())
        );
        worker.join().unwrap();
    }

    #[test]
    fn chunked_json_response_is_framed_and_bounded() {
        let listener = TcpListener::bind((LOOPBACK_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            let body = br#"{"healthy":true,"version":"contract-build"}"#;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:X}\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
            stream.write_all(b"\r\n0\r\n\r\n").unwrap();
        });
        let client = OpenCodeClient::new(OpenCodeEndpoint::loopback(port).unwrap());
        assert_eq!(client.health().unwrap().version, "contract-build");
        worker.join().unwrap();
    }
}
