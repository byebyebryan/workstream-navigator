//! Passive, fail-closed Codex lifecycle-hook ingestion.

use std::io::Read;

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
    let mut retained = Vec::with_capacity(4096);
    let mut total = 0_usize;
    let mut chunk = [0_u8; 8192];
    loop {
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
    if total > MAX_HOOK_PAYLOAD_BYTES {
        return Err(HookError::PayloadTooLarge);
    }

    let payload: HookPayload = serde_json::from_slice(&retained).map_err(HookError::Malformed)?;
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
    #[error("unsupported lifecycle event")]
    UnsupportedEvent,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

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
}
