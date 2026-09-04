use super::{
    ActionError, CatalogAuthorization, HostRegistry, Instant, LinuxProcessProbe, PrivateRuntime,
    ProviderKind, Revision, RuntimeId, RuntimePaths, RuntimeProbe, StateError, SystemClock,
    SystemTmux, WorkstreamId, WorkstreamLifecycle, terminate_owned_observer_process, thread,
};
use super::{
    cleanup::{
        PARK_CONFIRM_POLL_INTERVAL, PARK_CONFIRM_TIMEOUT, PROVIDER_STOP_TIMEOUT,
        fail_runtime_cleanup, matches_recorded_runtime, recorded_provider_identity,
        stop_recorded_provider_if_present,
    },
    model::{ensure_workstream_revision, workstream_overview_authorized, workstream_revision},
};
use crate::domain::Clock;

/// Stops one exact live Runtime while preserving provider history and project
/// files. This is an internal cleanup primitive used by Archive and recovery;
/// it is not a user-facing action.
///
/// # Errors
///
/// Returns an error when the expected Workstream revision is stale, the
/// runtime cannot be stopped, or durable state cannot record the exact effect.
pub(crate) fn park(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<Revision, ActionError> {
    park_authorized(
        root,
        registry,
        workstream_id,
        expected_revision,
        CatalogAuthorization::ActiveOnly,
    )
}

/// Stops one exact Runtime under an explicit catalog authorization. Archived
/// Forget and native-exit reconciliation need the secondary scope; the
/// active-only `park` wrapper preserves the ordinary public boundary.
pub(crate) fn park_authorized(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    authorization: CatalogAuthorization,
) -> Result<Revision, ActionError> {
    let overview = workstream_overview_authorized(registry, workstream_id, authorization)?;
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
    let probe = runtime.probe()?;
    let clean_provider_exit = clean_provider_exit_status(&runtime, &record, &probe)? == Some(0);
    match probe {
        probe @ RuntimeProbe::Live { .. } if matches_recorded_runtime(&record, &probe, false) => {}
        RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } if clean_provider_exit => {}
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

/// Reconciles the return from one native tmux attachment. `false` means the
/// exact provider remains live and the client merely detached. `true` means
/// the exact provider pane exited normally and its Runtime was parked.
///
/// # Errors
///
/// Returns an error when Runtime identity, generation, topology, process
/// birth, exit status, or the final revision-fenced stop cannot be proven.
pub(crate) fn reconcile_provider_attachment_end(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_runtime_id: RuntimeId,
    expected_runtime_generation: &str,
) -> Result<bool, ActionError> {
    let overview = workstream_overview_authorized(
        registry,
        workstream_id,
        CatalogAuthorization::ArchivedAllowed,
    )?;
    let record = overview
        .runtime
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    if record.runtime_id != expected_runtime_id
        || record.tmux_generation != expected_runtime_generation
    {
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    let probe = runtime.probe()?;
    if matches_recorded_runtime(&record, &probe, false) {
        return Ok(false);
    }
    if clean_provider_exit_status(&runtime, &record, &probe)? != Some(0) {
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    park_authorized(
        root,
        registry,
        workstream_id,
        Some(overview.revision),
        CatalogAuthorization::ArchivedAllowed,
    )?;
    Ok(true)
}

pub(super) fn clean_provider_exit_status(
    runtime: &PrivateRuntime<'_>,
    record: &crate::state::RuntimeRecord,
    probe: &RuntimeProbe,
) -> Result<Option<i32>, ActionError> {
    let pane_pid = match probe {
        RuntimeProbe::Live {
            process_birth: Some(_),
            ..
        }
        | RuntimeProbe::Missing => return Ok(None),
        RuntimeProbe::Live {
            pane_pid,
            process_birth: None,
            ..
        } => Some(*pane_pid),
        // A retained dead pane can make ordinary live-pane fields unavailable
        // on some tmux versions. The independent dead-topology proof below is
        // the authority for this otherwise unknown observation.
        RuntimeProbe::Unknown { .. } => None,
    };
    let (recorded_pid, _) = recorded_provider_identity(record)?;
    if pane_pid.is_some_and(|pane_pid| pane_pid != recorded_pid) {
        return Ok(None);
    }
    Ok(runtime.provider_exit_status(recorded_pid, &record.cwd).ok())
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
/// Runtime is stopped first so the provider is never left running behind a
/// hidden row. If exact cleanup commits but the archive transition cannot, the
/// Workstream remains visible with its stopped Runtime and can be retried with
/// fresh revision evidence.
///
/// # Errors
///
/// Returns an error when the Workstream revision is stale, a required Runtime
/// exact cleanup fails, the Workstream is already archived, or durable state cannot
/// commit the exact archive transition.
pub fn archive(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
) -> Result<Revision, ActionError> {
    let overview = workstream_overview_authorized(
        registry,
        workstream_id,
        CatalogAuthorization::ArchivedAllowed,
    )?;
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
/// starting, resuming, or stopping its provider Runtime.
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

/// Permanently forgets one archived Workstream from `WSNav`'s catalog. Provider
/// history, Project/Location/Git state, and unrelated Workstreams remain
/// outside this action's ownership boundary.
///
/// # Errors
///
/// Returns an error when the Workstream is not archived, its revision is stale,
/// its exact Runtime cannot be stopped, its operation effects are ambiguous or
/// unresolved, or the owned graph cannot be removed transactionally.
pub fn forget(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
) -> Result<(), ActionError> {
    let overview = workstream_overview_authorized(
        registry,
        workstream_id,
        CatalogAuthorization::ArchivedAllowed,
    )?;
    if overview.archived_at_millis.is_none() {
        return Err(ActionError::WorkstreamNotArchived);
    }

    // Refuse before any external stop if the retained graph has an unresolved
    // or shared provider-effect operation. The final state transaction repeats
    // this fence after exact cleanup in case another participant raced us.
    if overview.revision != expected_revision {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    registry
        .validate_forget_workstream(workstream_id, expected_revision)
        .map_err(ActionError::State)?;

    let revision = if overview.runtime.is_some() {
        park_authorized(
            root,
            registry,
            workstream_id,
            Some(expected_revision),
            CatalogAuthorization::ArchivedAllowed,
        )?
    } else {
        expected_revision
    };
    registry
        .forget_workstream(workstream_id, revision)
        .map_err(ActionError::State)
}

/// Waits briefly for the durable outcome of a concurrently requested exact
/// Runtime stop.
///
/// The exact stop first terminates the private tmux server, which makes an
/// already attached native client exit before the stop action can commit its
/// `SQLite` transaction. Treat that exit as clean only after the exact Runtime
/// and Workstream record the deliberate stopped outcome. A crash, stale
/// Runtime, or replacement generation never satisfies this predicate.
///
/// # Errors
///
/// Returns an error when the registry cannot be opened or queried.
pub(crate) fn await_deliberate_park(
    root: &crate::state::StateRoot,
    runtime_id: RuntimeId,
    workstream_id: WorkstreamId,
) -> Result<bool, StateError> {
    let deadline = Instant::now() + PARK_CONFIRM_TIMEOUT;
    loop {
        let registry = crate::state::open_current(root)?.into_host_registry()?;
        if registry.runtime_is_deliberately_parked(runtime_id, workstream_id)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(PARK_CONFIRM_POLL_INTERVAL);
    }
}
