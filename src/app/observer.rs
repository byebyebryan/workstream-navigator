use super::{
    HostRegistry, LinuxProcessProbe, OBSERVER_PROFILE_SCHEMA_VERSION, PrivateRuntime, RuntimePaths,
    RuntimeProbe, StateRoot, SystemTmux,
};
use super::{local::observer_profile, model::AppError};
use crate::{
    domain::{ProviderKind, Revision},
    provider::codex::profile::{ObserverProfile, ProfileInspection},
    state::{
        CodexIntegration, D16State, IntegrationLifecycle, ProvisionalLease, RuntimeRecord,
        StateError,
    },
};
use thiserror::Error;

/// Read-only classification of the exact Codex observer contract.  The
/// readiness probe is intentionally separate from activation: opening the
/// Navigator, a passive command, or a shell gate never installs a profile or
/// changes native trust.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverReadiness {
    Ready,
    /// Native trust is present in the profile but the durable lifecycle row
    /// still needs its exact revision-fenced finalization. This is a crash
    /// recovery state, not a reason to run native review again.
    TrustFinalizationRequired,
    SetupRequired,
    TrustReviewRequired,
    UpdateRequired,
    Modified,
    Disabled,
    Foreign,
    Ambiguous,
    Unknown,
}

impl ObserverReadiness {
    #[must_use]
    pub(crate) const fn needs_interactive_setup(self) -> bool {
        matches!(
            self,
            Self::SetupRequired
                | Self::TrustFinalizationRequired
                | Self::TrustReviewRequired
                | Self::UpdateRequired
        )
    }
}

/// Bounded evidence captured for one contextual observer request.  Paths and
/// provider payloads are deliberately absent from the public projection; the
/// retained ownership record is used only for exact revalidation by the
/// activation/finalization owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObserverReadinessEvidence {
    pub(crate) readiness: ObserverReadiness,
    pub(crate) integration_revision: Option<Revision>,
    pub(crate) integration: Option<CodexIntegration>,
}

/// Result of an explicit, interactive profile preparation.  A review is
/// always native Codex UI; `WSNav` records only bounded trust state after the
/// native process exits and never captures its output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ObserverActivation {
    Ready(CodexIntegration),
    ReviewRequired(CodexIntegration),
}

#[derive(Debug, Error)]
pub(crate) enum ObserverActivationError {
    #[error("D17 observer state is unavailable")]
    State(#[from] StateError),
    #[error("D17 observer profile is unavailable")]
    Profile(#[from] crate::provider::codex::profile::ProfileError),
    #[error("D17 observer profile mutation is blocked by a live runtime")]
    LiveRuntime,
    #[error("D17 observer readiness changed before activation")]
    EvidenceChanged,
    #[error("D17 observer readiness does not permit interactive activation")]
    NotReady,
}

/// Refuses observer profile mutation unless every retained Codex Runtime has
/// an exact private-tmux absence proof. `OpenCode` Runtime state is independent
/// of the Codex observer profile. SQL lifecycle alone is staleable state; an
/// unavailable or ambiguous Codex probe is treated as live.
fn require_no_live_runtime(
    root: &StateRoot,
    registry: &HostRegistry,
    refusal: AppError,
) -> Result<(), AppError> {
    require_no_live_runtime_with_probe(root, registry, refusal, |root, runtime_record| {
        let paths = RuntimePaths::for_record(
            root.base(),
            runtime_record.runtime_id,
            &runtime_record.tmux_session,
        )
        .map_err(|_| ())?;
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
        runtime.probe().map_err(|_| ())
    })
}

fn require_no_live_runtime_with_probe<F>(
    root: &StateRoot,
    registry: &HostRegistry,
    refusal: AppError,
    mut probe: F,
) -> Result<(), AppError>
where
    F: FnMut(&StateRoot, &RuntimeRecord) -> Result<RuntimeProbe, ()>,
{
    let overviews = registry.workstream_overviews()?;
    for overview in overviews {
        // Codex profile ownership is independent from OpenCode's provider
        // Runtime.  A live or ambiguous OpenCode session must not make a
        // Codex setup/removal request fail; every Codex Runtime still needs
        // its own exact private-tmux absence proof below.
        if overview.provider != ProviderKind::Codex {
            continue;
        }
        let Some(runtime_record) = overview.runtime else {
            continue;
        };
        if runtime_record.provider != ProviderKind::Codex {
            return Err(refusal);
        }
        let Ok(probe) = probe(root, &runtime_record) else {
            return Err(refusal);
        };
        if matches!(
            probe,
            RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. }
        ) {
            return Err(refusal);
        }
    }
    Ok(())
}

/// Reads the observer contract through the schema-14 D17 state boundary.
///
/// This is the only readiness classification used by a Codex launch request.
/// It intentionally treats missing ownership, foreign files, changed files,
/// disabled integrations, and unreadable evidence as non-ready rather than
/// attempting a best-effort repair.
pub(crate) fn observer_readiness(
    root: &StateRoot,
    state: &D16State,
) -> Result<ObserverReadinessEvidence, ObserverActivationError> {
    let integration = state.d17_codex_integration()?;
    let manager = match integration.as_ref() {
        Some(integration) => Some(observer_profile_for(root, Some(integration))?),
        None => Some(observer_profile(root).map_err(|_| ObserverActivationError::NotReady)?),
    };
    let manager = manager.ok_or(ObserverActivationError::NotReady)?;
    observer_readiness_with_profile(state, &manager)
}

/// Deterministic readiness seam for production adapters and tests. The
/// injected manager is still read-only; no profile setup or trust mutation can
/// happen through this boundary.
pub(crate) fn observer_readiness_with_profile(
    state: &D16State,
    manager: &ObserverProfile,
) -> Result<ObserverReadinessEvidence, ObserverActivationError> {
    let integration = state.d17_codex_integration()?;
    classify_observer_readiness(integration, Some(manager))
}

fn classify_observer_readiness(
    integration: Option<CodexIntegration>,
    manager: Option<&ObserverProfile>,
) -> Result<ObserverReadinessEvidence, ObserverActivationError> {
    let readiness = match integration.as_ref() {
        None => {
            let manager = manager.ok_or(ObserverActivationError::NotReady)?;
            match manager.inspect(None)? {
                ProfileInspection::Missing => ObserverReadiness::SetupRequired,
                ProfileInspection::Foreign
                | ProfileInspection::Modified
                | ProfileInspection::UpdateRequired
                | ProfileInspection::TrustPending
                | ProfileInspection::Ready => ObserverReadiness::Foreign,
            }
        }
        Some(integration) => {
            if integration.lifecycle == IntegrationLifecycle::Disabled {
                ObserverReadiness::Disabled
            } else {
                match integration.ownership.canonical_path.parent() {
                    None => ObserverReadiness::Ambiguous,
                    Some(codex_home) if !codex_home.is_absolute() => ObserverReadiness::Ambiguous,
                    Some(_codex_home) => {
                        let manager = manager.ok_or(ObserverActivationError::NotReady)?;
                        let inspection = match manager.inspect(Some(&integration.ownership)) {
                            Ok(inspection) => inspection,
                            Err(_) => ProfileInspection::Modified,
                        };
                        match inspection {
                            ProfileInspection::Missing => ObserverReadiness::Unknown,
                            ProfileInspection::Foreign => ObserverReadiness::Foreign,
                            ProfileInspection::Modified => ObserverReadiness::Modified,
                            ProfileInspection::UpdateRequired => ObserverReadiness::UpdateRequired,
                            ProfileInspection::TrustPending => {
                                if integration.lifecycle == IntegrationLifecycle::Modified {
                                    ObserverReadiness::Modified
                                } else {
                                    ObserverReadiness::TrustReviewRequired
                                }
                            }
                            ProfileInspection::Ready => match integration.lifecycle {
                                IntegrationLifecycle::Ready => ObserverReadiness::Ready,
                                IntegrationLifecycle::TrustPending => {
                                    ObserverReadiness::TrustFinalizationRequired
                                }
                                IntegrationLifecycle::Modified => ObserverReadiness::Modified,
                                IntegrationLifecycle::Disabled => ObserverReadiness::Disabled,
                            },
                        }
                    }
                }
            }
        }
    };
    Ok(ObserverReadinessEvidence {
        readiness,
        integration_revision: integration.as_ref().map(|record| record.revision),
        integration,
    })
}

fn observer_profile_for(
    root: &StateRoot,
    integration: Option<&CodexIntegration>,
) -> Result<ObserverProfile, ObserverActivationError> {
    if let Some(integration) = integration {
        let codex_home = integration
            .ownership
            .canonical_path
            .parent()
            .filter(|path| path.is_absolute())
            .ok_or(ObserverActivationError::NotReady)?;
        let executable = std::env::current_exe().map_err(|_| ObserverActivationError::NotReady)?;
        Ok(ObserverProfile::new(codex_home, executable, root.base()))
    } else {
        observer_profile(root).map_err(|_| ObserverActivationError::NotReady)
    }
}

/// Applies one exact owned-profile create/update after a caller has shown an
/// interactive consent surface.  Trust is never written here: Codex's native
/// `/hooks` review remains the sole authority for trust, and the returned row
/// stays `trust_pending` until finalization.
pub(crate) fn prepare_observer_activation_d17(
    root: &StateRoot,
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    evidence: &ObserverReadinessEvidence,
) -> Result<ObserverActivation, ObserverActivationError> {
    let manager = observer_profile_for(root, evidence.integration.as_ref())?;
    prepare_observer_activation_d17_with_profile(root, state, provisional_lease, evidence, &manager)
}

/// Lease-held activation seam with an explicitly selected profile manager.
/// Production callers use [`prepare_observer_activation_d17`], while
/// disposable tests can keep their Codex home out of ambient environment.
pub(crate) fn prepare_observer_activation_d17_with_profile(
    root: &StateRoot,
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    evidence: &ObserverReadinessEvidence,
    manager: &ObserverProfile,
) -> Result<ObserverActivation, ObserverActivationError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let current = state.d17_codex_integration()?;
    if current != evidence.integration {
        return Err(ObserverActivationError::EvidenceChanged);
    }
    let observed = observer_readiness_with_profile(state, manager)?;
    if observed != *evidence {
        return Err(ObserverActivationError::EvidenceChanged);
    }
    if evidence.readiness == ObserverReadiness::Ready {
        return Ok(ObserverActivation::Ready(
            evidence
                .integration
                .clone()
                .ok_or(ObserverActivationError::EvidenceChanged)?,
        ));
    }
    if !evidence.readiness.needs_interactive_setup() {
        return Err(ObserverActivationError::NotReady);
    }

    let mut registry = {
        // Keep the lease and the D17 state handle alive while the exact
        // profile mutation is performed.  The registry conversion consumes
        // only the SQLite handle, not the independent provisional lease.
        let state_for_registry = std::mem::replace(
            state,
            crate::state::open_d17_current_only(root).map_err(ObserverActivationError::State)?,
        );
        state_for_registry.into_d17_host_registry()?
    };
    require_no_live_runtime(root, &registry, AppError::LiveRuntimePreventsRemoval)
        .map_err(|_| ObserverActivationError::LiveRuntime)?;

    let integration = match evidence.readiness {
        ObserverReadiness::SetupRequired => {
            let owner_id = uuid::Uuid::new_v4().to_string();
            let ownership = manager.install(owner_id, None)?;
            registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?
        }
        ObserverReadiness::UpdateRequired => {
            let expected = evidence
                .integration
                .as_ref()
                .ok_or(ObserverActivationError::EvidenceChanged)?;
            let ownership = manager.update(&expected.ownership)?;
            registry.replace_codex_integration(
                &expected.ownership,
                ownership,
                IntegrationLifecycle::TrustPending,
            )?
        }
        ObserverReadiness::TrustReviewRequired => evidence
            .integration
            .clone()
            .ok_or(ObserverActivationError::EvidenceChanged)?,
        ObserverReadiness::TrustFinalizationRequired => {
            let expected = evidence
                .integration
                .as_ref()
                .ok_or(ObserverActivationError::EvidenceChanged)?;
            manager.verify_native_trust(&expected.ownership)?;
            provisional_lease.revalidate_for_mutation(root.base())?;
            let replacement = crate::state::open_d17_current_only(root)
                .map_err(ObserverActivationError::State)?;
            let state_for_registry = std::mem::replace(state, replacement);
            let mut registry = state_for_registry.into_d17_host_registry()?;
            let ready =
                registry.set_codex_integration_lifecycle(expected, IntegrationLifecycle::Ready)?;
            return Ok(ObserverActivation::Ready(ready));
        }
        _ => return Err(ObserverActivationError::NotReady),
    };
    provisional_lease.revalidate_for_mutation(root.base())?;
    Ok(ObserverActivation::ReviewRequired(integration))
}

/// Lease-held native-trust finalization. Callers that already
/// proved the presentation marker can use this form to keep that proof and
/// the lifecycle revision fence in one linear transaction boundary.
pub(crate) fn finalize_observer_trust_d17_under_lease(
    root: &StateRoot,
    state: D16State,
    lease: &ProvisionalLease,
    expected: &CodexIntegration,
) -> Result<CodexIntegration, ObserverActivationError> {
    lease.revalidate_for_mutation(root.base())?;
    let current = state
        .d17_codex_integration()?
        .ok_or(ObserverActivationError::EvidenceChanged)?;
    if current != *expected {
        return Err(ObserverActivationError::EvidenceChanged);
    }
    let manager = observer_profile_for(root, Some(&current))?;
    manager.verify_native_trust(&current.ownership)?;
    lease.revalidate_for_mutation(root.base())?;
    let mut registry = state.into_d17_host_registry()?;
    registry
        .set_codex_integration_lifecycle(&current, IntegrationLifecycle::Ready)
        .map_err(ObserverActivationError::State)
}

pub(super) fn doctor(root: &StateRoot, registry: &HostRegistry) -> Result<(), AppError> {
    let integration = registry.codex_integration()?;
    let Some(integration) = integration else {
        println!("observer: not installed");
        return Ok(());
    };
    if integration.ownership.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
        println!("observer: update required");
        return Ok(());
    }
    let manager = observer_profile(root)?;
    let inspection = manager
        .inspect(Some(&integration.ownership))
        .map_err(AppError::Profile)?;
    match inspection {
        crate::provider::codex::profile::ProfileInspection::UpdateRequired => {
            println!("observer: update required");
        }
        crate::provider::codex::profile::ProfileInspection::TrustPending => {
            println!("observer: trust pending");
        }
        crate::provider::codex::profile::ProfileInspection::Ready => {
            println!("observer: {:?}", integration.lifecycle);
        }
        crate::provider::codex::profile::ProfileInspection::Missing => {
            println!("observer: owned profile is missing");
        }
        crate::provider::codex::profile::ProfileInspection::Foreign => {
            println!("observer: profile path is foreign");
        }
        crate::provider::codex::profile::ProfileInspection::Modified => {
            println!("observer: owned profile is modified");
        }
    }
    Ok(())
}

pub(super) fn remove_observer(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), AppError> {
    remove_observer_exact(root, registry)?;
    println!("observer integration removed; any provider model settings were preserved");
    Ok(())
}

/// Removes only the exact observer declaration and native trust, preserving an
/// accepted provider-owned model prefix. This helper is silent so the native
/// provider pane never receives control-plane diagnostics.
pub(crate) fn remove_observer_exact(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), AppError> {
    require_no_live_runtime(root, registry, AppError::LiveRuntimePreventsRemoval)?;
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    observer_profile(root)?.remove(&integration.ownership)?;
    registry.remove_codex_integration(&integration.ownership)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, fs, path::Path, path::PathBuf};

    use sha2::{Digest, Sha256};

    use super::{
        ObserverActivation, ObserverReadiness, observer_readiness_with_profile,
        prepare_observer_activation_d17_with_profile, require_no_live_runtime_with_probe,
    };
    use crate::{
        app::AppError,
        domain::RandomIdGenerator,
        provider::codex::profile::{
            OBSERVER_PROFILE_NAME, ObserverProfile, ProfileInspection, ProfileOwnership,
        },
        runtime::RuntimeProbe,
        state::{
            IntegrationLifecycle, StateRoot, fresh_create, fresh_create_d17,
            migrate_current_to_d17, open_d17_current_only,
        },
    };

    fn fixture() -> (
        tempfile::TempDir,
        StateRoot,
        crate::provider::codex::profile::ObserverProfile,
    ) {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let codex_home = temporary.path().join("codex");
        fs::create_dir(&codex_home).unwrap();
        let executable = std::env::current_exe().unwrap();
        drop(fresh_create_d17(&state_path, &RandomIdGenerator).unwrap());
        let root = StateRoot::select(&state_path);
        let manager = ObserverProfile::new(codex_home, executable, state_path);
        (temporary, root, manager)
    }

    fn complete_native_hook_suffix(manager: &ObserverProfile) -> String {
        let mut suffix = String::from("\n[hooks.state]\n");
        for hook in ["session_start", "user_prompt_submit", "stop", "session_end"] {
            let key =
                serde_json::to_string(&format!("{}:{hook}:0:0", manager.path().display())).unwrap();
            writeln!(
                suffix,
                "\n[hooks.state.{key}]\ntrusted_hash = \"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""
            )
            .unwrap();
        }
        suffix
    }

    fn registered_codex_runtime_fixture() -> (
        tempfile::TempDir,
        StateRoot,
        crate::state::HostRegistry,
        crate::state::ProjectLocationWorkstreamRegistration,
    ) {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let mut state = fresh_create(&state_path, &RandomIdGenerator).unwrap();
        let registration = state
            .register_project_location_with_initial_workstream(
                Path::new("/fixture/project"),
                "fixture project",
                None,
                None,
                crate::domain::ProviderKind::Codex,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);
        let root = StateRoot::select(&state_path);
        migrate_current_to_d17(&root).unwrap();
        let state = open_d17_current_only(&root).unwrap();
        let registry = state.into_d17_host_registry().unwrap();
        (temporary, root, registry, registration)
    }

    fn live_probe() -> RuntimeProbe {
        RuntimeProbe::Live {
            pane_id: "%1".to_owned(),
            pane_pid: 1,
            cwd: PathBuf::from("/fixture/project"),
            process_birth: None,
        }
    }

    #[test]
    fn live_opencode_runtime_does_not_block_codex_observer_mutation_fence() {
        let (_temporary, root, mut registry, registration) = registered_codex_runtime_fixture();
        let open_code = registry
            .create_independent_workstream(
                "opencode-runtime",
                registration.workstream.workstream_id,
                crate::domain::Revision::INITIAL,
                crate::domain::ProviderKind::OpenCode,
            )
            .unwrap();
        registry
            .reserve_runtime_with_provider(
                open_code.workstream_id,
                crate::domain::ProviderKind::OpenCode,
            )
            .unwrap();

        let mut probed_providers = Vec::new();
        let result = require_no_live_runtime_with_probe(
            &root,
            &registry,
            AppError::LiveRuntimePreventsRemoval,
            |_, runtime| {
                probed_providers.push(runtime.provider);
                Ok(live_probe())
            },
        );

        assert!(result.is_ok());
        assert!(probed_providers.is_empty());
    }

    #[test]
    fn live_codex_runtime_blocks_observer_mutation_fence() {
        let (_temporary, root, mut registry, registration) = registered_codex_runtime_fixture();
        registry
            .reserve_runtime(registration.workstream.workstream_id)
            .unwrap();

        let result = require_no_live_runtime_with_probe(
            &root,
            &registry,
            AppError::LiveRuntimePreventsRemoval,
            |_, runtime| {
                assert_eq!(runtime.provider, crate::domain::ProviderKind::Codex);
                Ok(live_probe())
            },
        );

        assert!(matches!(result, Err(AppError::LiveRuntimePreventsRemoval)));
    }

    #[test]
    fn ambiguous_codex_runtime_blocks_observer_mutation_fence() {
        let (_temporary, root, mut registry, registration) = registered_codex_runtime_fixture();
        registry
            .reserve_runtime(registration.workstream.workstream_id)
            .unwrap();

        let result = require_no_live_runtime_with_probe(
            &root,
            &registry,
            AppError::LiveRuntimePreventsRemoval,
            |_, runtime| {
                assert_eq!(runtime.provider, crate::domain::ProviderKind::Codex);
                Ok(RuntimeProbe::Unknown {
                    diagnostic: "ambiguous private runtime".to_owned(),
                })
            },
        );

        assert!(matches!(result, Err(AppError::LiveRuntimePreventsRemoval)));
    }

    #[test]
    fn fresh_readiness_preflight_is_read_only_before_any_handoff_reservation() {
        let (_temporary, root, manager) = fixture();
        let state = open_d17_current_only(&root).unwrap();

        let evidence = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(evidence.readiness, ObserverReadiness::SetupRequired);
        assert!(evidence.integration.is_none());
        assert!(
            state
                .d17_onboarding_workstream_projections()
                .unwrap()
                .is_empty()
        );

        // Reopening the same schema-14 root proves classification did not
        // create a capability, candidate Runtime, or observer row.
        drop(state);
        let state = open_d17_current_only(&root).unwrap();
        assert!(state.d17_codex_integration().unwrap().is_none());
        assert!(
            state
                .d17_onboarding_workstream_projections()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn fresh_schema14_missing_observer_requires_consent_then_records_trust_pending() {
        let (_temporary, root, manager) = fixture();
        let mut state = open_d17_current_only(&root).unwrap();
        let lease = state.acquire_d17_provisional_lease().unwrap();
        let evidence = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(evidence.readiness, ObserverReadiness::SetupRequired);
        assert!(evidence.integration.is_none());

        let activation = prepare_observer_activation_d17_with_profile(
            &root, &mut state, &lease, &evidence, &manager,
        )
        .unwrap();
        let ObserverActivation::ReviewRequired(integration) = activation else {
            panic!("fresh profile setup must require native trust review");
        };
        assert_eq!(
            integration.lifecycle,
            crate::state::IntegrationLifecycle::TrustPending
        );
        assert_eq!(
            manager.inspect(Some(&integration.ownership)).unwrap(),
            ProfileInspection::TrustPending
        );
        assert_eq!(state.d17_codex_integration().unwrap(), Some(integration));
        assert!(
            manager
                .path()
                .ends_with(format!("{OBSERVER_PROFILE_NAME}.config.toml"))
        );
    }

    #[test]
    fn foreign_fresh_profile_is_refused_without_profile_or_registry_mutation() {
        let (_temporary, root, manager) = fixture();
        fs::write(manager.path(), b"[model]\nname = 'foreign'\n").unwrap();
        let mut state = open_d17_current_only(&root).unwrap();
        let lease = state.acquire_d17_provisional_lease().unwrap();
        let evidence = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(evidence.readiness, ObserverReadiness::Foreign);
        assert!(
            prepare_observer_activation_d17_with_profile(
                &root, &mut state, &lease, &evidence, &manager,
            )
            .is_err()
        );
        assert_eq!(
            fs::read(manager.path()).unwrap(),
            b"[model]\nname = 'foreign'\n"
        );
        assert!(state.d17_codex_integration().unwrap().is_none());
    }

    #[test]
    fn activation_reclassification_after_install_is_trust_review_required() {
        let (_temporary, root, manager) = fixture();
        let mut state = open_d17_current_only(&root).unwrap();
        let lease = state.acquire_d17_provisional_lease().unwrap();
        let evidence = observer_readiness_with_profile(&state, &manager).unwrap();
        let ObserverActivation::ReviewRequired(integration) =
            prepare_observer_activation_d17_with_profile(
                &root, &mut state, &lease, &evidence, &manager,
            )
            .unwrap()
        else {
            panic!("fresh activation must await native review");
        };
        let after = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(after.readiness, ObserverReadiness::TrustReviewRequired);
        assert_eq!(after.integration, Some(integration));
    }

    #[test]
    fn exact_owned_update_required_profile_is_replaced_and_returns_to_trust_pending() {
        let (temporary, root, manager) = fixture();
        let old_executable = temporary.path().join("old-wsnav");
        let old_manager = ObserverProfile::new(
            manager.path().parent().unwrap(),
            &old_executable,
            root.base(),
        );
        let legacy = old_manager
            .rendered()
            .replace(&format!(" --state-root '{}'", root.base().display()), "");
        fs::write(manager.path(), &legacy).unwrap();
        let ownership = ProfileOwnership {
            canonical_path: manager.path(),
            owner_id: "owner".to_owned(),
            profile_schema_version: 1,
            hook_executable: old_executable,
            content_hash: format!("{:x}", Sha256::digest(legacy.as_bytes())),
        };
        let mut state = open_d17_current_only(&root).unwrap();
        let lease = state.acquire_d17_provisional_lease().unwrap();
        let mut registry = {
            let replacement = open_d17_current_only(&root).unwrap();
            std::mem::replace(&mut state, replacement)
                .into_d17_host_registry()
                .unwrap()
        };
        registry
            .record_codex_integration(ownership, IntegrationLifecycle::TrustPending)
            .unwrap();
        drop(registry);

        let evidence = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(evidence.readiness, ObserverReadiness::UpdateRequired);
        let ObserverActivation::ReviewRequired(updated) =
            prepare_observer_activation_d17_with_profile(
                &root, &mut state, &lease, &evidence, &manager,
            )
            .unwrap()
        else {
            panic!("an exact old declaration must require native review after update");
        };
        assert_eq!(updated.lifecycle, IntegrationLifecycle::TrustPending);
        assert_eq!(updated.ownership.profile_schema_version, 2);
        assert_eq!(
            manager.inspect(Some(&updated.ownership)).unwrap(),
            ProfileInspection::TrustPending
        );
    }

    #[test]
    fn modified_owned_profile_is_refused_without_mutation() {
        let (_temporary, root, manager) = fixture();
        let mut state = open_d17_current_only(&root).unwrap();
        let lease = state.acquire_d17_provisional_lease().unwrap();
        let evidence = observer_readiness_with_profile(&state, &manager).unwrap();
        let ObserverActivation::ReviewRequired(integration) =
            prepare_observer_activation_d17_with_profile(
                &root, &mut state, &lease, &evidence, &manager,
            )
            .unwrap()
        else {
            panic!("fresh profile setup must require review");
        };
        fs::write(manager.path(), b"modified by another owner\n").unwrap();
        let changed = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(changed.readiness, ObserverReadiness::Modified);
        assert!(
            prepare_observer_activation_d17_with_profile(
                &root, &mut state, &lease, &changed, &manager,
            )
            .is_err()
        );
        assert_eq!(
            fs::read(manager.path()).unwrap(),
            b"modified by another owner\n"
        );
        assert_eq!(
            state.d17_codex_integration().unwrap().unwrap().ownership,
            integration.ownership
        );
    }

    #[test]
    fn completed_native_review_finalizes_trust_pending_without_second_review() {
        let (_temporary, root, manager) = fixture();
        let mut state = open_d17_current_only(&root).unwrap();
        let lease = state.acquire_d17_provisional_lease().unwrap();
        let missing = observer_readiness_with_profile(&state, &manager).unwrap();
        let ObserverActivation::ReviewRequired(integration) =
            prepare_observer_activation_d17_with_profile(
                &root, &mut state, &lease, &missing, &manager,
            )
            .unwrap()
        else {
            panic!("fresh profile setup must require review");
        };
        fs::write(
            manager.path(),
            format!(
                "{}{}",
                manager.rendered(),
                complete_native_hook_suffix(&manager)
            ),
        )
        .unwrap();
        let pending = observer_readiness_with_profile(&state, &manager).unwrap();
        assert_eq!(
            pending.readiness,
            ObserverReadiness::TrustFinalizationRequired
        );
        let ObserverActivation::Ready(ready) = prepare_observer_activation_d17_with_profile(
            &root, &mut state, &lease, &pending, &manager,
        )
        .unwrap() else {
            panic!("exact native trust must finalize without another review");
        };
        assert_eq!(ready.lifecycle, IntegrationLifecycle::Ready);
        assert_eq!(state.d17_codex_integration().unwrap(), Some(ready.clone()));
        assert_eq!(
            observer_readiness_with_profile(&state, &manager)
                .unwrap()
                .readiness,
            ObserverReadiness::Ready
        );
        assert_eq!(integration.ownership, ready.ownership);
    }
}
