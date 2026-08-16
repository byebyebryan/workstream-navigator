//! Stateless release compatibility metadata for manually installed hosts.

use std::io::Write;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Increment only when a build changes the meaning of the control protocol
/// independently of its wire version or host schema.
pub const CONTROL_ABI: u16 = 2;
const MAX_PACKAGE_VERSION_BYTES: usize = 64;

/// Safe metadata returned by the hidden, state-free `_probe` command.
///
/// It contains no host identity, registry generation, path, provider data, or
/// persisted state observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildInfo {
    pub package_version: String,
    pub control_abi: u16,
    pub protocol_version: u16,
    pub host_schema_version: i64,
}

impl BuildInfo {
    /// Returns the build metadata for the executable currently running.
    #[must_use]
    pub fn current() -> Self {
        Self {
            package_version: env!("CARGO_PKG_VERSION").to_owned(),
            control_abi: CONTROL_ABI,
            protocol_version: crate::protocol::CURRENT_PROTOCOL_VERSION,
            host_schema_version: crate::state::HOST_SCHEMA_VERSION,
        }
    }

    /// Checks whether a manually installed remote executable can participate
    /// in this build's control and state contract.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed metadata or an ABI, protocol, or schema
    /// mismatch. Package version is informational; compatible development
    /// builds need not carry a release tag change.
    pub fn ensure_compatible_with_local(&self) -> Result<(), BuildInfoError> {
        self.validate()?;
        let local = Self::current();
        if self.control_abi != local.control_abi {
            return Err(BuildInfoError::ControlAbiMismatch {
                local: local.control_abi,
                remote: self.control_abi,
            });
        }
        if self.protocol_version != local.protocol_version {
            return Err(BuildInfoError::ProtocolVersionMismatch {
                local: local.protocol_version,
                remote: self.protocol_version,
            });
        }
        if self.host_schema_version != local.host_schema_version {
            return Err(BuildInfoError::HostSchemaVersionMismatch {
                local: local.host_schema_version,
                remote: self.host_schema_version,
            });
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), BuildInfoError> {
        if self.package_version.trim().is_empty()
            || self.package_version.len() > MAX_PACKAGE_VERSION_BYTES
            || self.package_version.contains(['\n', '\r'])
        {
            return Err(BuildInfoError::InvalidPackageVersion);
        }
        if self.control_abi == 0 {
            return Err(BuildInfoError::InvalidControlAbi);
        }
        if self.protocol_version == 0 {
            return Err(BuildInfoError::InvalidProtocolVersion);
        }
        if self.host_schema_version < 1 {
            return Err(BuildInfoError::InvalidHostSchemaVersion);
        }
        Ok(())
    }
}

/// Writes one state-free JSON probe response for a manually installed binary.
///
/// # Errors
///
/// Returns an error only when the bounded JSON response cannot be serialized
/// or written to the supplied output.
pub fn write_probe(output: &mut impl Write) -> Result<(), BuildInfoError> {
    let mut encoded = serde_json::to_vec(&BuildInfo::current()).map_err(BuildInfoError::Encode)?;
    encoded.push(b'\n');
    output.write_all(&encoded).map_err(BuildInfoError::Write)?;
    output.flush().map_err(BuildInfoError::Write)
}

/// Errors from state-free build compatibility handling.
#[derive(Debug, Error)]
pub enum BuildInfoError {
    #[error("remote build probe has an invalid package version")]
    InvalidPackageVersion,
    #[error("remote build probe has an invalid control ABI")]
    InvalidControlAbi,
    #[error("remote build probe has an invalid protocol version")]
    InvalidProtocolVersion,
    #[error("remote build probe has an invalid host schema version")]
    InvalidHostSchemaVersion,
    #[error("remote control ABI {remote} does not match local ABI {local}")]
    ControlAbiMismatch { local: u16, remote: u16 },
    #[error("remote protocol version {remote} does not match local version {local}")]
    ProtocolVersionMismatch { local: u16, remote: u16 },
    #[error("remote host schema {remote} does not match local schema {local}")]
    HostSchemaVersionMismatch { local: i64, remote: i64 },
    #[error("could not encode the build probe")]
    Encode(serde_json::Error),
    #[error("could not write the build probe")]
    Write(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_build_is_compatible_with_itself() {
        assert!(BuildInfo::current().ensure_compatible_with_local().is_ok());
        assert_eq!(CONTROL_ABI, 2);
    }

    #[test]
    fn control_abi_mismatch_is_rejected_before_other_metadata() {
        let mut remote = BuildInfo::current();
        remote.control_abi = 1;

        assert!(matches!(
            remote.ensure_compatible_with_local(),
            Err(BuildInfoError::ControlAbiMismatch {
                local: 2,
                remote: 1
            })
        ));
    }

    #[test]
    fn schema_mismatch_requires_manual_upgrade() {
        let mut remote = BuildInfo::current();
        remote.host_schema_version += 1;

        assert!(matches!(
            remote.ensure_compatible_with_local(),
            Err(BuildInfoError::HostSchemaVersionMismatch { .. })
        ));
    }
}
