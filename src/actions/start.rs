use super::{
    ActionError, Duration, HostRegistry, Instant, IntegrationLifecycle, LinuxProcessProbe,
    NativeLaunch, OpenCodeClient, OpenCodeEndpoint, OsString, Path, PathBuf, PrivateRuntime,
    ProcessProbe, ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimePaths, RuntimeProbe,
    StartOutcome, StateError, SystemTmux, WorkstreamId, WorkstreamLifecycle, codex_launch_program,
    codex_recovery_program, env, opencode, prove_owned_process_group, thread,
};
use super::{
    attachment::{PriorOpenCodeRuntime, inspect_opencode_prior_runtime, prepare_opencode_runtime},
    cleanup::{
        PARK_CONFIRM_POLL_INTERVAL, PARK_CONFIRM_TIMEOUT, clean_missing_stopped_runtime,
        fail_unidentified_runtime_launch, matches_recorded_runtime, park_and_stop_process_instance,
        park_and_stop_provider, prefer_cleanup_error, spawned_observer_identity_matches,
        stop_recorded_provider, stop_recorded_provider_if_present,
    },
    lifecycle::stop_opencode_observer,
    model::{
        active_workstream_overview, observer_profile, reconcile_observer_trust,
        require_codex_provider,
    },
    providers::managed_codex_environment,
};

/// Starts or resumes exactly one Workstream using the host's owned Codex
/// profile and private tmux Runtime.
///
/// # Errors
///
/// Returns an error when the expected Workstream revision is stale, observer
/// ownership/trust is incomplete, process evidence is ambiguous, or the
/// native launch cannot be reconciled safely.
pub fn start(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<StartOutcome, ActionError> {
    let overview = active_workstream_overview(registry, workstream_id)?;
    match overview.provider {
        ProviderKind::Codex => {
            start_codex(root, registry, workstream_id, expected_revision, &overview)
        }
        ProviderKind::OpenCode => {
            start_opencode(root, registry, workstream_id, expected_revision, &overview)
        }
    }
}

fn start_codex(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    overview: &crate::state::WorkstreamOverview,
) -> Result<StartOutcome, ActionError> {
    require_codex_provider(overview.provider)?;
    if expected_revision.is_some_and(|expected| expected != overview.revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    if overview.lifecycle == WorkstreamLifecycle::RecoveryRequired {
        return Err(ActionError::NativeRecoveryRequired);
    }
    reconcile_observer_trust(root, registry)?;
    let integration = registry
        .codex_integration()?
        .ok_or(ActionError::ObserverNotInstalled)?;
    if integration.lifecycle != IntegrationLifecycle::Ready {
        return Err(ActionError::ObserverNotReady);
    }
    let manager = observer_profile(root)?;
    manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    )?;
    manager.verify_native_trust(&integration.ownership)?;
    let mut prior_runtime = registry.runtime_for_workstream(workstream_id)?;
    if let Some(prior_record) = prior_runtime.as_mut() {
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let prior = PrivateRuntime::new(
            &tmux,
            &process_probe,
            RuntimePaths::for_record(
                root.base(),
                prior_record.runtime_id,
                &prior_record.tmux_session,
            )?,
        );
        let prior_probe = prior.probe()?;
        if let Some(backfilled) =
            backfill_live_runtime_provider_pid(registry, prior_record, &prior_probe)?
        {
            *prior_record = backfilled;
        }
        if matches_recorded_runtime(prior_record, &prior_probe, false) {
            return Ok(StartOutcome::AlreadyLive);
        }
        match prior_probe {
            RuntimeProbe::Missing => {
                clean_missing_stopped_runtime(&prior, prior_record)?;
                if prior_record.process_birth.is_some()
                    && !matches!(prior_record.status, crate::domain::RuntimeStatus::Stopped)
                {
                    registry.mark_runtime_recovery_required(
                        prior_record.runtime_id,
                        prior_record.revision,
                    )?;
                    return Err(ActionError::NativeRecoveryRequired);
                }
                if !matches!(prior_record.status, crate::domain::RuntimeStatus::Stopped) {
                    registry
                        .mark_runtime_stopped(prior_record.runtime_id, prior_record.revision)?;
                }
            }
            RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
                return Err(ActionError::RuntimeProbeAmbiguous);
            }
        }
    }
    let prior_binding = prior_runtime
        .as_ref()
        .map(|runtime| registry.retained_codex_binding_for_runtime(runtime.runtime_id))
        .transpose()?
        .flatten();
    let record = registry.reserve_runtime_with_provider(workstream_id, overview.provider)?;
    launch_reserved_runtime(
        root,
        registry,
        &record,
        codex_launch_program(&record.cwd, prior_binding.as_ref()),
    )?;
    Ok(StartOutcome::Started)
}

fn start_opencode(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    overview: &crate::state::WorkstreamOverview,
) -> Result<StartOutcome, ActionError> {
    if expected_revision.is_some_and(|expected| expected != overview.revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    if overview.lifecycle == WorkstreamLifecycle::RecoveryRequired {
        return Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        ));
    }
    crate::provider::require_new_eligible(registry, ProviderKind::OpenCode)
        .map_err(ActionError::ProviderReadiness)?;
    let prior_binding = match inspect_opencode_prior_runtime(root, registry, workstream_id)? {
        PriorOpenCodeRuntime::AlreadyLive => return Ok(StartOutcome::AlreadyLive),
        PriorOpenCodeRuntime::Ready(binding) => binding.map(|binding| *binding),
    };
    let (record, endpoint, session) =
        prepare_opencode_runtime(registry, workstream_id, overview, prior_binding.as_ref())?;
    if let Err(error) =
        launch_reserved_opencode_runtime(root, registry, &record, &endpoint, &session)
    {
        let cleanup = cleanup_failed_opencode_runtime(root, registry, &record);
        return Err(prefer_cleanup_error(cleanup, error));
    }
    Ok(StartOutcome::Started)
}

/// Starts a new private tmux generation only after a lost Runtime has been
/// made visible as recovery-required. A known native session resumes exactly;
/// an unbound Runtime opens Codex's native resume picker rather than creating
/// an unrelated blank thread.
///
/// The Workstream remains recovery-required until its verified
/// `SessionStart(source=resume)` hook arrives.
///
/// # Errors
///
/// Returns an error when the Workstream is not recovery-required, observer
/// trust is incomplete, the owned private runtime cannot be verified missing,
/// or its replacement process cannot be recorded safely.
pub fn recover(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<StartOutcome, ActionError> {
    let overview = active_workstream_overview(registry, workstream_id)?;
    match overview.provider {
        ProviderKind::Codex => {
            recover_codex(root, registry, workstream_id, expected_revision, &overview)
        }
        ProviderKind::OpenCode => {
            recover_opencode(root, registry, workstream_id, expected_revision, &overview)
        }
    }
}

fn recover_codex(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    overview: &crate::state::WorkstreamOverview,
) -> Result<StartOutcome, ActionError> {
    require_codex_provider(overview.provider)?;
    if expected_revision.is_some_and(|expected| expected != overview.revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    if overview.lifecycle != WorkstreamLifecycle::RecoveryRequired {
        return Err(ActionError::NativeRecoveryUnavailable);
    }
    reconcile_observer_trust(root, registry)?;
    let integration = registry
        .codex_integration()?
        .ok_or(ActionError::ObserverNotInstalled)?;
    if integration.lifecycle != IntegrationLifecycle::Ready {
        return Err(ActionError::ObserverNotReady);
    }
    let manager = observer_profile(root)?;
    manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    )?;
    manager.verify_native_trust(&integration.ownership)?;
    let prior_runtime = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
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
    if matches_recorded_runtime(&prior_runtime, &prior_probe, true) {
        return Ok(StartOutcome::AlreadyLive);
    }
    match prior_probe {
        // The path is derived solely from this persisted Runtime ID. Once its
        // exact server is conclusively gone, `park` uses that same private
        // socket and removes only the owned socket/config directory before a
        // new generation is allowed to claim it.
        RuntimeProbe::Missing => {
            let provider_result = stop_recorded_provider(&prior_runtime);
            provider_result?;
            prior.park().map_err(ActionError::Runtime)?;
        }
        RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    }
    let prior_binding = registry.retained_codex_binding_for_runtime(prior_runtime.runtime_id)?;
    let record =
        registry.reserve_runtime_recovery_with_provider(workstream_id, overview.provider)?;
    launch_reserved_runtime(
        root,
        registry,
        &record,
        codex_recovery_program(&record.cwd, prior_binding.as_ref()),
    )?;
    Ok(StartOutcome::Started)
}

fn recover_opencode(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    overview: &crate::state::WorkstreamOverview,
) -> Result<StartOutcome, ActionError> {
    if expected_revision.is_some_and(|expected| expected != overview.revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    if overview.lifecycle != WorkstreamLifecycle::RecoveryRequired {
        return Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        ));
    }
    crate::provider::require_new_eligible(registry, ProviderKind::OpenCode)
        .map_err(ActionError::ProviderReadiness)?;
    let prior_runtime = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    if prior_runtime.provider != ProviderKind::OpenCode
        || prior_runtime.status != crate::domain::RuntimeStatus::Unknown
    {
        return Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        ));
    }
    let binding = registry
        .binding_for_runtime(prior_runtime.runtime_id)?
        .ok_or(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        ))?;
    let handle = registry
        .opencode_runtime_handle(prior_runtime.runtime_id)?
        .ok_or(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        ))?;
    if !opencode_recovery_handle_matches(&prior_runtime, &binding, &handle) {
        return Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        ));
    }
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
    match prior.probe()? {
        RuntimeProbe::Missing => {}
        RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    }
    let observer_result = stop_opencode_observer(&handle);
    let provider_result = stop_recorded_provider(&prior_runtime);
    let runtime_result = if provider_result.is_ok() {
        prior.park().map_err(ActionError::Runtime)
    } else {
        Ok(())
    };
    observer_result.and(provider_result).and(runtime_result)?;

    launch_recovered_opencode_runtime(root, registry, workstream_id, &binding.native_session_id)?;
    Ok(StartOutcome::Started)
}

fn launch_recovered_opencode_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    session: &ProviderSessionId,
) -> Result<(), ActionError> {
    let record =
        registry.reserve_runtime_recovery_with_provider(workstream_id, ProviderKind::OpenCode)?;
    if let Err(error) = registry.bind_opencode_session(
        record.runtime_id,
        &record.tmux_generation,
        session,
        "resume",
    ) {
        let cleanup = cleanup_failed_opencode_runtime(root, registry, &record);
        return Err(prefer_cleanup_error(cleanup, ActionError::State(error)));
    }
    let port = match opencode::reserve_loopback_port() {
        Ok(port) => port,
        Err(error) => {
            let cleanup = cleanup_failed_opencode_runtime(root, registry, &record);
            return Err(prefer_cleanup_error(cleanup, ActionError::OpenCode(error)));
        }
    };
    let endpoint = match OpenCodeEndpoint::loopback(port) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let cleanup = cleanup_failed_opencode_runtime(root, registry, &record);
            return Err(prefer_cleanup_error(cleanup, ActionError::OpenCode(error)));
        }
    };
    if let Err(error) =
        launch_reserved_opencode_runtime(root, registry, &record, &endpoint, session)
    {
        let cleanup = cleanup_failed_opencode_runtime(root, registry, &record);
        return Err(prefer_cleanup_error(cleanup, error));
    }
    Ok(())
}

pub(super) fn opencode_recovery_handle_matches(
    runtime: &crate::state::RuntimeRecord,
    binding: &crate::state::ProviderBinding,
    handle: &crate::state::OpenCodeRuntimeHandle,
) -> bool {
    runtime.provider == ProviderKind::OpenCode
        && runtime.provider_pid.is_some()
        && runtime
            .process_birth
            .as_deref()
            .is_some_and(|birth| !birth.is_empty())
        && handle.runtime_id == runtime.runtime_id
        && handle.runtime_generation == runtime.tmux_generation
        && handle.endpoint_host == opencode::LOOPBACK_HOST
        && handle.endpoint_port != 0
        && !handle.version.is_empty()
        && binding.runtime_id == runtime.runtime_id
        && binding.runtime_generation == runtime.tmux_generation
        && binding.provider == ProviderKind::OpenCode
        && binding.native_session_id == handle.native_session_id
}

fn cleanup_failed_opencode_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
) -> Result<(), ActionError> {
    let observer_result = match registry.opencode_runtime_handle(record.runtime_id) {
        Ok(Some(handle)) => stop_opencode_observer(&handle),
        Ok(None) => Ok(()),
        Err(error) => Err(ActionError::State(error)),
    };
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let current_record = registry
        .runtime_by_id(record.runtime_id)
        .map(|current| current.filter(|current| current.tmux_generation == record.tmux_generation))
        .map_err(ActionError::State);
    let runtime_result = match (
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session),
        current_record,
    ) {
        (Ok(paths), Ok(current_record)) => {
            let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
            let provider_result = current_record
                .as_ref()
                .map_or(Ok(()), stop_recorded_provider_if_present);
            match provider_result {
                Ok(()) => runtime.park().map_err(ActionError::Runtime),
                Err(error) => Err(error),
            }
        }
        (Err(error), _) => Err(ActionError::Runtime(error)),
        (_, Err(error)) => Err(error),
    };
    let recovery_result = match registry.runtime_for_workstream(record.workstream_id) {
        Ok(Some(current))
            if current.runtime_id == record.runtime_id
                && current.status != crate::domain::RuntimeStatus::Unknown =>
        {
            registry
                .mark_runtime_recovery_required(current.runtime_id, current.revision)
                .map_err(ActionError::State)
        }
        Ok(_) => Ok(()),
        Err(error) => Err(ActionError::State(error)),
    };
    observer_result.and(runtime_result).and(recovery_result)
}

/// Reconciles only conclusive loss of an owned private Runtime before a
/// navigator snapshot is projected. An unavailable tmux socket, changed cwd,
/// or changed provider-process birth makes the Workstream recovery-required;
/// an ambiguous probe is deliberately left unchanged.
///
/// This is observation, not adoption: it never discovers or attaches an
/// external process and it preserves the existing provider binding verbatim.
///
/// # Errors
///
/// Returns an error only when a conclusive loss cannot be durably recorded.
/// Ambiguous or unavailable probes deliberately leave state unchanged.
pub fn reconcile_lost_runtimes(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
) -> Result<(), StateError> {
    let overviews = registry.workstream_overviews()?;
    for overview in overviews {
        if overview.lifecycle == WorkstreamLifecycle::RecoveryRequired {
            continue;
        }
        let Some(runtime_record) = overview.runtime else {
            continue;
        };
        if runtime_record.process_birth.is_none()
            || matches!(runtime_record.status, crate::domain::RuntimeStatus::Stopped)
        {
            continue;
        }
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let Ok(paths) = RuntimePaths::for_record(
            root.base(),
            runtime_record.runtime_id,
            &runtime_record.tmux_session,
        ) else {
            continue;
        };
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
        let conclusively_lost = match runtime.probe() {
            Ok(RuntimeProbe::Missing) => true,
            Ok(RuntimeProbe::Live {
                pane_pid,
                cwd,
                process_birth,
                ..
            }) => {
                if runtime_record.provider_pid.is_none()
                    && cwd == runtime_record.cwd
                    && process_birth.as_deref() == runtime_record.process_birth.as_deref()
                    && let Some(process_birth) = process_birth.as_deref()
                {
                    registry.backfill_runtime_provider_pid(
                        runtime_record.runtime_id,
                        runtime_record.revision,
                        pane_pid,
                        process_birth,
                    )?;
                    false
                } else {
                    cwd != runtime_record.cwd
                        || runtime_record.provider_pid != Some(pane_pid)
                        || process_birth.as_deref() != runtime_record.process_birth.as_deref()
                }
            }
            Ok(RuntimeProbe::Unknown { .. }) | Err(_) => false,
        };
        if conclusively_lost {
            registry.mark_runtime_recovery_required(
                runtime_record.runtime_id,
                runtime_record.revision,
            )?;
        }
    }
    Ok(())
}

fn launch_reserved_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    program: Vec<OsString>,
) -> Result<(), ActionError> {
    let paths = RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
    let program = runtime_launch_program(root.base(), record.runtime_id, program)?;
    let launch = NativeLaunch {
        cwd: record.cwd.clone(),
        program,
        environment: managed_codex_environment(),
    };
    if let Err(error) = runtime.start(&launch) {
        return Err(fail_unidentified_runtime_launch(
            registry,
            &runtime,
            record,
            ActionError::Runtime(error),
        ));
    }
    let probe = runtime.probe().map_err(|error| {
        fail_unidentified_runtime_launch(registry, &runtime, record, ActionError::Runtime(error))
    })?;
    let (pane_pid, process_birth) = match probe {
        RuntimeProbe::Live {
            pane_pid,
            cwd,
            process_birth: Some(process_birth),
            ..
        } if cwd == record.cwd => (pane_pid, process_birth),
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            return Err(fail_unidentified_runtime_launch(
                registry,
                &runtime,
                record,
                ActionError::RuntimeProbeAmbiguous,
            ));
        }
    };
    if let Err(error) =
        prove_owned_process_group(pane_pid, &process_birth, &process_probe, &process_probe)
    {
        let cleanup = park_and_stop_process_instance(&runtime, pane_pid, &process_birth);
        let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
        return Err(prefer_cleanup_error(cleanup, ActionError::Runtime(error)));
    }
    if let Err(error) = registry.record_runtime_process_identity(
        record.runtime_id,
        record.revision,
        pane_pid,
        &process_birth,
    ) {
        let cleanup = park_and_stop_provider(&runtime, pane_pid, &process_birth);
        let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
        return Err(prefer_cleanup_error(cleanup, ActionError::State(error)));
    }
    if let Err(error) = runtime.release_launch() {
        let cleanup = park_and_stop_provider(&runtime, pane_pid, &process_birth);
        if let Ok(Some(current)) = registry.runtime_for_workstream(record.workstream_id)
            && current.runtime_id == record.runtime_id
        {
            let _ = registry.mark_runtime_recovery_required(current.runtime_id, current.revision);
        }
        return Err(prefer_cleanup_error(cleanup, ActionError::Runtime(error)));
    }
    Ok(())
}

fn launch_reserved_opencode_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    endpoint: &OpenCodeEndpoint,
    session: &ProviderSessionId,
) -> Result<(), ActionError> {
    opencode::ensure_port_available(endpoint)?;
    let paths = RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
    let program = runtime_launch_program(
        root.base(),
        record.runtime_id,
        opencode::native_command("opencode", &record.cwd, endpoint, session),
    )?;
    let launch = NativeLaunch {
        cwd: record.cwd.clone(),
        program,
        environment: managed_codex_environment(),
    };
    if let Err(error) = runtime.start(&launch) {
        return Err(fail_unidentified_runtime_launch(
            registry,
            &runtime,
            record,
            ActionError::Runtime(error),
        ));
    }
    let probe = runtime.probe().map_err(|error| {
        fail_unidentified_runtime_launch(registry, &runtime, record, ActionError::Runtime(error))
    })?;
    let (pane_pid, process_birth) = match probe {
        RuntimeProbe::Live {
            pane_pid,
            cwd,
            process_birth: Some(process_birth),
            ..
        } if cwd == record.cwd => (pane_pid, process_birth),
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            return Err(fail_unidentified_runtime_launch(
                registry,
                &runtime,
                record,
                ActionError::RuntimeProbeAmbiguous,
            ));
        }
    };
    if let Err(error) =
        prove_owned_process_group(pane_pid, &process_birth, &process_probe, &process_probe)
    {
        let cleanup = park_and_stop_process_instance(&runtime, pane_pid, &process_birth);
        let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
        return Err(prefer_cleanup_error(cleanup, ActionError::Runtime(error)));
    }
    if let Err(error) = registry.record_runtime_process_identity(
        record.runtime_id,
        record.revision,
        pane_pid,
        &process_birth,
    ) {
        let cleanup = park_and_stop_provider(&runtime, pane_pid, &process_birth);
        return Err(prefer_cleanup_error(cleanup, ActionError::State(error)));
    }
    if let Err(error) = runtime.release_launch() {
        let cleanup = park_and_stop_provider(&runtime, pane_pid, &process_birth);
        return Err(prefer_cleanup_error(cleanup, ActionError::Runtime(error)));
    }
    let provider_version = match wait_for_opencode_provider(endpoint, pane_pid, &process_birth) {
        Ok(version) => version,
        Err(error) => {
            let cleanup = park_and_stop_provider(&runtime, pane_pid, &process_birth);
            return Err(prefer_cleanup_error(cleanup, error));
        }
    };
    let handle = match registry.record_opencode_runtime_handle(
        record.runtime_id,
        &record.tmux_generation,
        endpoint.port,
        &provider_version,
        session,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            let cleanup = park_and_stop_provider(&runtime, pane_pid, &process_birth);
            return Err(prefer_cleanup_error(cleanup, ActionError::State(error)));
        }
    };
    // The app-level observer is intentionally disconnected from the native
    // pane.  It owns only health/SSE corroboration and writes bounded status
    // to the private handle row.
    let observer = OpenCodeObserverLaunch {
        root: root.base().to_owned(),
        runtime_id: record.runtime_id,
        generation: record.tmux_generation.clone(),
        endpoint: endpoint.clone(),
        session: session.clone(),
        pane_pid,
        cwd: record.cwd.clone(),
        process_birth,
        handle_revision: handle.revision,
    };
    spawn_opencode_observer(registry, &observer)?;
    Ok(())
}

fn wait_for_opencode_provider(
    endpoint: &OpenCodeEndpoint,
    pane_pid: u32,
    process_birth: &str,
) -> Result<String, ActionError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let client = OpenCodeClient::new(endpoint.clone());
    loop {
        if opencode::endpoint_owned_by_process(endpoint, pane_pid, process_birth)
            && let Ok(health) = client.health()
        {
            return Ok(health.version);
        }
        if Instant::now() >= deadline {
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

struct OpenCodeObserverLaunch {
    root: PathBuf,
    runtime_id: RuntimeId,
    generation: String,
    endpoint: OpenCodeEndpoint,
    session: ProviderSessionId,
    pane_pid: u32,
    cwd: PathBuf,
    process_birth: String,
    handle_revision: Revision,
}

fn spawn_opencode_observer(
    registry: &mut HostRegistry,
    observer: &OpenCodeObserverLaunch,
) -> Result<(), ActionError> {
    let executable = env::current_exe().map_err(ActionError::Io)?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("--state-root")
        .arg(&observer.root)
        .arg("_opencode_observer")
        .arg(observer.runtime_id.to_string())
        .arg(&observer.generation)
        .arg(observer.endpoint.port.to_string())
        .arg(observer.session.native_id())
        .arg(observer.pane_pid.to_string())
        .arg(observer.cwd.as_os_str())
        .arg(&observer.process_birth)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(ActionError::Io)?;
    let observer_pid = child.id();
    let Some(observer_birth) = LinuxProcessProbe.process_birth(observer_pid) else {
        terminate_spawned_observer(&mut child);
        return Err(ActionError::RuntimeProbeAmbiguous);
    };
    let starting = match registry.record_opencode_observer_started(
        observer.runtime_id,
        &observer.generation,
        observer.handle_revision,
        observer_pid,
        &observer_birth,
    ) {
        Ok(starting) => starting,
        Err(error) => {
            terminate_spawned_observer(&mut child);
            return Err(ActionError::State(error));
        }
    };
    wait_for_spawned_observer_ready(
        registry,
        observer,
        &mut child,
        observer_pid,
        &observer_birth,
        &starting,
    )
}

pub(super) fn backfill_live_runtime_provider_pid(
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    probe: &RuntimeProbe,
) -> Result<Option<crate::state::RuntimeRecord>, ActionError> {
    let RuntimeProbe::Live {
        pane_pid,
        cwd,
        process_birth: Some(process_birth),
        ..
    } = probe
    else {
        return Ok(None);
    };
    if record.provider_pid.is_some()
        || cwd != &record.cwd
        || record.process_birth.as_deref() != Some(process_birth.as_str())
    {
        return Ok(None);
    }
    registry.backfill_runtime_provider_pid(
        record.runtime_id,
        record.revision,
        *pane_pid,
        process_birth,
    )?;
    registry
        .runtime_by_id(record.runtime_id)?
        .ok_or(ActionError::RuntimeProbeAmbiguous)
        .map(Some)
}

fn wait_for_spawned_observer_ready(
    registry: &mut HostRegistry,
    observer: &OpenCodeObserverLaunch,
    child: &mut std::process::Child,
    observer_pid: u32,
    observer_birth: &str,
    starting: &crate::state::OpenCodeRuntimeHandle,
) -> Result<(), ActionError> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let child_status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                terminate_spawned_observer(child);
                return Err(ActionError::Io(error));
            }
        };
        if let Some(_status) = child_status {
            mark_spawned_observer_unknown(
                registry,
                observer,
                observer_pid,
                observer_birth,
                starting.revision,
            );
            terminate_spawned_observer(child);
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
        let current = match registry.opencode_runtime_handle(observer.runtime_id) {
            Ok(Some(current)) => current,
            Ok(None) => {
                terminate_spawned_observer(child);
                return Err(ActionError::RuntimeProbeAmbiguous);
            }
            Err(error) => {
                terminate_spawned_observer(child);
                return Err(ActionError::State(error));
            }
        };
        let exact_child = spawned_observer_identity_matches(
            &current,
            observer_pid,
            observer_birth,
            &LinuxProcessProbe,
        );
        if current.runtime_generation != observer.generation
            || current.revision < starting.revision
            || !exact_child
        {
            let _ = registry.mark_opencode_observer_unknown_exact(
                observer.runtime_id,
                &observer.generation,
                current.revision,
                observer_pid,
                observer_birth,
            );
            terminate_spawned_observer(child);
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
        if current.observer_status == crate::state::OpenCodeObserverStatus::Ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = registry.mark_opencode_observer_unknown_exact(
                observer.runtime_id,
                &observer.generation,
                current.revision,
                observer_pid,
                observer_birth,
            );
            terminate_spawned_observer(child);
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn mark_spawned_observer_unknown(
    registry: &mut HostRegistry,
    observer: &OpenCodeObserverLaunch,
    observer_pid: u32,
    observer_birth: &str,
    fallback_revision: Revision,
) {
    let revision = registry
        .opencode_runtime_handle(observer.runtime_id)
        .ok()
        .flatten()
        .filter(|handle| {
            handle.runtime_generation == observer.generation
                && handle.observer_pid == Some(observer_pid)
                && handle.observer_birth.as_deref() == Some(observer_birth)
        })
        .map_or(fallback_revision, |handle| handle.revision);
    let _ = registry.mark_opencode_observer_unknown_exact(
        observer.runtime_id,
        &observer.generation,
        revision,
        observer_pid,
        observer_birth,
    );
}

fn terminate_spawned_observer(child: &mut std::process::Child) {
    let deadline = Instant::now() + PARK_CONFIRM_TIMEOUT;
    let still_live = !matches!(child.try_wait(), Ok(Some(_)));
    if still_live {
        #[cfg(unix)]
        {
            use nix::{sys::signal, unistd::Pid};
            if let Ok(pid) = i32::try_from(child.id()) {
                let _ = signal::kill(Pid::from_raw(pid), signal::Signal::SIGTERM);
            }
        }
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(PARK_CONFIRM_POLL_INTERVAL);
        }
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
        }
    }
    let _ = child.wait();
}

pub(super) fn runtime_launch_program(
    state_root: &Path,
    runtime_id: RuntimeId,
    program: Vec<OsString>,
) -> Result<Vec<OsString>, ActionError> {
    let executable = env::current_exe().map_err(ActionError::Io)?;
    let mut wrapped = vec![
        executable.into_os_string(),
        "--state-root".into(),
        state_root.as_os_str().to_owned(),
        "_runtime_launch".into(),
        runtime_id.to_string().into(),
        "--".into(),
    ];
    wrapped.extend(program);
    Ok(wrapped)
}
