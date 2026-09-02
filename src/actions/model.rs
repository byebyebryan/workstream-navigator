use super::{
    AppServerError, Error, HostRegistry, IntegrationLifecycle, ObserverProfile, OpenCodeError,
    PathBuf, ProfileError, ProviderKind, Revision, StateError, WorkstreamId, env,
};

/// The durable outcome of a start-or-resume request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    Started,
    AlreadyLive,
}

pub(super) fn ensure_workstream_revision(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<(), ActionError> {
    let Some(expected_revision) = expected_revision else {
        return Ok(());
    };
    let current = workstream_revision(registry, workstream_id)?;
    if current != expected_revision {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    Ok(())
}

pub(super) fn require_codex_provider(provider: ProviderKind) -> Result<(), ActionError> {
    if provider == ProviderKind::Codex {
        Ok(())
    } else {
        Err(ActionError::UnsupportedProvider(provider))
    }
}

pub(super) fn workstream_revision(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<Revision, ActionError> {
    registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .map(|overview| overview.revision)
        .ok_or(ActionError::UnknownWorkstream)
}

pub(super) fn workstream_overview(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<crate::state::WorkstreamOverview, ActionError> {
    registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(ActionError::UnknownWorkstream)
}

pub(super) fn active_workstream_overview(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<crate::state::WorkstreamOverview, ActionError> {
    let overview = workstream_overview(registry, workstream_id)?;
    if overview.archived_at_millis.is_some() {
        return Err(ActionError::WorkstreamArchived);
    }
    Ok(overview)
}

pub(super) fn observer_profile(
    root: &crate::state::StateRoot,
) -> Result<ObserverProfile, ActionError> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or(ActionError::CodexHomeUnavailable)?;
    let executable = env::current_exe().map_err(ActionError::Io)?;
    Ok(ObserverProfile::new(codex_home, executable, root.base()))
}

/// Reconciles a completed native `/hooks` review into the durable observer
/// lifecycle before a managed native action begins.
///
/// Codex owns the trust record in the exact observer-profile suffix. This
/// function only records that already-verified native decision; it never
/// installs, changes, or trusts a hook declaration itself.
///
/// # Errors
///
/// Returns an error when the owned observer profile cannot be verified or the
/// resulting lifecycle transition cannot be recorded atomically.
pub fn reconcile_observer_trust(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), ActionError> {
    let manager = observer_profile(root)?;
    reconcile_observer_trust_with_manager(registry, &manager)
}

pub(super) fn reconcile_observer_trust_with_manager(
    registry: &mut HostRegistry,
    manager: &ObserverProfile,
) -> Result<(), ActionError> {
    let Some(integration) = registry.codex_integration()? else {
        return Ok(());
    };

    match manager.verify_native_trust(&integration.ownership) {
        Ok(()) if integration.lifecycle == IntegrationLifecycle::TrustPending => {
            registry
                .record_codex_integration(integration.ownership, IntegrationLifecycle::Ready)?;
        }
        Err(ProfileError::NativeTrustPending)
            if integration.lifecycle == IntegrationLifecycle::Ready =>
        {
            registry.record_codex_integration(
                integration.ownership,
                IntegrationLifecycle::TrustPending,
            )?;
        }
        Ok(()) | Err(ProfileError::NativeTrustPending) => {}
        Err(error) => return Err(ActionError::Profile(error)),
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ActionError {
    #[error(transparent)]
    ProviderReadiness(crate::provider::ProviderReadinessError),
    #[error("provider {0} does not support this action in the active V1 slice")]
    UnsupportedProvider(ProviderKind),
    #[error("provider {0} does not expose the bounded native recovery flow")]
    ProviderRecoveryUnavailable(ProviderKind),
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("workstream {0} has no runtime")]
    NoRuntime(WorkstreamId),
    #[error("workstream {0} has no current provider conversation")]
    NoProviderBinding(WorkstreamId),
    #[error("observer profile is not installed; open wsnav to activate it")]
    ObserverNotInstalled,
    #[error(
        "observer profile trust is pending; open wsnav and complete native Codex /hooks review"
    )]
    ObserverNotReady,
    #[error("private runtime probe is ambiguous; refusing to create another provider process")]
    RuntimeProbeAmbiguous,
    #[error("OpenCode provider did not become ready before the bounded startup deadline")]
    OpenCodeProviderReadinessTimeout,
    #[error("OpenCode observer did not become ready before the bounded startup deadline")]
    OpenCodeObserverReadinessTimeout,
    #[error("OpenCode observer failed during startup")]
    OpenCodeObserverStartupFailed,
    #[error("OpenCode observer identity changed during startup")]
    OpenCodeObserverIdentityChanged,
    #[error("OpenCode observer exited before becoming ready")]
    OpenCodeObserverExitedBeforeReady,
    #[error("private runtime disappeared; select native recovery before continuing")]
    NativeRecoveryRequired,
    #[error("workstream is not awaiting native recovery")]
    NativeRecoveryUnavailable,
    #[error("workstream is unknown")]
    UnknownWorkstream,
    #[error("workstream is archived; restore it before continuing")]
    WorkstreamArchived,
    #[error("workstream is already archived")]
    WorkstreamAlreadyArchived,
    #[error("workstream revision changed; refresh before acting")]
    WorkstreamRevisionConflict,
    #[error(
        "OpenCode session creation external effect is unknown; no retry was attempted; this Workstream requires explicit cleanup"
    )]
    OpenCodeSessionCreationExternalEffectUnknown,
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    AppServer(#[from] AppServerError),
    #[error(transparent)]
    OpenCode(#[from] OpenCodeError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    State(#[from] StateError),
}
