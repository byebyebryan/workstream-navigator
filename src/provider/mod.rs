//! Provider adapters. V1 contains one concrete Codex implementation.

use std::process::Command;

use thiserror::Error;

use crate::{
    domain::ProviderKind,
    process::output_bounded,
    protocol::{
        KNOWN_PROVIDER_KINDS, ProviderCapability, ProviderCapabilityReason,
        ProviderCapabilityStatus,
    },
    state::{HostRegistry, IntegrationLifecycle, StateError},
};

pub mod codex;

/// Dynamically observed provider readiness evidence. This is intentionally
/// read-only and bounded; it never carries process output, paths, prompts, or
/// provider payloads.
///
/// # Errors
///
/// Returns an error when the host registry cannot read its observer evidence.
pub fn discover_capabilities(
    registry: &HostRegistry,
) -> Result<Vec<ProviderCapability>, StateError> {
    discover_capabilities_with(registry, command_available)
}

/// Deterministic capability-discovery seam used by tests and host snapshots.
/// The probe only answers whether one fixed executable can report its version.
///
/// # Errors
///
/// Returns an error when the host registry cannot read its observer evidence.
pub fn discover_capabilities_with(
    registry: &HostRegistry,
    command_available: impl Fn(&str) -> bool,
) -> Result<Vec<ProviderCapability>, StateError> {
    let codex_installed = command_available("codex");
    let runtime_ready = command_available("tmux");
    let observer_ready = matches!(
        registry
            .codex_integration()?
            .map(|integration| integration.lifecycle),
        Some(IntegrationLifecycle::Ready)
    );
    let codex = if !codex_installed {
        capability(
            ProviderKind::Codex,
            ProviderCapabilityStatus::Unavailable,
            ProviderCapabilityReason::NotInstalled,
        )
    } else if !runtime_ready {
        capability(
            ProviderKind::Codex,
            ProviderCapabilityStatus::Unavailable,
            ProviderCapabilityReason::RuntimePrerequisiteMissing,
        )
    } else if !observer_ready {
        capability(
            ProviderKind::Codex,
            ProviderCapabilityStatus::Unavailable,
            ProviderCapabilityReason::ObserverNotReady,
        )
    } else {
        ProviderCapability {
            kind: ProviderKind::Codex,
            status: ProviderCapabilityStatus::Available,
            reason: ProviderCapabilityReason::None,
            fresh_launch: true,
            exact_resume: true,
            observe: true,
            metadata_read: true,
            rename: true,
            fork: true,
        }
    };
    let opencode = capability(
        ProviderKind::OpenCode,
        ProviderCapabilityStatus::Unavailable,
        ProviderCapabilityReason::AdapterUnavailable,
    );
    let capabilities = vec![codex, opencode];
    debug_assert_eq!(
        capabilities
            .iter()
            .map(|capability| capability.kind)
            .collect::<Vec<_>>(),
        KNOWN_PROVIDER_KINDS,
    );
    Ok(capabilities)
}

fn capability(
    kind: ProviderKind,
    status: ProviderCapabilityStatus,
    reason: ProviderCapabilityReason,
) -> ProviderCapability {
    ProviderCapability {
        kind,
        status,
        reason,
        fresh_launch: false,
        exact_resume: false,
        observe: false,
        metadata_read: false,
        rename: false,
        fork: false,
    }
}

/// Bounded, provider-neutral failure returned when a selected provider cannot
/// safely create a new recoverable Workstream. It intentionally exposes only
/// typed status/reason evidence, never raw process or state diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("provider is not eligible for a new Workstream")]
pub struct ProviderReadinessError {
    pub kind: ProviderKind,
    pub status: ProviderCapabilityStatus,
    pub reason: ProviderCapabilityReason,
}

/// Re-probes one provider immediately before a durable New/registration
/// transaction. A stale cached snapshot is never used as authorization.
///
/// # Errors
///
/// Returns a bounded typed error when the provider is not currently eligible.
pub fn require_new_eligible(
    registry: &HostRegistry,
    kind: ProviderKind,
) -> Result<(), ProviderReadinessError> {
    require_new_eligible_with(registry, kind, command_available)
}

fn require_new_eligible_with(
    registry: &HostRegistry,
    kind: ProviderKind,
    command_available: impl Fn(&str) -> bool,
) -> Result<(), ProviderReadinessError> {
    let capability = discover_capabilities_with(registry, command_available)
        .ok()
        .and_then(|capabilities| {
            capabilities
                .into_iter()
                .find(|capability| capability.kind == kind)
        })
        .unwrap_or_else(|| {
            capability(
                kind,
                ProviderCapabilityStatus::Unknown,
                ProviderCapabilityReason::ProbeFailed,
            )
        });
    if capability.is_new_eligible() {
        Ok(())
    } else {
        Err(ProviderReadinessError {
            kind: capability.kind,
            status: capability.status,
            reason: capability.reason,
        })
    }
}

pub(crate) fn command_available(program: &str) -> bool {
    let mut command = Command::new(program);
    command.arg(if program == "tmux" { "-V" } else { "--version" });
    output_bounded(&mut command, 4096, 4096).is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn registry() -> (tempfile::TempDir, HostRegistry) {
        let temporary = tempfile::tempdir().unwrap();
        let root = crate::state::StateRoot::create(temporary.path().join("state")).unwrap();
        (temporary, HostRegistry::open(&root).unwrap())
    }

    #[test]
    fn injected_probe_requires_all_codex_evidence_in_precedence_order() {
        let (_temporary, mut registry) = registry();
        let ready = crate::provider::codex::profile::ProfileOwnership {
            canonical_path: PathBuf::from("/tmp/wsnav-observer.json"),
            owner_id: "owner".to_owned(),
            profile_schema_version: 2,
            hook_executable: PathBuf::from("/tmp/wsnav"),
            content_hash: "hash".to_owned(),
        };
        registry
            .record_codex_integration(ready, IntegrationLifecycle::Ready)
            .unwrap();

        let capabilities =
            discover_capabilities_with(&registry, |program| matches!(program, "codex" | "tmux"))
                .unwrap();
        assert!(capabilities[0].is_new_eligible());
        assert!(
            require_new_eligible_with(&registry, ProviderKind::Codex, |program| {
                matches!(program, "codex" | "tmux")
            })
            .is_ok()
        );

        let missing_runtime =
            discover_capabilities_with(&registry, |program| program == "codex").unwrap();
        assert_eq!(
            missing_runtime[0].reason,
            ProviderCapabilityReason::RuntimePrerequisiteMissing
        );
        assert_eq!(
            require_new_eligible_with(&registry, ProviderKind::Codex, |program| program == "codex"),
            Err(ProviderReadinessError {
                kind: ProviderKind::Codex,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::RuntimePrerequisiteMissing,
            })
        );

        let missing_command = discover_capabilities_with(&registry, |_| false).unwrap();
        assert_eq!(
            missing_command[0].reason,
            ProviderCapabilityReason::NotInstalled
        );
    }

    #[test]
    fn opencode_is_always_adapter_unavailable_without_a_probe() {
        let (_temporary, registry) = registry();
        let capabilities = discover_capabilities_with(&registry, |_| true).unwrap();
        assert_eq!(
            capabilities[1],
            ProviderCapability {
                kind: ProviderKind::OpenCode,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::AdapterUnavailable,
                fresh_launch: false,
                exact_resume: false,
                observe: false,
                metadata_read: false,
                rename: false,
                fork: false,
            }
        );
        assert_eq!(
            require_new_eligible_with(&registry, ProviderKind::OpenCode, |_| true),
            Err(ProviderReadinessError {
                kind: ProviderKind::OpenCode,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::AdapterUnavailable,
            })
        );
    }
}
