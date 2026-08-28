//! Boot-scoped monotonic time for D17 one-shot handoffs.
//!
//! A capability is valid only within the same Linux boot.  The broker persists
//! a digest of the boot identifier in its claims, while the helper recomputes
//! it before consumption.  This prevents a stale monotonic expiry from being
//! interpreted after a restart.

use sha2::{Digest, Sha256};
use thiserror::Error;

const BOOT_PROVENANCE_VERSION: &str = "d17-boot-v1";

/// Clock readings share a single boot-scoped provenance.
pub(crate) trait D17Clock {
    fn now_monotonic_millis(&self) -> Result<i64, ClockError>;
    fn boot_provenance(&self) -> Result<String, ClockError>;
}

/// The Linux `CLOCK_BOOTTIME` clock and kernel boot identifier.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemD17Clock;

impl D17Clock for SystemD17Clock {
    fn now_monotonic_millis(&self) -> Result<i64, ClockError> {
        system_monotonic_millis()
    }

    fn boot_provenance(&self) -> Result<String, ClockError> {
        system_boot_provenance()
    }
}

#[cfg(target_os = "linux")]
fn system_monotonic_millis() -> Result<i64, ClockError> {
    use nix::{
        sys::time::TimeValLike,
        time::{ClockId, clock_gettime},
    };

    let millis = clock_gettime(ClockId::CLOCK_BOOTTIME)
        .map_err(|_| ClockError::Unavailable)?
        .num_milliseconds();
    (millis >= 0)
        .then_some(millis)
        .ok_or(ClockError::Unavailable)
}

#[cfg(not(target_os = "linux"))]
fn system_monotonic_millis() -> Result<i64, ClockError> {
    Err(ClockError::Unavailable)
}

#[cfg(target_os = "linux")]
fn system_boot_provenance() -> Result<String, ClockError> {
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| ClockError::Unavailable)?;
    let boot_id = boot_id.trim();
    if uuid::Uuid::parse_str(boot_id).is_err() {
        return Err(ClockError::Unavailable);
    }
    let digest = Sha256::digest(boot_id.as_bytes());
    Ok(format!("{BOOT_PROVENANCE_VERSION}:sha256:{digest:x}"))
}

#[cfg(not(target_os = "linux"))]
fn system_boot_provenance() -> Result<String, ClockError> {
    Err(ClockError::Unavailable)
}

/// Bounded clock/provenance failure that carries no host identifier.
#[derive(Debug, Error)]
pub(crate) enum ClockError {
    #[error("the D17 boot-scoped clock is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::{D17Clock, SystemD17Clock};

    #[test]
    #[cfg(target_os = "linux")]
    fn system_clock_has_a_nonnegative_reading_and_private_boot_provenance() {
        let clock = SystemD17Clock;
        assert!(clock.now_monotonic_millis().unwrap() >= 0);
        assert!(
            clock
                .boot_provenance()
                .unwrap()
                .starts_with("d17-boot-v1:sha256:")
        );
    }
}
