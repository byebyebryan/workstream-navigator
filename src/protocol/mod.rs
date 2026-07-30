//! Bounded machine-to-machine frames used by local and SSH host adapters.
//!
//! The protocol deliberately contains only durable navigator metadata. It
//! never transports a provider prompt, response, terminal capture, checkout
//! path, or hook payload.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    HostId, LocationId, Revision, RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
};

pub const CURRENT_PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 512;
pub const MAX_SNAPSHOT_WORKSTREAMS: usize = 128;

const MAX_ALIAS_BYTES: usize = 128;
const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 64;
const MAX_REGISTRY_GENERATION_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestEnvelope {
    pub version: u16,
    pub request: HostRequest,
}

impl RequestEnvelope {
    /// Validates a request before it can be dispatched by a host.
    ///
    /// # Errors
    ///
    /// Returns an error for an incompatible protocol version or malformed,
    /// unbounded request field.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        self.request.validate()
    }

    /// Serializes one validated request as a single bounded JSON frame.
    ///
    /// # Errors
    ///
    /// Returns an error if validation or JSON encoding fails, or the frame
    /// would exceed the protocol's bounded transport size.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        encode_frame(self)
    }

    /// Decodes and validates one bounded JSON request frame.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, malformed, or incompatible frames.
    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        let request: Self = decode_frame(frame)?;
        request.validate()?;
        Ok(request)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostRequest {
    Hello { client_alias: String },
    Snapshot,
    Attach { runtime_id: RuntimeId },
    Apply { action: HostAction },
}

impl HostRequest {
    fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Hello { client_alias } => {
                validate_bounded("client alias", client_alias, MAX_ALIAS_BYTES)
            }
            Self::Snapshot | Self::Attach { .. } => Ok(()),
            Self::Apply { action } => action.validate(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostAction {
    AcknowledgeAttention {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    Park {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    Start {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
}

impl HostAction {
    fn validate(&self) -> Result<(), ProtocolError> {
        let expected_revision = match self {
            Self::AcknowledgeAttention {
                expected_revision, ..
            }
            | Self::Park {
                expected_revision, ..
            }
            | Self::Start {
                expected_revision, ..
            } => *expected_revision,
        };
        Revision::try_from(expected_revision).map_err(|_| ProtocolError::InvalidRevision)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResponseEnvelope {
    pub version: u16,
    pub response: HostResponse,
}

impl ResponseEnvelope {
    /// Creates a bounded rejection frame safe for machine-oriented transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the diagnostic cannot safely fit in the protocol
    /// frame.
    pub fn rejected(diagnostic: String) -> Result<Self, ProtocolError> {
        validate_bounded("diagnostic", &diagnostic, MAX_DIAGNOSTIC_BYTES)?;
        Ok(Self {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Rejected { diagnostic },
        })
    }

    /// Validates a response received from a host before client state can use
    /// it for presentation or mutation authority.
    ///
    /// # Errors
    ///
    /// Returns an error for incompatible, malformed, or oversized response
    /// fields.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        self.response.validate()
    }

    /// Serializes one validated response as a single bounded JSON frame.
    ///
    /// # Errors
    ///
    /// Returns an error if validation or JSON encoding fails, or the frame
    /// would exceed the protocol's bounded transport size.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        encode_frame(self)
    }

    /// Decodes and validates one bounded JSON response frame.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, malformed, or incompatible frames.
    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        let response: Self = decode_frame(frame)?;
        response.validate()?;
        Ok(response)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostResponse {
    Hello(HelloResponse),
    Snapshot(SnapshotResponse),
    Applied { revision: i64 },
    Attach { runtime_id: RuntimeId },
    Rejected { diagnostic: String },
}

impl HostResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Hello(response) => response.validate(),
            Self::Snapshot(response) => response.validate(),
            Self::Applied { revision } => Revision::try_from(*revision)
                .map(|_| ())
                .map_err(|_| ProtocolError::InvalidRevision),
            Self::Attach { .. } => Ok(()),
            Self::Rejected { diagnostic } => {
                validate_bounded("diagnostic", diagnostic, MAX_DIAGNOSTIC_BYTES)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HelloResponse {
    pub host_id: HostId,
    pub registry_generation: String,
    pub wsnav_version: String,
    pub capabilities: Capabilities,
}

impl HelloResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_bounded(
            "registry generation",
            &self.registry_generation,
            MAX_REGISTRY_GENERATION_BYTES,
        )?;
        validate_bounded("wsnav version", &self.wsnav_version, MAX_VERSION_BYTES)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Capabilities {
    pub codex: bool,
    pub git: bool,
    pub tmux: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotResponse {
    pub workstreams: Vec<SnapshotWorkstream>,
}

impl SnapshotResponse {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.workstreams.len() > MAX_SNAPSHOT_WORKSTREAMS {
            return Err(ProtocolError::SnapshotTooLarge);
        }
        self.workstreams
            .iter()
            .try_for_each(SnapshotWorkstream::validate)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotWorkstream {
    pub workstream_id: WorkstreamId,
    pub location_id: LocationId,
    pub display_name: String,
    pub runtime_id: Option<RuntimeId>,
    pub runtime_status: RuntimeStatus,
    pub lifecycle: WorkstreamLifecycle,
    pub result_ready: bool,
    pub recovery_required: bool,
    pub attention_revision: Option<i64>,
    pub activity_sequence: i64,
    pub revision: i64,
}

impl SnapshotWorkstream {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_bounded("display name", &self.display_name, MAX_DISPLAY_NAME_BYTES)?;
        if self.activity_sequence < 0 {
            return Err(ProtocolError::InvalidActivitySequence);
        }
        Revision::try_from(self.revision).map_err(|_| ProtocolError::InvalidRevision)?;
        if let Some(revision) = self.attention_revision {
            Revision::try_from(revision).map_err(|_| ProtocolError::InvalidRevision)?;
        }
        Ok(())
    }
}

fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let mut frame = serde_json::to_vec(value).map_err(ProtocolError::Encode)?;
    frame.push(b'\n');
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(frame)
}

fn decode_frame<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<T, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    serde_json::from_slice(frame).map_err(ProtocolError::Decode)
}

fn validate_bounded(name: &'static str, value: &str, maximum: usize) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        return Err(ProtocolError::EmptyField(name));
    }
    if value.len() > maximum {
        return Err(ProtocolError::FieldTooLong { name, maximum });
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(ProtocolError::ControlCharacter(name));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("{0} cannot be empty")]
    EmptyField(&'static str),
    #[error("{0} contains a newline")]
    ControlCharacter(&'static str),
    #[error("{name} exceeds {maximum} bytes")]
    FieldTooLong { name: &'static str, maximum: usize },
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("revision must be positive")]
    InvalidRevision,
    #[error("activity sequence must not be negative")]
    InvalidActivitySequence,
    #[error("snapshot has too many workstreams")]
    SnapshotTooLarge,
    #[error("protocol frame exceeds its maximum size")]
    FrameTooLarge,
    #[error("could not encode protocol frame")]
    Encode(serde_json::Error),
    #[error("could not decode protocol frame")]
    Decode(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_versions_fail_closed() {
        let envelope = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION + 1,
            request: HostRequest::Snapshot,
        };

        assert!(matches!(
            envelope.validate(),
            Err(ProtocolError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn client_alias_cannot_be_used_as_a_multiline_shell_fragment() {
        let envelope = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Hello {
                client_alias: "trusted\nuntrusted".to_owned(),
            },
        };

        assert!(matches!(
            envelope.validate(),
            Err(ProtocolError::ControlCharacter("client alias"))
        ));
    }

    #[test]
    fn response_diagnostics_are_bounded() {
        let diagnostic = "x".repeat(MAX_DIAGNOSTIC_BYTES + 1);

        assert!(matches!(
            ResponseEnvelope::rejected(diagnostic),
            Err(ProtocolError::FieldTooLong {
                name: "diagnostic",
                ..
            })
        ));
    }

    #[test]
    fn frame_round_trip_uses_only_one_bounded_json_document() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Hello {
                client_alias: "laptop".to_owned(),
            },
        };

        let frame = request.encode().unwrap();

        assert!(frame.ends_with(b"\n"));
        assert_eq!(RequestEnvelope::decode(&frame).unwrap(), request);
    }

    #[test]
    fn invalid_action_revision_fails_before_dispatch() {
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::Park {
                    workstream_id: WorkstreamId::new(),
                    expected_revision: 0,
                },
            },
        };

        assert!(matches!(
            request.validate(),
            Err(ProtocolError::InvalidRevision)
        ));
    }

    #[test]
    fn oversized_frames_are_rejected_without_parsing() {
        let frame = vec![b'x'; MAX_FRAME_BYTES + 1];

        assert!(matches!(
            RequestEnvelope::decode(&frame),
            Err(ProtocolError::FrameTooLarge)
        ));
    }
}
