//! Bounded `OpenCode` event-envelope parsing.

use std::path::Path;

use serde_json::Value;
use thiserror::Error;

use crate::{domain::ProviderSessionId, provider::lifecycle::LifecycleHint};

const MAX_EVENT_BYTES: usize = 64 * 1024;

const RECOGNIZED_EVENT_TYPES: [&str; 7] = [
    "session.created",
    "session.started",
    "session.deleted",
    "session.ended",
    "session.idle",
    "session.status",
    "message.updated",
];

/// A recognized lifecycle envelope could not be validated without guessing
/// at identity or provider state. Well-formed unrelated events are represented
/// by `Ok(None)` instead.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("OpenCode lifecycle event envelope was malformed")]
pub struct OpenCodeEventParseError;

/// One bounded `OpenCode` event after its envelope and root-session identity
/// have been validated. Raw payload content is never retained here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeEvent {
    pub hint: Option<LifecycleHint>,
    pub candidate_message_id: Option<String>,
    pub clears_candidate: bool,
}

#[must_use]
pub fn parse_event_hint(data: &[u8], root_session: &ProviderSessionId) -> Option<LifecycleHint> {
    parse_event_for_project(data, root_session, None)?.hint
}

/// Parses an event while optionally requiring an exact project-root field.
/// Child sessions and unrelated project events are discarded before any
/// neutral lifecycle hint is produced.
#[must_use]
pub fn parse_event_hint_for_project(
    data: &[u8],
    root_session: &ProviderSessionId,
    project_root: Option<&Path>,
) -> Option<LifecycleHint> {
    parse_event_for_project(data, root_session, project_root)?.hint
}

/// Parses one exact `OpenCode` event envelope and retains only a lifecycle
/// hint plus a completed-assistant candidate ID. The candidate is not a
/// settled turn until corroborated by exact idle status evidence.
#[must_use]
pub fn parse_event_for_project(
    data: &[u8],
    root_session: &ProviderSessionId,
    project_root: Option<&Path>,
) -> Option<OpenCodeEvent> {
    parse_event_strict(data, root_session, project_root)
        .ok()
        .flatten()
}

/// Parses one event with an explicit malformed-envelope outcome for the
/// observer. Compatibility `Option` wrappers above intentionally collapse
/// this error for older pure parser callers.
///
/// # Errors
///
/// Returns an error when a recognized lifecycle envelope is malformed or
/// exceeds the bounded input limit.
pub fn parse_event_strict(
    data: &[u8],
    root_session: &ProviderSessionId,
    project_root: Option<&Path>,
) -> Result<Option<OpenCodeEvent>, OpenCodeEventParseError> {
    if data.is_empty() || data.len() > MAX_EVENT_BYTES {
        return Err(OpenCodeEventParseError);
    }
    let value: Value = serde_json::from_slice(data).map_err(|_| OpenCodeEventParseError)?;
    let payload = value
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(OpenCodeEventParseError)?;
    let event_type = payload.get("type").and_then(Value::as_str);
    let property_type = payload
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get("type"))
        .and_then(Value::as_str);
    let Some(event_type) = event_type.or(property_type) else {
        return Ok(None);
    };
    if !RECOGNIZED_EVENT_TYPES.contains(&event_type) {
        return Ok(None);
    }
    if payload.get("type").and_then(Value::as_str) != Some(event_type) {
        return Err(OpenCodeEventParseError);
    }
    let properties = payload
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(OpenCodeEventParseError)?;
    let info = properties.get("info");
    let session_id = properties
        .get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| {
            info.and_then(|info| info.get("sessionID"))
                .and_then(Value::as_str)
        })
        .ok_or(OpenCodeEventParseError)?;
    if session_id != root_session.native_id() {
        return Ok(None);
    }
    if let Some(project_root) = project_root {
        let expected = project_root.to_string_lossy();
        let observed = properties
            .get("directory")
            .and_then(Value::as_str)
            .or_else(|| properties.get("project").and_then(Value::as_str));
        if observed.is_some_and(|observed| observed != expected) {
            return Ok(None);
        }
    }
    let status_type = properties
        .get("status")
        .and_then(Value::as_object)
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str);
    let candidate_message_id = (event_type == "message.updated")
        .then(|| assistant_message_id(info))
        .flatten();
    let clears_candidate = event_type == "message.updated" && candidate_message_id.is_none();
    let hint = match event_type {
        "session.created" | "session.started" => Some(LifecycleHint::Started),
        "session.deleted" | "session.ended" => Some(LifecycleHint::Ended),
        "session.idle" => Some(LifecycleHint::Settled { message_id: None }),
        "session.status" => match status_type {
            Some("busy" | "retry") => Some(LifecycleHint::Working),
            Some("idle") => Some(LifecycleHint::Settled { message_id: None }),
            _ => return Err(OpenCodeEventParseError),
        },
        "message.updated" if candidate_message_id.is_none() => Some(LifecycleHint::Working),
        "message.updated" => None,
        _ => return Ok(None),
    };
    Ok(Some(OpenCodeEvent {
        hint,
        candidate_message_id,
        clears_candidate,
    }))
}

fn assistant_message_id(info: Option<&Value>) -> Option<String> {
    let info = info?;
    if info.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let completed = info
        .get("time")
        .and_then(|time| time.get("completed"))
        .and_then(Value::as_u64)
        .is_some();
    let finished = info
        .get("finish")
        .and_then(Value::as_str)
        .is_some_and(|finish| !finish.is_empty());
    if !completed || !finished {
        return None;
    }
    let id = info.get("id").and_then(Value::as_str)?;
    (id.len() <= 512 && !id.is_empty()).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_and_unknown_events_are_not_lifecycle_evidence() {
        let root = ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "root").unwrap();
        assert_eq!(
            parse_event_for_project(
                br#"{"payload":{"type":"unknown","properties":{"sessionID":"root"}}}"#,
                &root,
                None,
            ),
            None
        );
    }
}
