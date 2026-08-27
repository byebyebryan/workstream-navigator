//! Provider adapters. Each provider has a concrete, bounded capability probe.

use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    domain::ProviderKind,
    process::output_bounded,
    state::{HostRegistry, IntegrationLifecycle, StateError},
};

pub mod codex;
pub(crate) mod d17_grammar;
pub mod lifecycle;
pub mod names;
pub mod opencode;

/// The fixed provider set supported by this build, in deterministic order.
///
/// Provider capability evidence belongs to the provider boundary rather than
/// to the retired host protocol.  The application and state layers consume
/// this typed evidence directly; protocol adapters, where still present for
/// historical compatibility, merely serialize it.
pub const KNOWN_PROVIDER_KINDS: [ProviderKind; 2] = [ProviderKind::Codex, ProviderKind::OpenCode];

/// Dynamic provider availability observed by one host snapshot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityStatus {
    Available,
    Unavailable,
    Unknown,
}

/// Bounded reason for a provider's dynamic availability state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityReason {
    None,
    AdapterUnavailable,
    NotInstalled,
    UnsupportedVersion,
    ObserverNotReady,
    RuntimePrerequisiteMissing,
    ProbeFailed,
}

/// One provider's bounded, read-only host capability evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapability {
    pub kind: ProviderKind,
    pub status: ProviderCapabilityStatus,
    pub reason: ProviderCapabilityReason,
    pub fresh_launch: bool,
    pub exact_resume: bool,
    pub observe: bool,
    pub metadata_read: bool,
    pub rename: bool,
    pub fork: bool,
}

impl ProviderCapability {
    /// Returns whether this provider may be selected for a recoverable New.
    #[must_use]
    pub const fn is_new_eligible(self) -> bool {
        matches!(self.status, ProviderCapabilityStatus::Available)
            && self.fresh_launch
            && self.exact_resume
            && self.observe
    }
}

/// Returns the closed default capability set used when no provider probe has
/// succeeded.  It is intentionally provider-owned so state/application code
/// never needs to import the retired protocol module for capability records.
#[must_use]
pub fn default_provider_capabilities() -> Vec<ProviderCapability> {
    KNOWN_PROVIDER_KINDS
        .into_iter()
        .map(|kind| ProviderCapability {
            kind,
            status: if kind == ProviderKind::Codex {
                ProviderCapabilityStatus::Unknown
            } else {
                ProviderCapabilityStatus::Unavailable
            },
            reason: if kind == ProviderKind::Codex {
                ProviderCapabilityReason::ProbeFailed
            } else {
                ProviderCapabilityReason::AdapterUnavailable
            },
            fresh_launch: false,
            exact_resume: false,
            observe: false,
            metadata_read: false,
            rename: false,
            fork: false,
        })
        .collect()
}

/// Static executable evidence collected once for a navigator process.
///
/// This cache deliberately contains only installation/probe results. Dynamic
/// integration and observer readiness remains read from the registry whenever
/// capabilities are assembled, and action boundaries continue to use a fresh
/// [`discover_capabilities`] probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstallationProbeCache {
    codex_available: bool,
    tmux_available: bool,
    opencode: opencode::InstallationProbe,
}

impl InstallationProbeCache {
    /// Collects bounded executable evidence for one navigator process.
    #[must_use]
    pub fn probe() -> Self {
        Self::probe_with(command_available, opencode::probe_installation())
    }

    /// Deterministic seam for tests and isolated callers.
    #[must_use]
    pub fn probe_with(
        command_available: impl Fn(&str) -> bool,
        opencode: opencode::InstallationProbe,
    ) -> Self {
        Self {
            codex_available: command_available("codex"),
            tmux_available: command_available("tmux"),
            opencode,
        }
    }
}

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
    discover_capabilities_with_probe(registry, command_available, opencode::probe_installation())
}

/// Reassembles capabilities from cached static installation evidence and
/// freshly observed registry integration state.
///
/// # Errors
///
/// Returns an error when the host registry cannot read Codex observer state.
pub fn discover_capabilities_with_installation_cache(
    registry: &HostRegistry,
    cache: InstallationProbeCache,
) -> Result<Vec<ProviderCapability>, StateError> {
    discover_capabilities_with_probe(
        registry,
        |program| match program {
            "codex" => cache.codex_available,
            "tmux" => cache.tmux_available,
            _ => false,
        },
        cache.opencode,
    )
}

/// Capability discovery with an injected `OpenCode` installation outcome. The
/// Codex-only boolean seam remains available for existing deterministic tests,
/// while production discovery requires a successful bounded `opencode
/// --version` result without treating its release as compatibility authority
/// or consulting provider configuration or credentials.
///
/// # Errors
///
/// Returns an error when the host registry cannot read Codex observer state.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::missing_errors_doc)]
pub fn discover_capabilities_with_probe(
    registry: &HostRegistry,
    command_available: impl Fn(&str) -> bool,
    opencode_probe: opencode::InstallationProbe,
) -> Result<Vec<ProviderCapability>, StateError> {
    let tmux_available = command_available("tmux");
    let codex = codex_capability(registry, command_available("codex"), tmux_available)?;
    let opencode = match opencode_probe {
        opencode::InstallationProbe::Available if tmux_available => ProviderCapability {
            kind: ProviderKind::OpenCode,
            status: ProviderCapabilityStatus::Available,
            reason: ProviderCapabilityReason::None,
            fresh_launch: true,
            exact_resume: true,
            observe: true,
            metadata_read: true,
            rename: false,
            fork: true,
        },
        opencode::InstallationProbe::Available => capability(
            ProviderKind::OpenCode,
            ProviderCapabilityStatus::Unavailable,
            ProviderCapabilityReason::RuntimePrerequisiteMissing,
        ),
        opencode::InstallationProbe::NotInstalled => capability(
            ProviderKind::OpenCode,
            ProviderCapabilityStatus::Unavailable,
            ProviderCapabilityReason::NotInstalled,
        ),
        opencode::InstallationProbe::ProbeFailed => capability(
            ProviderKind::OpenCode,
            ProviderCapabilityStatus::Unknown,
            ProviderCapabilityReason::ProbeFailed,
        ),
    };
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

/// Deterministic capability-discovery seam used by tests and host snapshots.
/// The probe only answers whether one fixed executable can produce a bounded
/// successful version report.
///
/// # Errors
///
/// Returns an error when the host registry cannot read its observer evidence.
pub fn discover_capabilities_with(
    registry: &HostRegistry,
    command_available: impl Fn(&str) -> bool,
) -> Result<Vec<ProviderCapability>, StateError> {
    let codex = codex_capability(
        registry,
        command_available("codex"),
        command_available("tmux"),
    )?;
    let capabilities = vec![
        codex,
        capability(
            ProviderKind::OpenCode,
            ProviderCapabilityStatus::Unavailable,
            ProviderCapabilityReason::AdapterUnavailable,
        ),
    ];
    debug_assert_eq!(
        capabilities
            .iter()
            .map(|capability| capability.kind)
            .collect::<Vec<_>>(),
        KNOWN_PROVIDER_KINDS,
    );
    Ok(capabilities)
}

fn codex_capability(
    registry: &HostRegistry,
    codex_installed: bool,
    runtime_ready: bool,
) -> Result<ProviderCapability, StateError> {
    let observer_ready = matches!(
        registry
            .codex_integration()?
            .map(|integration| integration.lifecycle),
        Some(IntegrationLifecycle::Ready)
    );
    Ok(if !codex_installed {
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
    })
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

/// Returns eligible providers in the provider boundary's fixed known-provider order.
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
        && capabilities.iter().all(capability_is_well_formed)
}

fn capability_is_well_formed(capability: &ProviderCapability) -> bool {
    match (capability.status, capability.reason) {
        (ProviderCapabilityStatus::Available, ProviderCapabilityReason::None)
        | (
            ProviderCapabilityStatus::Unavailable | ProviderCapabilityStatus::Unknown,
            ProviderCapabilityReason::AdapterUnavailable
            | ProviderCapabilityReason::NotInstalled
            | ProviderCapabilityReason::UnsupportedVersion
            | ProviderCapabilityReason::ObserverNotReady
            | ProviderCapabilityReason::RuntimePrerequisiteMissing
            | ProviderCapabilityReason::ProbeFailed,
        ) => {}
        _ => return false,
    }
    matches!(capability.status, ProviderCapabilityStatus::Available)
        || !(capability.fresh_launch
            || capability.exact_resume
            || capability.observe
            || capability.metadata_read
            || capability.rename
            || capability.fork)
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
    require_new_eligible_from_capabilities(registry, kind, discover_capabilities(registry))
}

/// Requires the selected host/provider pair to expose exact Fork support.
/// Fork capability is distinct from New eligibility, and the authoritative
/// host is re-probed immediately before the durable provider boundary.
///
/// # Errors
///
/// Returns a typed readiness error when discovery is stale, unavailable, or
/// does not advertise Fork for the exact provider.
pub fn require_fork_eligible(
    registry: &HostRegistry,
    kind: ProviderKind,
) -> Result<(), ProviderReadinessError> {
    let capability = discover_capabilities(registry)
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
    if matches!(capability.status, ProviderCapabilityStatus::Available) && capability.fork {
        Ok(())
    } else {
        Err(ProviderReadinessError {
            kind: capability.kind,
            status: capability.status,
            reason: capability.reason,
        })
    }
}

fn require_new_eligible_from_capabilities(
    _registry: &HostRegistry,
    kind: ProviderKind,
    capabilities: Result<Vec<ProviderCapability>, StateError>,
) -> Result<(), ProviderReadinessError> {
    let capability = capabilities
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

#[cfg(test)]
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
    use std::{cell::Cell, path::PathBuf};

    use super::*;

    fn registry() -> (tempfile::TempDir, HostRegistry) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("state");
        let state = crate::state::fresh_create(&root, &crate::domain::RandomIdGenerator).unwrap();
        (temporary, state.into_host_registry().unwrap())
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
    fn capability_probe_drift_does_not_reregister_the_host() {
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
        let identity_before = registry.identity().unwrap();

        let available =
            discover_capabilities_with(&registry, |program| matches!(program, "codex" | "tmux"))
                .unwrap();
        assert_eq!(
            available
                .iter()
                .find(|capability| capability.kind == ProviderKind::Codex)
                .map(|capability| capability.status),
            Some(ProviderCapabilityStatus::Available)
        );
        assert!(
            require_new_eligible_with(&registry, ProviderKind::Codex, |program| {
                matches!(program, "codex" | "tmux")
            })
            .is_ok()
        );

        // The same persisted registration is observed after the executable
        // probe changes. A capability refresh is evidence only: it cannot
        // mutate or replace host registration identity.
        let unavailable =
            discover_capabilities_with(&registry, |program| program == "tmux").unwrap();
        assert_eq!(
            unavailable
                .iter()
                .find(|capability| capability.kind == ProviderKind::Codex)
                .map(|capability| capability.reason),
            Some(ProviderCapabilityReason::NotInstalled)
        );
        assert_eq!(
            require_new_eligible_with(&registry, ProviderKind::Codex, |program| program == "tmux"),
            Err(ProviderReadinessError {
                kind: ProviderKind::Codex,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::NotInstalled,
            })
        );
        assert_eq!(registry.identity().unwrap(), identity_before);
        assert_eq!(
            registry
                .codex_integration()
                .unwrap()
                .map(|integration| integration.lifecycle),
            Some(IntegrationLifecycle::Ready)
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

    #[test]
    fn injected_opencode_probe_is_independent_of_codex_observer_state() {
        let (_temporary, registry) = registry();
        let capabilities = discover_capabilities_with_probe(
            &registry,
            |program| program == "tmux",
            opencode::InstallationProbe::Available,
        )
        .unwrap();
        assert_eq!(
            capabilities[0].reason,
            ProviderCapabilityReason::NotInstalled
        );
        assert_eq!(capabilities[1].status, ProviderCapabilityStatus::Available);
        assert!(capabilities[1].fresh_launch);
        assert!(capabilities[1].exact_resume);
        assert!(capabilities[1].observe);
        assert!(capabilities[1].metadata_read);
        assert!(!capabilities[1].rename);
        assert!(capabilities[1].fork);
    }

    #[test]
    fn opencode_installation_is_not_ready_without_private_tmux() {
        let (_temporary, registry) = registry();
        let capabilities = discover_capabilities_with_probe(
            &registry,
            |_| false,
            opencode::InstallationProbe::Available,
        )
        .unwrap();
        assert_eq!(
            capabilities[1].reason,
            ProviderCapabilityReason::RuntimePrerequisiteMissing
        );
        assert_eq!(
            capabilities[1].status,
            ProviderCapabilityStatus::Unavailable
        );
        assert!(!capabilities[1].fresh_launch);
    }

    #[test]
    fn cached_installation_evidence_is_reused_while_registry_readiness_refreshes() {
        let (_temporary, mut registry) = registry();
        let codex_calls = Cell::new(0);
        let tmux_calls = Cell::new(0);
        let cache = InstallationProbeCache::probe_with(
            |program| match program {
                "codex" => {
                    codex_calls.set(codex_calls.get() + 1);
                    true
                }
                "tmux" => {
                    tmux_calls.set(tmux_calls.get() + 1);
                    true
                }
                _ => false,
            },
            opencode::InstallationProbe::Available,
        );

        let initial = discover_capabilities_with_installation_cache(&registry, cache).unwrap();
        assert_eq!(codex_calls.get(), 1);
        assert_eq!(tmux_calls.get(), 1);
        assert_eq!(
            initial[0].reason,
            ProviderCapabilityReason::ObserverNotReady
        );

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
        let refreshed = discover_capabilities_with_installation_cache(&registry, cache).unwrap();

        assert_eq!(codex_calls.get(), 1);
        assert_eq!(tmux_calls.get(), 1);
        assert_eq!(refreshed[0].status, ProviderCapabilityStatus::Available);
        assert_eq!(refreshed[1].status, ProviderCapabilityStatus::Available);
    }
}
