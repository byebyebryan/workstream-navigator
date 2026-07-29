use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    HostId, LocationId, RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
};

pub const CURRENT_PROTOCOL_VERSION: u16 = 1;
const MAX_ALIAS_BYTES: usize = 128;
const MAX_DIAGNOSTIC_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestEnvelope {
    pub version: u16,
    pub request: HostRequest,
}

impl RequestEnvelope {
    /// Validates the protocol version and all request fields before dispatch.
    ///
    /// # Errors
    ///
    /// Returns an error for an incompatible protocol version or a malformed,
    /// unbounded request field.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        self.request.validate()
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
        if let Self::Hello { client_alias } = self {
            validate_bounded("client alias", client_alias, MAX_ALIAS_BYTES)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    Resume {
        workstream_id: WorkstreamId,
        expected_revision: i64,
    },
    RegisterLocation {
        location_id: LocationId,
        expected_revision: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResponseEnvelope {
    pub version: u16,
    pub response: HostResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostResponse {
    Hello(HelloResponse),
    Snapshot(SnapshotResponse),
    Applied { revision: i64 },
    Attach { command: Vec<String> },
    Rejected { diagnostic: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HelloResponse {
    pub host_id: HostId,
    pub registry_generation: String,
    pub capabilities: Capabilities,
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
    pub revision: i64,
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
}
