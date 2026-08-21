use super::{
    Command, HostRegistry, IntegrationLifecycle, LinuxProcessProbe,
    OBSERVER_PROFILE_SCHEMA_VERSION, ObserverProfile, PrivateRuntime, RuntimePaths, RuntimeProbe,
    StateRoot, SystemTmux, fs,
};
use super::{launch::provider_wait, local::observer_profile, model::AppError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverActivation {
    Ready,
    ReviewRequired,
}

/// Refuses observer profile mutation unless every retained Runtime has an
/// exact private-tmux absence proof. SQL lifecycle alone is staleable state;
/// an unavailable or ambiguous native probe is treated as live.
fn require_no_live_runtime(
    root: &StateRoot,
    registry: &HostRegistry,
    refusal: AppError,
) -> Result<(), AppError> {
    let overviews = registry.workstream_overviews()?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    for overview in overviews {
        let Some(runtime_record) = overview.runtime else {
            continue;
        };
        let Ok(paths) = RuntimePaths::for_record(
            root.base(),
            runtime_record.runtime_id,
            &runtime_record.tmux_session,
        ) else {
            return Err(refusal);
        };
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
        let Ok(probe) = runtime.probe() else {
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

/// Reconciles the exact observer declaration before native work can begin.
///
/// `wsnav` itself is the explicit user intent for this bounded setup action.
/// It never trusts a hook, rewrites an unowned declaration, or changes a
/// profile while a managed Runtime is live.
pub(crate) fn prepare_observer_activation(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<ObserverActivation, AppError> {
    let manager = observer_profile(root)?;
    prepare_observer_activation_with_manager(root, registry, &manager)
}

pub(super) fn prepare_observer_activation_with_manager(
    root: &StateRoot,
    registry: &mut HostRegistry,
    manager: &ObserverProfile,
) -> Result<ObserverActivation, AppError> {
    let existing = registry.codex_integration()?;
    let Some(integration) = existing else {
        require_no_live_runtime(
            root,
            registry,
            AppError::LiveRuntimePreventsObserverActivation,
        )?;
        let ownership = manager.install(uuid::Uuid::new_v4().to_string(), None)?;
        registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?;
        return Ok(ObserverActivation::ReviewRequired);
    };

    if integration.ownership.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
        require_no_live_runtime(
            root,
            registry,
            AppError::LiveRuntimePreventsObserverActivation,
        )?;
        let ownership = manager.update(&integration.ownership)?;
        registry.replace_codex_integration(
            &integration.ownership,
            ownership,
            IntegrationLifecycle::TrustPending,
        )?;
        return Ok(ObserverActivation::ReviewRequired);
    }

    let ownership = match manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    ) {
        Ok(ownership) => ownership,
        Err(crate::provider::codex::profile::ProfileError::OwnershipMismatch) => {
            require_no_live_runtime(
                root,
                registry,
                AppError::LiveRuntimePreventsObserverActivation,
            )?;
            let ownership = manager.update(&integration.ownership)?;
            registry.replace_codex_integration(
                &integration.ownership,
                ownership,
                IntegrationLifecycle::TrustPending,
            )?;
            return Ok(ObserverActivation::ReviewRequired);
        }
        Err(error) => return Err(AppError::Profile(error)),
    };
    if finalize_native_trust(registry, manager, &ownership)? {
        return Ok(ObserverActivation::Ready);
    }
    require_no_live_runtime(
        root,
        registry,
        AppError::LiveRuntimePreventsObserverActivation,
    )?;
    if integration.lifecycle != IntegrationLifecycle::TrustPending {
        registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?;
    }
    Ok(ObserverActivation::ReviewRequired)
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

/// Verifies Codex's own completed native review before recording this observer
/// as usable. `false` means the exact owned profile is still untrusted; other
/// profile errors fail closed instead of starting or marking an observer ready.
pub(super) fn finalize_native_trust(
    registry: &mut HostRegistry,
    manager: &ObserverProfile,
    ownership: &crate::provider::codex::profile::ProfileOwnership,
) -> Result<bool, AppError> {
    match manager.verify_native_trust(ownership) {
        Ok(()) => {
            let ownership = manager.install(ownership.owner_id.clone(), Some(ownership))?;
            registry.record_codex_integration(ownership, IntegrationLifecycle::Ready)?;
            Ok(true)
        }
        Err(crate::provider::codex::profile::ProfileError::NativeTrustPending) => Ok(false),
        Err(error) => Err(AppError::Profile(error)),
    }
}

/// Runs only in the presentation's provider pane. Native Codex owns every
/// visible byte while the user reviews the exact hook declaration. After exit,
/// this helper silently reconciles native trust and returns the pane to its
/// blank wait state.
pub(super) fn observer_review(root: &StateRoot) -> Result<(), AppError> {
    observer_review_once(root);
    provider_wait()
}

pub(super) fn observer_review_once(root: &StateRoot) {
    let _ = native_trust_review_in_provider_pane(root);
    let _ = reconcile_observer_review(root);
}

fn reconcile_observer_review(root: &StateRoot) -> Result<(), AppError> {
    let state = crate::state::open_current_only(&StateRoot::select(root.base()))?;
    let mut registry = state.into_host_registry()?;
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let manager = observer_profile(root)?;
    let _ = finalize_native_trust(&mut registry, &manager, &integration.ownership)?;
    Ok(())
}

fn native_trust_review_in_provider_pane(root: &StateRoot) -> Result<(), AppError> {
    let review_root = root.base().join("review");
    fs::create_dir_all(&review_root).map_err(AppError::Io)?;
    let review_cwd = review_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&review_cwd).map_err(AppError::Io)?;
    let result = Command::new("codex")
        .args(["--profile", "wsnav-observer", "-C"])
        .arg(&review_cwd)
        .status()
        .map_err(AppError::Io);
    let remove = fs::remove_dir_all(&review_cwd).map_err(AppError::Io);
    let _ = fs::remove_dir(&review_root);
    result?;
    remove?;
    Ok(())
}
