use super::{
    ActionError, Duration, HostRegistry, PrivateRuntime, ProcessProbe, RuntimeProbe,
    terminate_owned_observer_process, terminate_owned_provider_process,
};

pub(super) const PARK_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);
pub(super) const PARK_CONFIRM_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(super) const PROVIDER_STOP_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn observer_identity_matches<P: ProcessProbe + ?Sized>(
    probe: &P,
    pid: u32,
    expected_birth: &str,
) -> bool {
    !expected_birth.is_empty() && probe.process_birth(pid).as_deref() == Some(expected_birth)
}

pub(super) fn spawned_observer_identity_matches<P: ProcessProbe + ?Sized>(
    handle: &crate::state::OpenCodeRuntimeHandle,
    pid: u32,
    birth: &str,
    probe: &P,
) -> bool {
    handle.observer_pid == Some(pid)
        && handle.observer_birth.as_deref() == Some(birth)
        && observer_identity_matches(probe, pid, birth)
}

pub(super) fn attachment_runtime_matches(
    record: &crate::state::RuntimeRecord,
    probe: &RuntimeProbe,
) -> bool {
    matches_recorded_runtime(record, probe, false)
}

pub(super) fn recorded_provider_identity(
    record: &crate::state::RuntimeRecord,
) -> Result<(u32, &str), ActionError> {
    let pid = record
        .provider_pid
        .ok_or(ActionError::RuntimeProbeAmbiguous)?;
    let birth = record
        .process_birth
        .as_deref()
        .filter(|birth| !birth.is_empty())
        .ok_or(ActionError::RuntimeProbeAmbiguous)?;
    Ok((pid, birth))
}

pub(super) fn stop_recorded_provider(
    record: &crate::state::RuntimeRecord,
) -> Result<(), ActionError> {
    let (pid, birth) = recorded_provider_identity(record)?;
    terminate_owned_provider_process(pid, birth, PROVIDER_STOP_TIMEOUT)?;
    Ok(())
}

pub(super) fn stop_recorded_provider_if_present(
    record: &crate::state::RuntimeRecord,
) -> Result<(), ActionError> {
    match (record.provider_pid, record.process_birth.as_deref()) {
        (Some(_), Some(birth)) if !birth.is_empty() => stop_recorded_provider(record),
        (None, None) => Ok(()),
        _ => Err(ActionError::RuntimeProbeAmbiguous),
    }
}

pub(super) fn clean_missing_stopped_runtime(
    runtime: &PrivateRuntime<'_>,
    record: &crate::state::RuntimeRecord,
) -> Result<(), ActionError> {
    if record.status != crate::domain::RuntimeStatus::Stopped {
        return Ok(());
    }
    match (record.provider_pid, record.process_birth.as_deref()) {
        (Some(_), Some(birth)) if !birth.is_empty() => {
            let provider_result = stop_recorded_provider(record);
            provider_result?;
            runtime.park().map_err(ActionError::Runtime)
        }
        (None, None) => runtime.park().map_err(ActionError::Runtime),
        _ => Err(ActionError::RuntimeProbeAmbiguous),
    }
}

pub(super) fn park_and_stop_provider(
    runtime: &PrivateRuntime<'_>,
    provider_pid: u32,
    process_birth: &str,
) -> Result<(), ActionError> {
    let provider_result =
        terminate_owned_provider_process(provider_pid, process_birth, PROVIDER_STOP_TIMEOUT);
    provider_result?;
    runtime.park()?;
    Ok(())
}

pub(super) fn park_and_stop_process_instance(
    runtime: &PrivateRuntime<'_>,
    process_pid: u32,
    process_birth: &str,
) -> Result<(), ActionError> {
    // This fallback is valid only before `release_launch`: the pane still runs
    // WSNav's silent barrier and cannot yet have provider-owned descendants.
    terminate_owned_observer_process(process_pid, process_birth, PROVIDER_STOP_TIMEOUT)?;
    runtime.park()?;
    Ok(())
}

pub(super) fn prefer_cleanup_error(
    cleanup: Result<(), ActionError>,
    original: ActionError,
) -> ActionError {
    cleanup.err().unwrap_or(original)
}

pub(super) fn fail_unidentified_runtime_launch(
    registry: &mut HostRegistry,
    runtime: &PrivateRuntime<'_>,
    record: &crate::state::RuntimeRecord,
    original: ActionError,
) -> ActionError {
    let cleanup = runtime.park().map_err(ActionError::Runtime);
    let recovery = registry
        .mark_runtime_recovery_required(record.runtime_id, record.revision)
        .map_err(ActionError::State);
    prefer_cleanup_error(cleanup.and(recovery), original)
}

pub(super) fn fail_runtime_cleanup(
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    original: ActionError,
) -> ActionError {
    registry
        .mark_runtime_recovery_required(record.runtime_id, record.revision)
        .map_err(ActionError::State)
        .err()
        .unwrap_or(original)
}

pub(super) fn fail_unlaunched_runtime(
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    original: ActionError,
) -> ActionError {
    registry
        .mark_runtime_stopped(record.runtime_id, record.revision)
        .map_err(ActionError::State)
        .err()
        .unwrap_or(original)
}

pub(super) fn fail_known_absent_opencode_session_creation(
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    prepared: &crate::state::OpenCodeSessionCreationOperation,
    original: ActionError,
) -> ActionError {
    let journal = registry
        .fail_opencode_session_creation(prepared, "provider_effect_not_started")
        .map(|_| ())
        .map_err(ActionError::State);
    let stopped = registry
        .mark_runtime_stopped(record.runtime_id, record.revision)
        .map_err(ActionError::State);
    journal.and(stopped).err().unwrap_or(original)
}

pub(super) fn fail_unknown_opencode_session_creation(
    registry: &mut HostRegistry,
    started: &crate::state::OpenCodeSessionCreationOperation,
) -> ActionError {
    match registry.mark_opencode_session_creation_unknown(started) {
        Ok(_) => ActionError::OpenCodeSessionCreationExternalEffectUnknown,
        Err(error) => ActionError::State(error),
    }
}

pub(super) fn fail_cleanup_unknown_opencode_session_creation(
    registry: &mut HostRegistry,
    prepared: &crate::state::OpenCodeSessionCreationOperation,
) -> ActionError {
    match registry.mark_opencode_session_creation_cleanup_unknown(prepared) {
        Ok(_) => ActionError::OpenCodeSessionCreationExternalEffectUnknown,
        Err(error) => ActionError::State(error),
    }
}

pub(super) fn matches_recorded_runtime(
    record: &crate::state::RuntimeRecord,
    probe: &RuntimeProbe,
    require_starting: bool,
) -> bool {
    (!require_starting || matches!(record.status, crate::domain::RuntimeStatus::Starting))
        && matches!(
            probe,
            RuntimeProbe::Live {
                pane_pid,
                cwd,
                process_birth: Some(process_birth),
                ..
            } if cwd == &record.cwd
                && record.provider_pid == Some(*pane_pid)
                && record.process_birth.as_deref() == Some(process_birth.as_str())
        )
}
