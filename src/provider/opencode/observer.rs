//! The disconnected `OpenCode` lifecycle observer.
//!
//! This module owns the long-lived HTTP/SSE supervision loop.  The CLI only
//! parses its hidden arguments and delegates one typed context here; no
//! provider payload or observer diagnostic is written to the native pane.

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    domain::{ProviderSessionId, RuntimeId, RuntimeStatus},
    runtime::{LinuxProcessProbe, ProcessProbe},
    state::{HostRegistry, OpenCodeLifecycleObservation, StateError, StateRoot},
};

use super::{
    LifecycleHint, OpenCodeClient, OpenCodeEndpoint, OpenCodeError, OpenCodeEvent,
    OpenCodeSessionStatus, endpoint_owned_by_process,
};

const HEALTH_DEADLINE: Duration = Duration::from_secs(15);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(500);
const STATUS_FAILURE_LIMIT: u8 = 4;
const RECONNECT_LIMIT: u8 = 4;

/// Typed hidden-command arguments after the CLI has parsed and validated IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeObserverContext {
    pub runtime_id: RuntimeId,
    pub generation: String,
    pub endpoint: OpenCodeEndpoint,
    pub session: ProviderSessionId,
    pub pane_pid: u32,
    pub cwd: PathBuf,
    pub provider_birth: String,
}

/// A bounded observer failure.  Ambiguous process/handle evidence is never
/// converted into a lifecycle mutation.
#[derive(Debug, Error)]
pub enum OpenCodeObserverError {
    #[error("private runtime probe is ambiguous")]
    RuntimeProbeAmbiguous,
    #[error(transparent)]
    OpenCode(#[from] OpenCodeError),
    #[error(transparent)]
    State(#[from] StateError),
}

/// Runs one exact `OpenCode` observer until its Runtime disappears or a
/// fail-closed ownership/identity check fails.
///
/// # Errors
///
/// Returns a bounded provider, state, or ownership error when corroboration
/// cannot be completed safely.
pub fn run_observer(
    root: &StateRoot,
    context: &OpenCodeObserverContext,
) -> Result<(), OpenCodeObserverError> {
    let mut registry = HostRegistry::open(root)?;
    let Some(record) = registry.runtime_by_id(context.runtime_id)? else {
        return Ok(());
    };
    if !observer_target_matches(&record, context) {
        return Ok(());
    }
    let pane_birth = record
        .process_birth
        .as_deref()
        .ok_or(OpenCodeObserverError::RuntimeProbeAmbiguous)?;
    if !endpoint_owned_by_process(&context.endpoint, context.pane_pid, pane_birth) {
        mark_unknown(&mut registry, context);
        return Err(OpenCodeObserverError::RuntimeProbeAmbiguous);
    }
    let Some(handle) = registry.opencode_runtime_handle(context.runtime_id)? else {
        return Ok(());
    };
    if handle.runtime_generation != context.generation
        || handle.endpoint_port != context.endpoint.port
        || handle.native_session_id != context.session
    {
        mark_unknown(&mut registry, context);
        return Ok(());
    }

    let client = OpenCodeClient::new(context.endpoint.clone());
    let deadline = Instant::now() + HEALTH_DEADLINE;
    loop {
        if client.health().is_ok()
            && matches!(
                client.session_status(&context.session),
                Ok(OpenCodeSessionStatus::Busy | OpenCodeSessionStatus::Idle)
            )
            && endpoint_owned_by_process(&context.endpoint, context.pane_pid, pane_birth)
        {
            let (observer_pid, observer_birth) = prepare(&mut registry, context)?;
            return supervise(
                &mut registry,
                context,
                &client,
                pane_birth,
                observer_pid,
                &observer_birth,
            );
        }
        if Instant::now() >= deadline {
            mark_unknown(&mut registry, context);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn observer_target_matches(
    record: &crate::state::RuntimeRecord,
    context: &OpenCodeObserverContext,
) -> bool {
    record.provider == crate::domain::ProviderKind::OpenCode
        && record.tmux_generation == context.generation
        && record.cwd == context.cwd
        && record.process_birth.as_deref() == Some(context.provider_birth.as_str())
}

fn prepare(
    registry: &mut HostRegistry,
    context: &OpenCodeObserverContext,
) -> Result<(u32, String), OpenCodeObserverError> {
    let observer_pid = std::process::id();
    let observer_birth = LinuxProcessProbe
        .process_birth(observer_pid)
        .ok_or(OpenCodeObserverError::RuntimeProbeAmbiguous)?;
    let current_handle = registry
        .opencode_runtime_handle(context.runtime_id)?
        .ok_or(OpenCodeObserverError::RuntimeProbeAmbiguous)?;
    registry.mark_opencode_observer_ready(
        context.runtime_id,
        &context.generation,
        current_handle.revision,
        observer_pid,
        &observer_birth,
    )?;
    if let Some(current) = registry.runtime_by_id(context.runtime_id)?
        && current.status == RuntimeStatus::Starting
    {
        let evidence = ObserverEvidence {
            context,
            observer_pid,
            observer_birth: &observer_birth,
        };
        if let Err(error) = apply_hint(registry, &evidence, LifecycleHint::Started) {
            mark_unknown(registry, context);
            return Err(error);
        }
    }
    Ok((observer_pid, observer_birth))
}

fn supervise(
    registry: &mut HostRegistry,
    context: &OpenCodeObserverContext,
    client: &OpenCodeClient,
    pane_birth: &str,
    observer_pid: u32,
    observer_birth: &str,
) -> Result<(), OpenCodeObserverError> {
    let mut reconnect_failures = 0_u8;
    let mut stream = None;
    let mut candidate_message_id = None;
    let mut last_status_poll = Instant::now();
    let mut status_failures = 0_u8;
    loop {
        let Some(current) = registry.runtime_by_id(context.runtime_id)? else {
            return Ok(());
        };
        if current.tmux_generation != context.generation
            || current.process_birth.as_deref() != Some(context.provider_birth.as_str())
            || client.health().is_err()
            || !endpoint_owned_by_process(client.endpoint(), context.pane_pid, pane_birth)
        {
            mark_unknown(registry, context);
            return Ok(());
        }

        // Polling is corroboration, never a source of a message ID. It also
        // keeps a Ready observer from surviving an endpoint that no longer
        // proves the exact root session identity.
        if last_status_poll.elapsed() >= STATUS_POLL_INTERVAL {
            let evidence = ObserverEvidence {
                context,
                observer_pid,
                observer_birth,
            };
            match poll_root_status(
                registry,
                &evidence,
                &mut candidate_message_id,
                client,
                &mut status_failures,
            ) {
                Ok(true) => {}
                Ok(false) => {
                    mark_unknown(registry, context);
                    return Ok(());
                }
                Err(error) => {
                    mark_unknown(registry, context);
                    return Err(error);
                }
            }
            last_status_poll = Instant::now();
        }

        if stream.is_none() {
            match client.event_stream() {
                Ok(new_stream) => stream = Some(new_stream),
                Err(_) => reconnect_failures = reconnect_failures.saturating_add(1),
            }
        }
        if let Some(event_stream) = stream.as_mut() {
            match event_stream.next_data() {
                Ok(Some(data)) => {
                    match super::parse_event_strict(&data, &context.session, Some(&context.cwd)) {
                        Ok(Some(event)) => {
                            let evidence = ObserverEvidence {
                                context,
                                observer_pid,
                                observer_birth,
                            };
                            if let Err(error) = apply_event(
                                registry,
                                &evidence,
                                &event,
                                &mut candidate_message_id,
                                client,
                            ) {
                                mark_unknown(registry, context);
                                return Err(error);
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            mark_unknown(registry, context);
                            return Ok(());
                        }
                    }
                    reconnect_failures = 0;
                }
                Err(OpenCodeError::IdleTimeout) => {}
                Ok(None) | Err(_) => {
                    stream = None;
                    reconnect_failures = reconnect_failures.saturating_add(1);
                }
            }
        }
        if reconnect_failures >= RECONNECT_LIMIT {
            mark_unknown(registry, context);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(
            100_u64.saturating_mul(1_u64 << reconnect_failures),
        ));
    }
}

fn poll_root_status(
    registry: &mut HostRegistry,
    evidence: &ObserverEvidence<'_>,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
    status_failures: &mut u8,
) -> Result<bool, OpenCodeObserverError> {
    let Ok(status) = client.session_status(&evidence.context.session) else {
        *status_failures = status_failures.saturating_add(1);
        return Ok(*status_failures < STATUS_FAILURE_LIMIT);
    };
    match status {
        OpenCodeSessionStatus::Busy => {
            *status_failures = 0;
            *candidate_message_id = None;
            let should_apply = registry
                .runtime_by_id(evidence.context.runtime_id)?
                .is_some_and(|runtime| runtime.status != RuntimeStatus::Working);
            if should_apply {
                apply_hint(registry, evidence, LifecycleHint::Working)?;
            }
        }
        OpenCodeSessionStatus::Idle => {
            *status_failures = 0;
            if candidate_message_id.is_some() {
                settle_if_idle(registry, evidence, candidate_message_id, client)
                    .map_err(OpenCodeObserverError::OpenCode)?;
            }
        }
        OpenCodeSessionStatus::Unknown => {
            *status_failures = status_failures.saturating_add(1);
        }
    }
    Ok(*status_failures < STATUS_FAILURE_LIMIT)
}

struct ObserverEvidence<'a> {
    context: &'a OpenCodeObserverContext,
    observer_pid: u32,
    observer_birth: &'a str,
}

fn apply_event(
    registry: &mut HostRegistry,
    evidence: &ObserverEvidence<'_>,
    event: &OpenCodeEvent,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
) -> Result<(), OpenCodeObserverError> {
    record_candidate(candidate_message_id, event);
    match &event.hint {
        Some(LifecycleHint::Working) => {
            *candidate_message_id = None;
            apply_hint(registry, evidence, LifecycleHint::Working)?;
        }
        Some(LifecycleHint::Settled { .. }) if candidate_message_id.is_some() => {
            settle_if_idle(registry, evidence, candidate_message_id, client)?;
        }
        Some(LifecycleHint::Started) => apply_hint(registry, evidence, LifecycleHint::Started)?,
        Some(LifecycleHint::Ended) => {
            apply_hint(registry, evidence, LifecycleHint::Ended)?;
        }
        Some(LifecycleHint::Settled { .. }) | None => {}
    }
    Ok(())
}

fn record_candidate(candidate_message_id: &mut Option<String>, event: &OpenCodeEvent) {
    if let Some(candidate) = event.candidate_message_id.clone() {
        *candidate_message_id = Some(candidate);
    }
    if matches!(&event.hint, Some(LifecycleHint::Working)) {
        *candidate_message_id = None;
    }
}

fn settle_if_idle(
    registry: &mut HostRegistry,
    evidence: &ObserverEvidence<'_>,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
) -> Result<(), OpenCodeError> {
    let status = client.session_status(&evidence.context.session)?;
    match status {
        OpenCodeSessionStatus::Idle => {
            let Some(message_id) = candidate_message_id.take() else {
                return Ok(());
            };
            apply_hint(
                registry,
                evidence,
                LifecycleHint::Settled {
                    message_id: Some(message_id),
                },
            )
            .map_err(|error| match error {
                OpenCodeObserverError::OpenCode(error) => error,
                OpenCodeObserverError::State(_) | OpenCodeObserverError::RuntimeProbeAmbiguous => {
                    OpenCodeError::MalformedResponse
                }
            })?;
        }
        OpenCodeSessionStatus::Busy => *candidate_message_id = None,
        OpenCodeSessionStatus::Unknown => {}
    }
    Ok(())
}

fn apply_hint(
    registry: &mut HostRegistry,
    evidence: &ObserverEvidence<'_>,
    hint: LifecycleHint,
) -> Result<(), OpenCodeObserverError> {
    let runtime_revision = registry
        .runtime_by_id(evidence.context.runtime_id)?
        .ok_or(StateError::UnknownRuntime(evidence.context.runtime_id))?
        .revision;
    let observation = OpenCodeLifecycleObservation {
        generation: evidence.context.generation.clone(),
        cwd: evidence.context.cwd.clone(),
        runtime_revision,
        session: evidence.context.session.clone(),
        observer_pid: evidence.observer_pid,
        observer_birth: evidence.observer_birth.to_owned(),
        hint,
    };
    registry
        .apply_opencode_lifecycle_observation(evidence.context.runtime_id, &observation)
        .map(|_| ())
        .map_err(OpenCodeObserverError::State)
}

fn mark_unknown(registry: &mut HostRegistry, context: &OpenCodeObserverContext) {
    if let Ok(Some(handle)) = registry.opencode_runtime_handle(context.runtime_id)
        && let (Some(observer_pid), Some(observer_birth)) =
            (handle.observer_pid, handle.observer_birth.as_deref())
    {
        let _ = registry.mark_opencode_observer_unknown_exact(
            context.runtime_id,
            &context.generation,
            handle.revision,
            observer_pid,
            observer_birth,
        );
    }
}

/// Marks an exact handle unknown for actions that cannot enter the observer.
pub fn mark_unknown_handle(
    registry: &mut HostRegistry,
    handle: &crate::state::OpenCodeRuntimeHandle,
    generation: &str,
) {
    if handle.runtime_generation != generation {
        return;
    }
    if let (Some(observer_pid), Some(observer_birth)) =
        (handle.observer_pid, handle.observer_birth.as_deref())
    {
        let _ = registry.mark_opencode_observer_unknown_exact(
            handle.runtime_id,
            generation,
            handle.revision,
            observer_pid,
            observer_birth,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_evidence_clears_a_completed_candidate_before_idle() {
        let mut candidate = Some("message-1".to_owned());
        record_candidate(
            &mut candidate,
            &OpenCodeEvent {
                hint: Some(LifecycleHint::Working),
                candidate_message_id: None,
            },
        );
        assert_eq!(candidate, None);
    }

    #[test]
    fn completed_candidate_is_retained_until_idle_evidence() {
        let mut candidate = None;
        record_candidate(
            &mut candidate,
            &OpenCodeEvent {
                hint: None,
                candidate_message_id: Some("message-1".to_owned()),
            },
        );
        assert_eq!(candidate.as_deref(), Some("message-1"));
    }
}
