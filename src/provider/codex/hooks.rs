//! Passive, fail-closed Codex lifecycle-hook ingestion.

use std::{io::Read, time::Instant};

use serde::Deserialize;
use thiserror::Error;

use crate::provider::lifecycle::{LifecycleEvent, LifecycleObservation};

const MAX_HOOK_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;

/// Reads all hook stdin before parsing a bounded prefix.
///
/// The returned error is intentionally non-fatal to Codex callers. It proves
/// stdin was drained, so a large provider payload cannot receive `SIGPIPE`
/// merely because `WSNav` rejects the event.
///
/// # Errors
///
/// Returns an error after draining input if reading fails, the input exceeds the
/// bounded parse limit, or the retained prefix is malformed.
pub fn drain_and_parse(input: &mut impl Read) -> Result<LifecycleObservation, HookError> {
    drain_and_parse_with_deadline(input, None)
}

/// Reads and parses hook stdin until an absolute preparation deadline.
///
/// The deadline is checked before and after each bounded read. A slow or
/// unclosed provider pipe therefore cannot consume the caller's remaining
/// preparation budget between payload chunks.
///
/// # Errors
///
/// Returns [`HookError::DeadlineExceeded`] when the absolute deadline expires
/// during input drain, or the same bounded parsing errors as
/// [`drain_and_parse`] after a complete read.
pub fn drain_and_parse_until(
    input: &mut impl Read,
    deadline: Instant,
) -> Result<LifecycleObservation, HookError> {
    drain_and_parse_with_deadline(input, Some(deadline))
}

/// Drains the process stdin with readiness polling on Linux so an unclosed
/// provider pipe cannot block a hook past its absolute deadline.
///
/// # Errors
///
/// Returns a bounded hook parsing error when polling, reading, or parsing the
/// input fails, including [`HookError::DeadlineExceeded`] on timeout.
#[cfg(target_os = "linux")]
pub fn drain_stdin_and_parse_until(deadline: Instant) -> Result<LifecycleObservation, HookError> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut retained = Vec::with_capacity(4096);
    let mut total = 0_usize;
    let mut chunk = [0_u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err(HookError::DeadlineExceeded);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = rustix::event::Timespec {
            tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: remaining.subsec_nanos().into(),
        };
        let mut poll_fds = [rustix::event::PollFd::new(
            &input,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) | Err(_) => return Err(HookError::DeadlineExceeded),
            Ok(_) => {}
        }
        let count = input.read(&mut chunk).map_err(HookError::Read)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count);
        if retained.len() < MAX_HOOK_PAYLOAD_BYTES {
            let available = MAX_HOOK_PAYLOAD_BYTES - retained.len();
            retained.extend_from_slice(&chunk[..count.min(available)]);
        }
    }
    parse_retained_payload(&retained, total)
}

/// Portable fallback for targets without the Linux readiness API. The generic
/// deadline checks still bound slow readers; platform-specific stdin polling is
/// used on Linux where Codex hooks run in production.
#[cfg(not(target_os = "linux"))]
pub fn drain_stdin_and_parse_until(deadline: Instant) -> Result<LifecycleObservation, HookError> {
    let stdin = std::io::stdin();
    drain_and_parse_until(&mut stdin.lock(), deadline)
}

fn drain_and_parse_with_deadline(
    input: &mut impl Read,
    deadline: Option<Instant>,
) -> Result<LifecycleObservation, HookError> {
    let mut retained = Vec::with_capacity(4096);
    let mut total = 0_usize;
    let mut chunk = [0_u8; 8192];
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(HookError::DeadlineExceeded);
        }
        let count = input.read(&mut chunk).map_err(HookError::Read)?;
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(HookError::DeadlineExceeded);
        }
        if count == 0 {
            break;
        }
        total = total.saturating_add(count);
        if retained.len() < MAX_HOOK_PAYLOAD_BYTES {
            let available = MAX_HOOK_PAYLOAD_BYTES - retained.len();
            retained.extend_from_slice(&chunk[..count.min(available)]);
        }
    }
    parse_retained_payload(&retained, total)
}

fn parse_retained_payload(
    retained: &[u8],
    total: usize,
) -> Result<LifecycleObservation, HookError> {
    if total > MAX_HOOK_PAYLOAD_BYTES {
        return Err(HookError::PayloadTooLarge);
    }
    let payload: HookPayload = serde_json::from_slice(retained).map_err(HookError::Malformed)?;
    LifecycleObservation::try_from(payload)
}

#[derive(Deserialize)]
struct HookPayload {
    hook_event_name: String,
    cwd: String,
    session_id: String,
    turn_id: Option<String>,
    source: Option<String>,
    reason: Option<String>,
}

impl TryFrom<HookPayload> for LifecycleObservation {
    type Error = HookError;

    fn try_from(value: HookPayload) -> Result<Self, Self::Error> {
        let event = match value.hook_event_name.as_str() {
            "SessionStart" => LifecycleEvent::SessionStart,
            "UserPromptSubmit" => LifecycleEvent::UserPromptSubmit,
            "Stop" => LifecycleEvent::Stop,
            "SessionEnd" => LifecycleEvent::SessionEnd,
            _ => return Err(HookError::UnsupportedEvent),
        };
        validate_field("cwd", &value.cwd)?;
        validate_field("session ID", &value.session_id)?;
        if let Some(turn_id) = &value.turn_id {
            validate_field("turn ID", turn_id)?;
        }
        let source = value.source.or(value.reason);
        if let Some(source) = &source {
            validate_field("source", source)?;
        }
        Ok(Self {
            event,
            cwd: value.cwd,
            native_session_id: value.session_id,
            turn_id: value.turn_id,
            source,
        })
    }
}

fn validate_field(name: &'static str, value: &str) -> Result<(), HookError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(HookError::InvalidField(name));
    }
    if value.contains('\n') || value.contains('\r') || value.contains('\0') {
        return Err(HookError::InvalidField(name));
    }
    Ok(())
}

/// Observer parsing failures. Internal hook command callers always convert them to success.
#[derive(Debug, Error)]
pub enum HookError {
    #[error("invalid {0}")]
    InvalidField(&'static str),
    #[error("hook payload is malformed")]
    Malformed(serde_json::Error),
    #[error("hook payload exceeds the bounded parse limit")]
    PayloadTooLarge,
    #[error("hook stdin could not be read")]
    Read(std::io::Error),
    #[error("hook preparation deadline expired while reading stdin")]
    DeadlineExceeded,
    #[error("unsupported lifecycle event")]
    UnsupportedEvent,
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Cursor, Read},
        time::Duration,
    };

    use super::*;

    #[test]
    fn oversized_unmanaged_input_is_fully_drained_before_rejection() {
        let payload = vec![b'x'; MAX_HOOK_PAYLOAD_BYTES + 1024];
        let mut input = Cursor::new(payload);

        assert!(matches!(
            drain_and_parse(&mut input),
            Err(HookError::PayloadTooLarge)
        ));
        assert_eq!(input.position(), (MAX_HOOK_PAYLOAD_BYTES + 1024) as u64);
    }

    #[test]
    fn prompt_content_is_discarded_from_the_observation_shape() {
        let mut input = Cursor::new(
            br#"{"hook_event_name":"Stop","cwd":"/repo","session_id":"session","turn_id":"turn","prompt":"secret"}"#,
        );
        let observation: LifecycleObservation = drain_and_parse(&mut input).unwrap();

        assert_eq!(observation.event, LifecycleEvent::Stop);
        assert_eq!(observation.native_session_id, "session");
        assert_eq!(observation.turn_id.as_deref(), Some("turn"));
    }

    #[test]
    fn unknown_events_fail_closed() {
        let mut input = Cursor::new(
            br#"{"hook_event_name":"PostToolUse","cwd":"/repo","session_id":"session"}"#,
        );

        assert!(matches!(
            drain_and_parse(&mut input),
            Err(HookError::UnsupportedEvent)
        ));
    }

    struct SlowReader {
        payload: Vec<u8>,
        delay: Duration,
        read: bool,
    }

    impl Read for SlowReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.read {
                return Ok(0);
            }
            std::thread::sleep(self.delay);
            self.read = true;
            let count = self.payload.len().min(output.len());
            output[..count].copy_from_slice(&self.payload[..count]);
            Ok(count)
        }
    }

    #[test]
    fn slow_input_expires_before_payload_parse() {
        let mut input = SlowReader {
            payload: br#"{"hook_event_name":"Stop","cwd":"/repo","session_id":"session"}"#.to_vec(),
            delay: Duration::from_millis(10),
            read: false,
        };
        let deadline = Instant::now() + Duration::from_millis(1);
        assert!(matches!(
            drain_and_parse_until(&mut input, deadline),
            Err(HookError::DeadlineExceeded)
        ));
    }

    #[test]
    fn expired_input_is_rejected_before_reading_payload() {
        let mut input =
            Cursor::new(br#"{"hook_event_name":"Stop","cwd":"/repo","session_id":"session"}"#);
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("test deadline remains representable");
        assert!(matches!(
            drain_and_parse_until(&mut input, deadline),
            Err(HookError::DeadlineExceeded)
        ));
        assert_eq!(input.position(), 0);
    }
}
