use super::{
    ActionError, EphemeralAppServer, HostRegistry, Instant, LinuxProcessProbe, PrivateRuntime,
    ProviderKind, Revision, RuntimeId, RuntimePaths, RuntimeProbe, StateError, SystemClock,
    SystemTmux, WorkstreamId, WorkstreamLifecycle, terminate_owned_observer_process, thread,
};
use super::{
    cleanup::{
        PARK_CONFIRM_POLL_INTERVAL, PARK_CONFIRM_TIMEOUT, PROVIDER_STOP_TIMEOUT,
        fail_runtime_cleanup, matches_recorded_runtime, recorded_provider_identity,
        stop_recorded_provider_if_present,
    },
    model::{
        active_workstream_overview, ensure_workstream_revision, require_codex_provider,
        workstream_overview, workstream_revision,
    },
};
use crate::domain::Clock;

/// Parks one live Runtime while preserving provider history and project files.
///
/// # Errors
///
/// Returns an error when the expected Workstream revision is stale, the
/// runtime cannot be parked, or durable state cannot record the exact effect.
pub fn park(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<Revision, ActionError> {
    let overview = active_workstream_overview(registry, workstream_id)?;
    if expected_revision.is_some_and(|expected| expected != overview.revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    let mut record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    match runtime.probe()? {
        probe @ RuntimeProbe::Live { .. } if matches_recorded_runtime(&record, &probe, false) => {}
        RuntimeProbe::Live {
            pane_pid,
            cwd,
            process_birth: Some(process_birth),
            ..
        } if record.provider_pid.is_none()
            && cwd == record.cwd
            && record.process_birth.as_deref() == Some(process_birth.as_str()) =>
        {
            registry.backfill_runtime_provider_pid(
                record.runtime_id,
                record.revision,
                pane_pid,
                &process_birth,
            )?;
            record = registry
                .runtime_by_id(record.runtime_id)?
                .ok_or(ActionError::RuntimeProbeAmbiguous)?;
        }
        RuntimeProbe::Missing
            if recorded_provider_identity(&record).is_ok()
                || (record.provider_pid.is_none() && record.process_birth.is_none()) => {}
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    }
    let opencode_handle = match record.provider {
        ProviderKind::Codex => None,
        ProviderKind::OpenCode => registry.opencode_runtime_handle(record.runtime_id)?,
    };
    let mut cleanup_error = opencode_handle
        .as_ref()
        .and_then(|handle| stop_opencode_observer(handle).err());
    match stop_recorded_provider_if_present(&record) {
        Ok(()) => {
            if let Err(error) = runtime.park() {
                cleanup_error.get_or_insert(ActionError::Runtime(error));
            }
        }
        Err(error) => {
            cleanup_error.get_or_insert(error);
        }
    }
    if let Some(error) = cleanup_error {
        return Err(fail_runtime_cleanup(registry, &record, error));
    }
    if opencode_handle.is_some()
        && let Err(error) = registry
            .delete_opencode_runtime_handle(record.runtime_id, &record.tmux_generation)
            .map_err(ActionError::State)
    {
        return Err(fail_runtime_cleanup(registry, &record, error));
    }
    if let Err(error) = registry
        .park_runtime(record.runtime_id, record.revision)
        .map_err(ActionError::State)
    {
        return Err(fail_runtime_cleanup(registry, &record, error));
    }
    workstream_revision(registry, workstream_id)
}

pub(super) fn stop_opencode_observer(
    handle: &crate::state::OpenCodeRuntimeHandle,
) -> Result<(), ActionError> {
    let Some(pid) = handle.observer_pid else {
        return Ok(());
    };
    let Some(expected_birth) = handle.observer_birth.as_deref() else {
        return Err(ActionError::RuntimeProbeAmbiguous);
    };
    if expected_birth.is_empty() {
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    terminate_owned_observer_process(pid, expected_birth, PROVIDER_STOP_TIMEOUT)?;
    Ok(())
}

/// Archives a Workstream as a reversible navigator-visibility change. A live
/// Runtime is parked first so the provider is never left running behind a
/// hidden row. If parking commits but the archive transition cannot, the
/// Workstream remains visibly parked and can be retried with fresh revision
/// evidence.
///
/// # Errors
///
/// Returns an error when the Workstream revision is stale, a required Runtime
/// park fails, the Workstream is already archived, or durable state cannot
/// commit the exact archive transition.
pub fn archive(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
) -> Result<Revision, ActionError> {
    let overview = workstream_overview(registry, workstream_id)?;
    if overview.revision != expected_revision {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    if overview.archived_at_millis.is_some() {
        return Err(ActionError::WorkstreamAlreadyArchived);
    }
    let archive_revision =
        if overview.lifecycle != WorkstreamLifecycle::Parked && overview.runtime.is_some() {
            park(root, registry, workstream_id, Some(expected_revision))?
        } else {
            expected_revision
        };
    let archived_at_millis = SystemClock.now_millis().map_err(StateError::from)?;
    registry
        .archive_workstream(workstream_id, archive_revision, archived_at_millis)
        .map_err(Into::into)
}

/// Restores an archived Workstream to the active navigator scope without
/// starting or resuming Codex.
///
/// # Errors
///
/// Returns an error when the Workstream revision is stale, it is not archived,
/// or durable state cannot commit the exact restore transition.
pub fn restore(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
) -> Result<Revision, ActionError> {
    ensure_workstream_revision(registry, workstream_id, Some(expected_revision))?;
    registry
        .restore_workstream(workstream_id, expected_revision)
        .map_err(Into::into)
}

/// Renames the exact current Codex conversation through Codex's canonical
/// name field, then refreshes only `WSNav`'s bounded name cache.
///
/// # Errors
///
/// Returns an error when the Workstream is archived, stale, unbound, or the
/// provider rejects the bounded canonical name change.
pub fn rename(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
    name: &str,
) -> Result<(), ActionError> {
    let overview = active_workstream_overview(registry, workstream_id)?;
    require_codex_provider(overview.provider)?;
    if overview.revision != expected_revision {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    let runtime = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    let binding = registry
        .binding_for_runtime(runtime.runtime_id)?
        .ok_or(ActionError::NoProviderBinding(workstream_id))?;
    EphemeralAppServer::default().set_thread_name(binding.native_session_id.native_id(), name)?;
    registry.record_thread_name(runtime.runtime_id, &binding.native_session_id, name)?;
    Ok(())
}

/// Waits briefly for the durable outcome of a concurrently requested park.
///
/// Parking first stops the private tmux server, which makes an already
/// attached native client exit before the park action can commit its `SQLite`
/// transaction. Treat that exit as clean only after the exact Runtime and
/// Workstream record the deliberate parked outcome. A crash, stale Runtime,
/// or replacement generation never satisfies this predicate.
///
/// # Errors
///
/// Returns an error when the registry cannot be opened or queried.
pub fn await_deliberate_park(
    root: &crate::state::StateRoot,
    runtime_id: RuntimeId,
    workstream_id: WorkstreamId,
) -> Result<bool, StateError> {
    let deadline = Instant::now() + PARK_CONFIRM_TIMEOUT;
    loop {
        let registry = HostRegistry::open(root)?;
        if registry.runtime_is_deliberately_parked(runtime_id, workstream_id)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(PARK_CONFIRM_POLL_INTERVAL);
    }
}
