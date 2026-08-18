use super::{
    BTreeMap, Command, HostRegistry, IntegrationLifecycle, LinuxProcessProbe, NativeLaunch,
    OBSERVER_PROFILE_SCHEMA_VERSION, ObserverProfile, PrivateRuntime, RuntimeId, RuntimePaths,
    StateRoot, SystemTmux, actions, fs,
};
use super::{launch::provider_wait, local::observer_profile, model::AppError};

pub(super) fn setup(
    root: &StateRoot,
    registry: &mut HostRegistry,
    skip_review: bool,
) -> Result<(), AppError> {
    match prepare_observer_activation(root, registry)? {
        ObserverActivation::Ready => {
            println!("observer profile is already ready");
            Ok(())
        }
        ObserverActivation::ReviewRequired if skip_review => {
            println!("observer profile installed; native hook trust remains pending");
            Ok(())
        }
        ObserverActivation::ReviewRequired => {
            native_trust_review(root)?;
            let integration = registry
                .codex_integration()?
                .ok_or(AppError::ObserverNotInstalled)?;
            let manager = observer_profile(root)?;
            if finalize_native_trust(registry, &manager, &integration.ownership)? {
                println!("observer profile is ready");
                Ok(())
            } else {
                Err(AppError::NativeTrustReviewIncomplete)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverActivation {
    Ready,
    ReviewRequired,
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
    prepare_observer_activation_with_manager(registry, &manager)
}

pub(super) fn prepare_observer_activation_with_manager(
    registry: &mut HostRegistry,
    manager: &ObserverProfile,
) -> Result<ObserverActivation, AppError> {
    let existing = registry.codex_integration()?;
    let Some(integration) = existing else {
        if registry.has_live_runtime()? {
            return Err(AppError::LiveRuntimePreventsObserverActivation);
        }
        let ownership = manager.install(uuid::Uuid::new_v4().to_string(), None)?;
        registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?;
        return Ok(ObserverActivation::ReviewRequired);
    };

    if integration.ownership.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
        if registry.has_live_runtime()? {
            return Err(AppError::LiveRuntimePreventsObserverActivation);
        }
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
            if registry.has_live_runtime()? {
                return Err(AppError::LiveRuntimePreventsObserverActivation);
            }
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
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsObserverActivation);
    }
    if integration.lifecycle != IntegrationLifecycle::TrustPending {
        registry.record_codex_integration(ownership, IntegrationLifecycle::TrustPending)?;
    }
    Ok(ObserverActivation::ReviewRequired)
}

pub(super) fn update_observer(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), AppError> {
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsUpdate);
    }
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let ownership = observer_profile(root)?.update(&integration.ownership)?;
    if ownership == integration.ownership {
        println!("observer profile is already current");
        return Ok(());
    }
    registry.replace_codex_integration(
        &integration.ownership,
        ownership,
        IntegrationLifecycle::TrustPending,
    )?;
    println!("observer profile updated; open a fresh wsnav to complete native hook review");
    Ok(())
}

pub(super) fn doctor(root: &StateRoot, registry: &mut HostRegistry) -> Result<(), AppError> {
    actions::reconcile_observer_trust(root, registry)?;
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
    match manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    ) {
        Err(crate::provider::codex::profile::ProfileError::UpdateRequired) => {
            println!("observer: update required");
            return Ok(());
        }
        Err(error) => return Err(AppError::Profile(error)),
        Ok(_) => {}
    }
    if integration.lifecycle == IntegrationLifecycle::Ready
        && manager.verify_native_trust(&integration.ownership).is_err()
    {
        println!("observer: trust pending");
        return Ok(());
    }
    println!("observer: {:?}", integration.lifecycle);
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
/// accepted provider-owned model prefix. The remote control service uses this
/// silent helper so its protocol stdout remains one framed response.
pub(crate) fn remove_observer_exact(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), AppError> {
    if registry.has_live_runtime()? {
        return Err(AppError::LiveRuntimePreventsRemoval);
    }
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    observer_profile(root)?.remove(&integration.ownership)?;
    registry.remove_codex_integration(&integration.ownership)?;
    Ok(())
}

pub(super) fn trust_observer(
    root: &StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), AppError> {
    let integration = registry
        .codex_integration()?
        .ok_or(AppError::ObserverNotInstalled)?;
    let manager = observer_profile(root)?;
    if finalize_native_trust(registry, &manager, &integration.ownership)? {
        println!("observer profile marked ready");
        Ok(())
    } else {
        Err(AppError::NativeTrustReviewIncomplete)
    }
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
    let mut registry = HostRegistry::open(root)?;
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

fn native_trust_review(root: &StateRoot) -> Result<(), AppError> {
    let review_root = root.base().join("review");
    fs::create_dir_all(&review_root).map_err(AppError::Io)?;
    let review_cwd = review_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&review_cwd).map_err(AppError::Io)?;

    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), RuntimeId::new()),
    );
    let launch = NativeLaunch {
        cwd: review_cwd.clone(),
        program: vec![
            "codex".into(),
            "--profile".into(),
            "wsnav-observer".into(),
            "-C".into(),
            review_cwd.clone().into_os_string(),
        ],
        environment: BTreeMap::new(),
    };
    if let Err(error) = runtime.start(&launch) {
        let _ = runtime.park();
        let _ = fs::remove_dir_all(&review_cwd);
        let _ = fs::remove_dir(&review_root);
        return Err(AppError::Runtime(error));
    }
    let attach = runtime
        .prepare_attach()
        .map_err(AppError::Runtime)
        .and_then(|()| runtime.attach_command().status().map_err(AppError::Io));
    let park = runtime.park();
    let remove = fs::remove_dir_all(&review_cwd).map_err(AppError::Io);
    let _ = fs::remove_dir(&review_root);
    attach?;
    park?;
    remove?;
    Ok(())
}
