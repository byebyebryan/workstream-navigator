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

/// Bounded failure returned when the caller cannot determine one exact
/// provider for a new Workstream from the currently observed capabilities.
/// Selection is deliberately provider-neutral; callers choose the appropriate
/// policy for registration or for a source Workstream before issuing an exact
/// action.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ProviderSelectionError {
    #[error("no provider is eligible for a new Workstream")]
    NoEligibleProviders,
    #[error("a provider must be selected explicitly with --provider <provider>")]
    SelectionRequired,
    #[error("requested provider is unavailable for a new Workstream")]
    ExplicitProviderUnavailable {
        kind: ProviderKind,
        status: ProviderCapabilityStatus,
        reason: ProviderCapabilityReason,
    },
}

/// Returns eligible providers in the protocol's fixed known-provider order.
/// Unknown or duplicate capability records are ignored rather than becoming a
/// fallback provider; authoritative protocol validation rejects malformed
/// wire sets before this helper is used.
#[must_use]
pub fn eligible_new_providers(capabilities: &[ProviderCapability]) -> Vec<ProviderKind> {
    if !capability_set_well_formed(capabilities) {
        return Vec::new();
    }
    KNOWN_PROVIDER_KINDS
        .into_iter()
        .filter(|kind| {
            capabilities
                .iter()
                .find(|capability| capability.kind == *kind)
                .is_some_and(|capability| capability.is_new_eligible())
        })
        .collect()
}

/// Selects a provider for initial registration. With no explicit choice this
/// permits exactly one currently eligible provider; zero or multiple providers
/// remain bounded, explicit selection outcomes.
///
/// # Errors
///
/// Returns [`ProviderSelectionError::NoEligibleProviders`] when no provider is
/// currently eligible, [`ProviderSelectionError::SelectionRequired`] when
/// more than one is eligible without an explicit choice, or
/// [`ProviderSelectionError::ExplicitProviderUnavailable`] for an explicit
/// provider that is not currently eligible.
pub fn select_registration_provider(
    capabilities: &[ProviderCapability],
    requested: Option<ProviderKind>,
) -> Result<ProviderKind, ProviderSelectionError> {
    select_provider(capabilities, requested)
}

/// Selects a provider for a new Workstream derived from `source_provider`.
/// Without an explicit choice, the source provider is the only implicit
/// default. If it is unavailable, the caller must choose explicitly even when
/// another provider is eligible.
///
/// # Errors
///
/// Returns a bounded selection error when no provider is eligible, an
/// explicit choice is unavailable, or the source provider cannot authorize an
/// implicit choice while more than one/another provider is available.
pub fn select_new_provider(
    capabilities: &[ProviderCapability],
    requested: Option<ProviderKind>,
    source_provider: ProviderKind,
) -> Result<ProviderKind, ProviderSelectionError> {
    if requested.is_some() {
        return select_provider(capabilities, requested);
    }

    let eligible = eligible_new_providers(capabilities);
    if eligible.contains(&source_provider) {
        return Ok(source_provider);
    }
    match eligible.as_slice() {
        [] => Err(ProviderSelectionError::NoEligibleProviders),
        _ => Err(ProviderSelectionError::SelectionRequired),
    }
}

fn select_provider(
    capabilities: &[ProviderCapability],
    requested: Option<ProviderKind>,
) -> Result<ProviderKind, ProviderSelectionError> {
    if let Some(kind) = requested {
        if !capability_set_well_formed(capabilities) {
            return Err(ProviderSelectionError::ExplicitProviderUnavailable {
                kind,
                status: ProviderCapabilityStatus::Unknown,
                reason: ProviderCapabilityReason::ProbeFailed,
            });
        }
        let capability = capabilities
            .iter()
            .find(|capability| capability.kind == kind)
            .copied()
            .unwrap_or(ProviderCapability {
                kind,
                status: ProviderCapabilityStatus::Unknown,
                reason: ProviderCapabilityReason::ProbeFailed,
                fresh_launch: false,
                exact_resume: false,
                observe: false,
                metadata_read: false,
                rename: false,
                fork: false,
            });
        return if capability.is_new_eligible() {
            Ok(kind)
        } else {
            Err(ProviderSelectionError::ExplicitProviderUnavailable {
                kind,
                status: capability.status,
                reason: capability.reason,
            })
        };
    }

    match eligible_new_providers(capabilities).as_slice() {
        [] => Err(ProviderSelectionError::NoEligibleProviders),
        [kind] => Ok(*kind),
        _ => Err(ProviderSelectionError::SelectionRequired),
    }
}

fn capability_set_well_formed(capabilities: &[ProviderCapability]) -> bool {
    capabilities.len() == KNOWN_PROVIDER_KINDS.len()
        && KNOWN_PROVIDER_KINDS.into_iter().all(|kind| {
            capabilities
                .iter()
                .filter(|capability| capability.kind == kind)
                .count()
                == 1
        })
        && capabilities
            .iter()
            .all(|capability| capability.validate().is_ok())
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

    fn eligible(kind: ProviderKind) -> ProviderCapability {
        ProviderCapability {
            kind,
            status: ProviderCapabilityStatus::Available,
            reason: ProviderCapabilityReason::None,
            fresh_launch: true,
            exact_resume: true,
            observe: true,
            metadata_read: true,
            rename: true,
            fork: true,
        }
    }

    #[test]
    fn selection_refuses_zero_and_auto_selects_sole_codex() {
        let none = vec![
            capability(
                ProviderKind::Codex,
                ProviderCapabilityStatus::Unavailable,
                ProviderCapabilityReason::NotInstalled,
            ),
            capability(
                ProviderKind::OpenCode,
                ProviderCapabilityStatus::Unavailable,
                ProviderCapabilityReason::AdapterUnavailable,
            ),
        ];
        assert_eq!(
            select_registration_provider(&none, None),
            Err(ProviderSelectionError::NoEligibleProviders)
        );

        let sole = vec![
            eligible(ProviderKind::Codex),
            capability(
                ProviderKind::OpenCode,
                ProviderCapabilityStatus::Unavailable,
                ProviderCapabilityReason::AdapterUnavailable,
            ),
        ];
        assert_eq!(
            select_registration_provider(&sole, None),
            Ok(ProviderKind::Codex)
        );
    }

    #[test]
    fn selection_requires_explicit_choice_for_multiple_and_preserves_known_order() {
        let capabilities = vec![
            eligible(ProviderKind::OpenCode),
            eligible(ProviderKind::Codex),
        ];
        assert_eq!(
            eligible_new_providers(&capabilities),
            vec![ProviderKind::Codex, ProviderKind::OpenCode]
        );
        assert_eq!(
            select_registration_provider(&capabilities, None),
            Err(ProviderSelectionError::SelectionRequired)
        );
        assert!(
            ProviderSelectionError::SelectionRequired
                .to_string()
                .contains("--provider")
        );
        assert_eq!(
            select_new_provider(&capabilities, None, ProviderKind::OpenCode),
            Ok(ProviderKind::OpenCode)
        );
        assert_eq!(
            select_new_provider(&capabilities, None, ProviderKind::Codex),
            Ok(ProviderKind::Codex)
        );
    }

    #[test]
    fn explicit_selection_never_falls_back_to_another_provider() {
        let capabilities = vec![
            eligible(ProviderKind::Codex),
            capability(
                ProviderKind::OpenCode,
                ProviderCapabilityStatus::Unavailable,
                ProviderCapabilityReason::AdapterUnavailable,
            ),
        ];
        assert_eq!(
            select_registration_provider(&capabilities, Some(ProviderKind::OpenCode)),
            Err(ProviderSelectionError::ExplicitProviderUnavailable {
                kind: ProviderKind::OpenCode,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::AdapterUnavailable,
            })
        );
        assert_eq!(
            select_new_provider(
                &capabilities,
                Some(ProviderKind::OpenCode),
                ProviderKind::Codex
            ),
            Err(ProviderSelectionError::ExplicitProviderUnavailable {
                kind: ProviderKind::OpenCode,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::AdapterUnavailable,
            })
        );
    }

    #[test]
    fn malformed_capability_sets_fail_closed() {
        let codex = eligible(ProviderKind::Codex);
        assert!(eligible_new_providers(&[codex]).is_empty());
        assert!(eligible_new_providers(&[codex, codex]).is_empty());
        let malformed_codex = ProviderCapability {
            reason: ProviderCapabilityReason::AdapterUnavailable,
            ..codex
        };
        assert!(
            eligible_new_providers(&[
                malformed_codex,
                capability(
                    ProviderKind::OpenCode,
                    ProviderCapabilityStatus::Unavailable,
                    ProviderCapabilityReason::AdapterUnavailable,
                ),
            ])
            .is_empty()
        );
        assert_eq!(
            select_registration_provider(&[codex], Some(ProviderKind::Codex)),
            Err(ProviderSelectionError::ExplicitProviderUnavailable {
                kind: ProviderKind::Codex,
                status: ProviderCapabilityStatus::Unknown,
                reason: ProviderCapabilityReason::ProbeFailed,
            })
        );
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
