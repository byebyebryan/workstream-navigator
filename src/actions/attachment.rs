use super::{
    ActionError, HostRegistry, LinuxProcessProbe, OpenCodeClient, OpenCodeEndpoint, OpenCodeError,
    PrivateRuntime, ProcessProbe, ProviderKind, ProviderSessionId, RuntimePaths, RuntimeProbe,
    SystemTmux, WorkstreamId, endpoint_owned_by_process, opencode,
};
use super::{
    cleanup::{
        attachment_runtime_matches, clean_missing_stopped_runtime,
        fail_cleanup_unknown_opencode_session_creation,
        fail_known_absent_opencode_session_creation, fail_unknown_opencode_session_creation,
        fail_unlaunched_runtime, matches_recorded_runtime,
    },
    start::backfill_live_runtime_provider_pid,
};

pub(super) enum PriorOpenCodeRuntime {
    AlreadyLive,
    Ready(Option<Box<crate::state::ProviderBinding>>),
}

pub(super) fn inspect_opencode_prior_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<PriorOpenCodeRuntime, ActionError> {
    let Some(mut prior_runtime) = registry.runtime_for_workstream(workstream_id)? else {
        return Ok(PriorOpenCodeRuntime::Ready(None));
    };
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let prior = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(
            root.base(),
            prior_runtime.runtime_id,
            &prior_runtime.tmux_session,
        )?,
    );
    let prior_probe = prior.probe()?;
    if let Some(backfilled) =
        backfill_live_runtime_provider_pid(registry, &prior_runtime, &prior_probe)?
    {
        prior_runtime = backfilled;
    }
    if matches_recorded_runtime(&prior_runtime, &prior_probe, false) {
        return validate_opencode_live_runtime(registry, &prior_runtime, &prior_probe);
    }
    if prior_runtime.provider == ProviderKind::OpenCode
        && matches!(
            prior_probe,
            RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. }
        )
        && let Some(handle) = registry.opencode_runtime_handle(prior_runtime.runtime_id)?
    {
        crate::provider::opencode::mark_unknown_handle(
            registry,
            &handle,
            &prior_runtime.tmux_generation,
        );
    }
    match prior_probe {
        RuntimeProbe::Missing => {
            if registry.has_unresolved_opencode_session_creation(
                prior_runtime.runtime_id,
                &prior_runtime.tmux_generation,
            )? {
                if prior_runtime.status != crate::domain::RuntimeStatus::Unknown {
                    registry.mark_runtime_recovery_required(
                        prior_runtime.runtime_id,
                        prior_runtime.revision,
                    )?;
                }
                return Err(ActionError::ProviderRecoveryUnavailable(
                    ProviderKind::OpenCode,
                ));
            }
            clean_missing_stopped_runtime(&prior, &prior_runtime)?;
            if prior_runtime.process_birth.is_some()
                && !matches!(prior_runtime.status, crate::domain::RuntimeStatus::Stopped)
            {
                registry.mark_runtime_recovery_required(
                    prior_runtime.runtime_id,
                    prior_runtime.revision,
                )?;
                return Err(ActionError::ProviderRecoveryUnavailable(
                    ProviderKind::OpenCode,
                ));
            }
            if !matches!(prior_runtime.status, crate::domain::RuntimeStatus::Stopped) {
                registry.mark_runtime_stopped(prior_runtime.runtime_id, prior_runtime.revision)?;
            }
        }
        RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    }
    registry
        .binding_for_runtime(prior_runtime.runtime_id)
        .map(|binding| PriorOpenCodeRuntime::Ready(binding.map(Box::new)))
        .map_err(ActionError::State)
}

/// An `OpenCode` Runtime may be reported `AlreadyLive` only after every
/// persisted ownership token is corroborated again.  A tmux pane alone is
/// not enough: the exact handle generation/session, observer PID birth,
/// endpoint listener, health, and root-session status must all agree.
pub(super) fn validate_opencode_live_runtime(
    registry: &mut HostRegistry,
    runtime: &crate::state::RuntimeRecord,
    probe: &RuntimeProbe,
) -> Result<PriorOpenCodeRuntime, ActionError> {
    let RuntimeProbe::Live {
        pane_pid,
        cwd,
        process_birth: Some(process_birth),
        ..
    } = probe
    else {
        return Err(ActionError::RuntimeProbeAmbiguous);
    };
    let Some(handle) = registry.opencode_runtime_handle(runtime.runtime_id)? else {
        return Err(ActionError::RuntimeProbeAmbiguous);
    };
    let binding = registry.binding_for_runtime(runtime.runtime_id)?;
    let endpoint = OpenCodeEndpoint::loopback(handle.endpoint_port)?;
    let observer_live = handle
        .observer_pid
        .zip(handle.observer_birth.as_deref())
        .is_some_and(|(pid, birth)| LinuxProcessProbe.process_birth(pid).as_deref() == Some(birth));
    let session_exact = binding.as_ref().is_some_and(|binding| {
        binding.provider == ProviderKind::OpenCode
            && binding.runtime_generation == runtime.tmux_generation
            && binding.native_session_id == handle.native_session_id
    });
    let exact = handle.runtime_generation == runtime.tmux_generation
        && handle.endpoint_host == crate::provider::opencode::LOOPBACK_HOST
        && runtime.status != crate::domain::RuntimeStatus::Stopped
        && handle.observer_status == crate::state::OpenCodeObserverStatus::Ready
        && observer_live
        && cwd == &runtime.cwd
        && runtime.provider_pid == Some(*pane_pid)
        && runtime.process_birth.as_deref() == Some(process_birth)
        && endpoint_owned_by_process(&endpoint, *pane_pid, process_birth)
        && session_exact
        && OpenCodeClient::new(endpoint.clone())
            .health()
            .is_ok_and(|health| health.version == handle.version)
        && matches!(
            OpenCodeClient::new(endpoint)
                .session_status_with_root(&handle.native_session_id, &runtime.cwd),
            Ok(crate::provider::opencode::OpenCodeSessionStatus::Busy
                | crate::provider::opencode::OpenCodeSessionStatus::Idle)
        );
    if !exact {
        crate::provider::opencode::mark_unknown_handle(registry, &handle, &runtime.tmux_generation);
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    Ok(PriorOpenCodeRuntime::AlreadyLive)
}

/// Performs the authoritative, provider-aware checks required immediately
/// before attaching a native provider pane.
///
/// The local presentation path calls this function. A private tmux pane is
/// never enough evidence for
/// `OpenCode`: the exact Runtime generation, provider process birth, binding,
/// observer identity/status, loopback ownership, health, and root-session
/// status must corroborate the persisted handle.  Codex likewise requires a
/// live probe whose cwd and process birth exactly match its persisted Runtime.
///
/// # Errors
///
/// Returns a bounded action error when the Runtime is missing, ambiguous, or
/// fails any provider-specific ownership check.  A failed `OpenCode` check marks
/// only its exact observer handle `Unknown`; it never adopts a different pane
/// or session.
pub fn preflight_attachment(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<crate::state::RuntimeRecord, ActionError> {
    let mut runtime_record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(
            root.base(),
            runtime_record.runtime_id,
            &runtime_record.tmux_session,
        )?,
    );
    let probe = runtime.probe()?;
    if let Some(backfilled) = backfill_live_runtime_provider_pid(registry, &runtime_record, &probe)?
    {
        runtime_record = backfilled;
    }
    if !attachment_runtime_matches(&runtime_record, &probe) {
        if matches!(probe, RuntimeProbe::Missing)
            && runtime_record.status != crate::domain::RuntimeStatus::Stopped
            && runtime_record.process_birth.is_some()
        {
            registry.mark_runtime_recovery_required(
                runtime_record.runtime_id,
                runtime_record.revision,
            )?;
        }
        if runtime_record.provider == ProviderKind::OpenCode
            && let Some(handle) = registry.opencode_runtime_handle(runtime_record.runtime_id)?
        {
            crate::provider::opencode::mark_unknown_handle(
                registry,
                &handle,
                &runtime_record.tmux_generation,
            );
        }
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    if runtime_record.provider == ProviderKind::OpenCode {
        validate_opencode_live_runtime(registry, &runtime_record, &probe)?;
    }
    Ok(runtime_record)
}

pub(super) fn prepare_opencode_runtime(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    overview: &crate::state::WorkstreamOverview,
    prior_binding: Option<&crate::state::ProviderBinding>,
) -> Result<
    (
        crate::state::RuntimeRecord,
        OpenCodeEndpoint,
        ProviderSessionId,
    ),
    ActionError,
> {
    let record = registry.reserve_runtime_with_provider(workstream_id, ProviderKind::OpenCode)?;
    let session = if let Some(binding) = prior_binding {
        if binding.provider != ProviderKind::OpenCode {
            return Err(fail_unlaunched_runtime(
                registry,
                &record,
                ActionError::UnsupportedProvider(ProviderKind::OpenCode),
            ));
        }
        registry
            .bind_opencode_session(
                record.runtime_id,
                &record.tmux_generation,
                &binding.native_session_id,
                "resume",
            )
            .map_err(ActionError::State)
            .map_err(|error| fail_unlaunched_runtime(registry, &record, error))?;
        binding.native_session_id.clone()
    } else {
        let port = opencode::reserve_loopback_port()
            .map_err(ActionError::OpenCode)
            .map_err(|error| fail_unlaunched_runtime(registry, &record, error))?;
        let endpoint = OpenCodeEndpoint::loopback(port)
            .map_err(ActionError::OpenCode)
            .map_err(|error| fail_unlaunched_runtime(registry, &record, error))?;
        let prepared = registry
            .prepare_opencode_session_creation(record.runtime_id, &record.tmux_generation)
            .map_err(ActionError::State)
            .map_err(|error| fail_unlaunched_runtime(registry, &record, error))?;
        let mut started = None;
        let session_result = opencode::create_blank_session_with_before_create(
            "opencode",
            &overview.project_repository_path,
            endpoint,
            || {
                let operation = registry
                    .begin_opencode_session_creation(&prepared)
                    .map_err(ActionError::State)?;
                started = Some(operation);
                Ok::<(), ActionError>(())
            },
        );
        let session = match session_result {
            Ok(session) => session,
            Err(error) => {
                return Err(if let Some(started) = started.as_ref() {
                    fail_unknown_opencode_session_creation(registry, started)
                } else if matches!(
                    &error,
                    ActionError::OpenCode(OpenCodeError::ServeCleanupFailed)
                ) {
                    fail_cleanup_unknown_opencode_session_creation(registry, &prepared)
                } else {
                    fail_known_absent_opencode_session_creation(registry, &record, &prepared, error)
                });
            }
        };
        let Some(started) = started.as_ref() else {
            return Err(fail_unlaunched_runtime(
                registry,
                &record,
                ActionError::RuntimeProbeAmbiguous,
            ));
        };
        if registry
            .commit_opencode_session_creation(started, &session)
            .is_err()
        {
            return Err(fail_unknown_opencode_session_creation(registry, started));
        }
        session
    };
    let port = opencode::reserve_loopback_port()
        .map_err(ActionError::OpenCode)
        .map_err(|error| fail_unlaunched_runtime(registry, &record, error))?;
    let endpoint = OpenCodeEndpoint::loopback(port)
        .map_err(ActionError::OpenCode)
        .map_err(|error| fail_unlaunched_runtime(registry, &record, error))?;
    Ok((record, endpoint, session))
}
