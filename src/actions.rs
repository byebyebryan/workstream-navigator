//! Host-local lifecycle actions shared by direct CLI and remote protocol paths.
//!
//! These actions own native process effects. The CLI and SSH protocol only
//! parse intent and render outcomes; neither gets to reimplement launch or
//! private-tmux authority.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    domain::{
        Clock, OperationId, OperationKind, OperationPhase, ProviderKind, ProviderSessionId,
        Revision, RuntimeId, SystemClock, WorkstreamId, WorkstreamLifecycle,
    },
    provider::codex::app_server::{AppServerError, EphemeralAppServer, ForkReconciliation},
    provider::codex::profile::{ObserverProfile, ProfileError},
    provider::opencode::{
        self, OpenCodeClient, OpenCodeEndpoint, OpenCodeError, endpoint_owned_by_process,
    },
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, ProcessProbe, RuntimePaths, RuntimeProbe,
        SystemTmux,
    },
    state::{HostRegistry, IntegrationLifecycle, ProviderBinding, StateError},
};

#[cfg(test)]
use crate::provider::names::NameState;

const PARK_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);
const PARK_CONFIRM_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn observer_identity_matches<P: ProcessProbe + ?Sized>(
    probe: &P,
    pid: u32,
    expected_birth: &str,
) -> bool {
    !expected_birth.is_empty() && probe.process_birth(pid).as_deref() == Some(expected_birth)
}

fn spawned_observer_identity_matches<P: ProcessProbe + ?Sized>(
    handle: &crate::state::OpenCodeRuntimeHandle,
    pid: u32,
    birth: &str,
    probe: &P,
) -> bool {
    handle.observer_pid == Some(pid)
        && handle.observer_birth.as_deref() == Some(birth)
        && observer_identity_matches(probe, pid, birth)
}

fn attachment_runtime_matches(record: &crate::state::RuntimeRecord, probe: &RuntimeProbe) -> bool {
    matches_recorded_runtime(record, probe, false)
}

/// The durable outcome of a start-or-resume request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    Started,
    AlreadyLive,
}

#[derive(Clone, Copy)]
struct IndependentStartSpec<'a> {
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: &'a str,
    provider: ProviderKind,
}

fn start_independent_workstream_with<R, S>(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    spec: IndependentStartSpec<'_>,
    readiness: R,
    starter: S,
) -> Result<WorkstreamId, ActionError>
where
    R: FnOnce(&HostRegistry, ProviderKind) -> Result<(), ActionError>,
    S: FnOnce(
        &crate::state::StateRoot,
        &mut HostRegistry,
        WorkstreamId,
        Option<Revision>,
        ProviderKind,
    ) -> Result<StartOutcome, ActionError>,
{
    let source = workstream_overview(registry, spec.source_workstream_id)?;
    if spec
        .expected_revision
        .is_some_and(|expected| expected != source.revision)
    {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    readiness(registry, spec.provider)?;
    let created = registry.create_independent_workstream(
        spec.request_key,
        spec.source_workstream_id,
        source.revision,
        spec.provider,
    )?;
    let _ = starter(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
        spec.provider,
    )?;
    Ok(created.workstream_id)
}

/// Creates an independent Workstream at a registered project's root, then
/// starts its first native Codex Runtime. The retained source may be archived:
/// archive changes navigator visibility only and does not revoke its project.
///
/// The source selects a `ProjectLocation` and expected revision only. This
/// action never invokes Git or copies files; Codex owns any worktree workflow
/// it chooses to perform after the native session starts.
///
/// # Errors
///
/// Returns an error when the source revision is stale or observer setup
/// prevents the native start.
pub fn start_independent_workstream(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: &str,
    provider: ProviderKind,
) -> Result<WorkstreamId, ActionError> {
    start_independent_workstream_with(
        root,
        registry,
        IndependentStartSpec {
            source_workstream_id,
            expected_revision,
            request_key,
            provider,
        },
        |registry, provider| {
            crate::provider::require_new_eligible(registry, provider)
                .map_err(ActionError::ProviderReadiness)
        },
        |root, registry, workstream_id, expected_revision, _provider| {
            start(root, registry, workstream_id, expected_revision)
        },
    )
}

/// Provider-readiness seam for the remote control path. Production callers use
/// this wrapper to perform the live capability re-probe; deterministic tests
/// use the adjacent injected-starter variant so they exercise the exact same
/// revision/request-key/provider path without launching a real provider.
pub(crate) fn start_independent_workstream_with_readiness<F>(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: &str,
    provider: ProviderKind,
    readiness: F,
) -> Result<WorkstreamId, ActionError>
where
    F: FnOnce(&HostRegistry, ProviderKind) -> Result<(), crate::provider::ProviderReadinessError>,
{
    start_independent_workstream_with_readiness_and_starter(
        root,
        registry,
        source_workstream_id,
        expected_revision,
        request_key,
        provider,
        readiness,
        |root, registry, workstream_id, expected_revision, _provider| {
            start(root, registry, workstream_id, expected_revision)
        },
    )
}

/// Provider-readiness seam with an injected native starter for bounded
/// protocol tests. Production callers should use
/// [`start_independent_workstream_with_readiness`], which supplies the real
/// provider-scoped start action.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_independent_workstream_with_readiness_and_starter<F, S>(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: &str,
    provider: ProviderKind,
    readiness: F,
    starter: S,
) -> Result<WorkstreamId, ActionError>
where
    F: FnOnce(&HostRegistry, ProviderKind) -> Result<(), crate::provider::ProviderReadinessError>,
    S: FnOnce(
        &crate::state::StateRoot,
        &mut HostRegistry,
        WorkstreamId,
        Option<Revision>,
        ProviderKind,
    ) -> Result<StartOutcome, ActionError>,
{
    start_independent_workstream_with(
        root,
        registry,
        IndependentStartSpec {
            source_workstream_id,
            expected_revision,
            request_key,
            provider,
        },
        |registry, provider| readiness(registry, provider).map_err(ActionError::ProviderReadiness),
        starter,
    )
}

/// Forks an active Codex Workstream at its last completed turn without
/// interrupting or waiting for the source's current turn. The destination
/// starts at the same registered project root; this action never creates or
/// validates a Git worktree. The provider fork is recorded before it is sent
/// and is never retried after an ambiguous result.
///
/// # Errors
///
/// Returns an error when the selected source lacks a live settled boundary,
/// provider evidence is not exact, observer setup prevents the destination
/// launch, or recovery is required instead of a retry.
pub fn fork_workstream(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
) -> Result<WorkstreamId, ActionError> {
    let source = active_workstream_overview(registry, source_workstream_id)?;
    require_codex_provider(source.provider)?;
    if expected_revision.is_some_and(|expected| expected != source.revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    let prepared = registry.prepare_fork_with_provider(
        request_key,
        OperationKind::Fork,
        source_workstream_id,
        source.revision,
        source.provider,
    )?;
    if prepared.plan.operation.phase == OperationPhase::Committed {
        let _ = start(root, registry, prepared.plan.workstream_id, None)?;
        return Ok(prepared.plan.workstream_id);
    }
    if prepared.plan.operation.phase == OperationPhase::RecoveryRequired {
        return Err(ActionError::ForkRecoveryRequired);
    }

    let provider_fork_already_attempted = prepared.plan.fork_attempted_at_millis.is_some();
    if !provider_fork_already_attempted {
        if ensure_live_fork_source(root, registry, &prepared.plan).is_err() {
            let _ = registry.mark_fork_recovery(&prepared.plan);
            return Err(ActionError::ForkRecoveryRequired);
        }
        // The source can park, clear, or be replaced between the initial
        // snapshot and the one permitted provider fork call.
        if ensure_live_fork_source(root, registry, &prepared.plan).is_err() {
            let _ = registry.mark_fork_recovery(&prepared.plan);
            return Err(ActionError::ForkRecoveryRequired);
        }
    }
    let prepared_plan = if provider_fork_already_attempted {
        prepared.plan
    } else {
        registry.record_fork_attempt(&prepared.plan)?
    };
    let source_session_id = prepared_plan
        .source_native_session_id
        .as_ref()
        .map(ProviderSessionId::native_id)
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let settled_turn_id = prepared_plan
        .last_settled_turn_id
        .as_deref()
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let app_server = EphemeralAppServer::default();
    let destination_result = if provider_fork_already_attempted {
        reconcile_fork(
            &app_server,
            &prepared_plan,
            source_session_id,
            settled_turn_id,
        )
    } else {
        match app_server.fork_thread(
            source_session_id,
            settled_turn_id,
            &prepared_plan.project_root,
        ) {
            Ok(destination) => Ok(destination),
            Err(_) => reconcile_fork(
                &app_server,
                &prepared_plan,
                source_session_id,
                settled_turn_id,
            ),
        }
    };
    let destination = match destination_result {
        Ok(destination) => destination,
        Err(error) => {
            let _ = registry.mark_fork_recovery(&prepared_plan);
            return Err(error);
        }
    };
    // A successful immediate fork is still before the destination TUI starts,
    // so the optional native title has no user rename race. Reconciliation is
    // intentionally different: do not overwrite an unknown later title.
    if !provider_fork_already_attempted
        && let Some(name) = provisional_fork_name(prepared_plan.source_native_name.as_deref())
    {
        let _ = app_server.set_thread_name(&destination.native_session_id, &name);
    }
    let created = registry.commit_fork(&prepared_plan, &destination.native_session_id)?;
    let _ = start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

/// Reopens one exact interrupted Fork operation without its original request
/// key. Recovery is always evidence-led: a recorded provider fork attempt is
/// reconciled rather than retried.
///
/// # Errors
///
/// Returns an error when the operation is terminal, its recorded effects are
/// not exact, the source is no longer eligible for an unattempted fork, or the
/// provider result is still ambiguous.
pub fn recover_managed_operation(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    operation_id: OperationId,
) -> Result<WorkstreamId, ActionError> {
    let plan = registry.fork_plan(operation_id)?;
    if !matches!(
        plan.operation.phase,
        OperationPhase::ExternalEffectStarted
            | OperationPhase::AwaitingReconciliation
            | OperationPhase::RecoveryRequired
    ) {
        return Err(ActionError::ForkRecoveryRequired);
    }
    if plan.operation.kind != OperationKind::Fork {
        return Err(ActionError::ForkRecoveryRequired);
    }
    require_codex_provider(plan.provider)?;
    recover_fork_operation(root, registry, plan)
}

fn recover_fork_operation(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    plan: crate::state::ForkPlan,
) -> Result<WorkstreamId, ActionError> {
    if plan.origin != crate::domain::WorkstreamOrigin::Fork {
        return Err(ActionError::ForkRecoveryRequired);
    }
    require_codex_provider(plan.provider)?;
    let provider_fork_already_attempted = plan.fork_attempted_at_millis.is_some();
    let prepared = if provider_fork_already_attempted {
        plan
    } else {
        if ensure_live_fork_source(root, registry, &plan).is_err() {
            require_fork_recovery(registry, &plan);
            return Err(ActionError::ForkRecoveryRequired);
        }
        // The marker is the exact boundary after which no path may issue a
        // second provider fork. A recovered unmarked plan may cross it once.
        registry.record_fork_attempt(&plan)?
    };
    let source_session_id = prepared
        .source_native_session_id
        .as_ref()
        .map(ProviderSessionId::native_id)
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let settled_turn_id = prepared
        .last_settled_turn_id
        .as_deref()
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let app_server = EphemeralAppServer::default();
    let destination = if provider_fork_already_attempted {
        reconcile_fork(&app_server, &prepared, source_session_id, settled_turn_id)
    } else {
        match app_server.fork_thread(source_session_id, settled_turn_id, &prepared.project_root) {
            Ok(destination) => Ok(destination),
            Err(_) => reconcile_fork(&app_server, &prepared, source_session_id, settled_turn_id),
        }
    };
    let destination = match destination {
        Ok(destination) => destination,
        Err(error) => {
            require_fork_recovery(registry, &prepared);
            return Err(error);
        }
    };
    let created = registry.commit_recovered_fork(&prepared, &destination.native_session_id)?;
    let _ = start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

fn reconcile_fork(
    app_server: &EphemeralAppServer,
    prepared: &crate::state::ForkPlan,
    source_session_id: &str,
    settled_turn_id: &str,
) -> Result<crate::provider::codex::app_server::ForkedThread, ActionError> {
    let attempted_at_millis = prepared
        .fork_attempted_at_millis
        .ok_or(ActionError::ForkRecoveryRequired)?;
    match app_server.reconcile_fork(source_session_id, settled_turn_id, attempted_at_millis) {
        Ok(ForkReconciliation::Found(destination)) => Ok(destination),
        Ok(ForkReconciliation::Absent | ForkReconciliation::Ambiguous) | Err(_) => {
            // Do not invoke `thread/fork` again. This durable operation is now
            // operator-recovery-only until exact provider evidence exists.
            // The original plan has the operation revision, which remains
            // current after `record_fork_attempt` only through the
            // updated `prepared` value passed here.
            Err(ActionError::ForkRecoveryRequired)
        }
    }
}

fn ensure_live_fork_source(
    root: &crate::state::StateRoot,
    registry: &HostRegistry,
    prepared: &crate::state::ForkPlan,
) -> Result<(), ActionError> {
    let runtime_id = prepared
        .source_runtime_id
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let source_session_id = prepared
        .source_native_session_id
        .as_ref()
        .map(ProviderSessionId::native_id)
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let runtime = registry
        .runtime_for_workstream(prepared.source_workstream_id)?
        .filter(|runtime| runtime.runtime_id == runtime_id)
        .filter(|runtime| {
            matches!(
                runtime.status,
                crate::domain::RuntimeStatus::Idle
                    | crate::domain::RuntimeStatus::Working
                    | crate::domain::RuntimeStatus::Attention
            )
        })
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let binding = registry
        .binding_for_runtime(runtime_id)?
        .filter(|binding| binding.native_session_id.native_id() == source_session_id)
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let private_runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), runtime.runtime_id, &runtime.tmux_session)?,
    );
    match private_runtime.probe()? {
        RuntimeProbe::Live { cwd, .. } if cwd == runtime.cwd => {
            // The binding is deliberately read only as evidence. Its value
            // cannot be mutated by this action.
            let _ = binding;
            Ok(())
        }
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            Err(ActionError::ForkSourceUnavailable)
        }
    }
}

fn provisional_fork_name(source_native_name: Option<&str>) -> Option<String> {
    let source_native_name = source_native_name?.trim();
    (!source_native_name.is_empty()
        && source_native_name.len() <= 505
        && !source_native_name.contains(['\n', '\r']))
    .then(|| format!("{source_native_name} · fork"))
}

fn require_fork_recovery(registry: &mut HostRegistry, prepared: &crate::state::ForkPlan) {
    if prepared.operation.phase != OperationPhase::RecoveryRequired {
        let _ = registry.mark_fork_recovery(prepared);
    }
}

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
    let prior_runtime = registry.runtime_for_workstream(workstream_id)?;
    if let Some(prior_runtime) = &prior_runtime {
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
        if matches_recorded_runtime(prior_runtime, &prior_probe, false) {
            return Ok(StartOutcome::AlreadyLive);
        }
        match prior_probe {
            RuntimeProbe::Missing => {
                if prior_runtime.process_birth.is_some()
                    && !matches!(prior_runtime.status, crate::domain::RuntimeStatus::Stopped)
                {
                    registry.mark_runtime_recovery_required(
                        prior_runtime.runtime_id,
                        prior_runtime.revision,
                    )?;
                    return Err(ActionError::NativeRecoveryRequired);
                }
                if !matches!(prior_runtime.status, crate::domain::RuntimeStatus::Stopped) {
                    registry
                        .mark_runtime_stopped(prior_runtime.runtime_id, prior_runtime.revision)?;
                }
            }
            RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
                return Err(ActionError::RuntimeProbeAmbiguous);
            }
        }
    }
    let prior_binding = prior_runtime
        .as_ref()
        .map(|runtime| registry.binding_for_runtime(runtime.runtime_id))
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
        PriorOpenCodeRuntime::Ready(binding) => binding,
    };
    let (record, endpoint, session, handle_revision) =
        prepare_opencode_runtime(registry, workstream_id, overview, prior_binding.as_ref())?;
    if let Err(error) = launch_reserved_opencode_runtime(
        root,
        registry,
        &record,
        &endpoint,
        &session,
        handle_revision,
    ) {
        if let Ok(Some(current)) = registry.runtime_for_workstream(workstream_id)
            && current.runtime_id == record.runtime_id
        {
            let _ = registry.mark_runtime_recovery_required(current.runtime_id, current.revision);
            let _ = park(root, registry, workstream_id, None);
        }
        return Err(error);
    }
    Ok(StartOutcome::Started)
}

enum PriorOpenCodeRuntime {
    AlreadyLive,
    Ready(Option<crate::state::ProviderBinding>),
}

fn inspect_opencode_prior_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<PriorOpenCodeRuntime, ActionError> {
    let Some(prior_runtime) = registry.runtime_for_workstream(workstream_id)? else {
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
        .map(PriorOpenCodeRuntime::Ready)
        .map_err(ActionError::State)
}

/// An `OpenCode` Runtime may be reported `AlreadyLive` only after every
/// persisted ownership token is corroborated again.  A tmux pane alone is
/// not enough: the exact handle generation/session, observer PID birth,
/// endpoint listener, health, and root-session status must all agree.
fn validate_opencode_live_runtime(
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
            && binding.native_session_id == handle.native_session_id
    });
    let exact = handle.runtime_generation == runtime.tmux_generation
        && handle.endpoint_host == crate::provider::opencode::LOOPBACK_HOST
        && handle.version == crate::provider::opencode::SUPPORTED_VERSION
        && runtime.status != crate::domain::RuntimeStatus::Stopped
        && handle.observer_status == crate::state::OpenCodeObserverStatus::Ready
        && observer_live
        && cwd == &runtime.cwd
        && runtime.process_birth.as_deref() == Some(process_birth)
        && endpoint_owned_by_process(&endpoint, *pane_pid, process_birth)
        && session_exact
        && OpenCodeClient::new(endpoint.clone()).health().is_ok()
        && !matches!(
            OpenCodeClient::new(endpoint).session_status(&handle.native_session_id),
            Ok(crate::provider::opencode::OpenCodeSessionStatus::Unknown) | Err(_)
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
/// The local presentation path and the remote interactive `_attach` endpoint
/// both call this function.  A private tmux pane is never enough evidence for
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
    let runtime_record = registry
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
    if !attachment_runtime_matches(&runtime_record, &probe) {
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

/// Runtime-ID form of [`preflight_attachment`] used by the SSH `_attach`
/// endpoint.  The requested opaque Runtime identity is part of the authority
/// boundary: if the Workstream has rotated to another generation between the
/// control lookup and this preflight, refuse rather than silently attaching
/// the replacement.
///
/// # Errors
///
/// Returns a bounded action error when the Runtime is unknown, no longer the
/// Workstream's current generation, or fails provider-specific preflight.
pub fn preflight_attachment_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    runtime_id: RuntimeId,
) -> Result<crate::state::RuntimeRecord, ActionError> {
    let requested = registry
        .runtime_by_id(runtime_id)?
        .ok_or(ActionError::RuntimeProbeAmbiguous)?;
    let current = preflight_attachment(root, registry, requested.workstream_id)?;
    if current.runtime_id != runtime_id {
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    Ok(current)
}

fn prepare_opencode_runtime(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    overview: &crate::state::WorkstreamOverview,
    prior_binding: Option<&crate::state::ProviderBinding>,
) -> Result<
    (
        crate::state::RuntimeRecord,
        OpenCodeEndpoint,
        ProviderSessionId,
        Revision,
    ),
    ActionError,
> {
    let record = registry.reserve_runtime_with_provider(workstream_id, ProviderKind::OpenCode)?;
    let fail = |registry: &mut HostRegistry, error: ActionError| {
        let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
        error
    };
    let (session, start_source) = if let Some(binding) = prior_binding {
        if binding.provider != ProviderKind::OpenCode {
            return Err(fail(
                registry,
                ActionError::UnsupportedProvider(ProviderKind::OpenCode),
            ));
        }
        (binding.native_session_id.clone(), "resume")
    } else {
        let port = opencode::reserve_loopback_port()
            .map_err(ActionError::OpenCode)
            .map_err(|error| fail(registry, error))?;
        let endpoint = OpenCodeEndpoint::loopback(port)
            .map_err(ActionError::OpenCode)
            .map_err(|error| fail(registry, error))?;
        let session =
            opencode::create_blank_session("opencode", &overview.project_repository_path, endpoint)
                .map_err(ActionError::OpenCode)
                .map_err(|error| fail(registry, error))?;
        (session, "new")
    };
    registry
        .bind_opencode_session(
            record.runtime_id,
            &record.tmux_generation,
            &session,
            start_source,
        )
        .map_err(|error| fail(registry, error.into()))?;
    let port = opencode::reserve_loopback_port()
        .map_err(ActionError::OpenCode)
        .map_err(|error| fail(registry, error))?;
    let endpoint = OpenCodeEndpoint::loopback(port)
        .map_err(ActionError::OpenCode)
        .map_err(|error| fail(registry, error))?;
    let handle = registry
        .record_opencode_runtime_handle(
            record.runtime_id,
            &record.tmux_generation,
            endpoint.port,
            opencode::SUPPORTED_VERSION,
            &session,
        )
        .map_err(|error| fail(registry, error.into()))?;
    Ok((record, endpoint, session, handle.revision))
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
        ProviderKind::OpenCode => Err(ActionError::ProviderRecoveryUnavailable(
            ProviderKind::OpenCode,
        )),
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
        RuntimeProbe::Missing => prior.park()?,
        RuntimeProbe::Live { .. } | RuntimeProbe::Unknown { .. } => {
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    }
    let prior_binding = registry.binding_for_runtime(prior_runtime.runtime_id)?;
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

fn matches_recorded_runtime(
    record: &crate::state::RuntimeRecord,
    probe: &RuntimeProbe,
    require_starting: bool,
) -> bool {
    (!require_starting || matches!(record.status, crate::domain::RuntimeStatus::Starting))
        && matches!(
            probe,
            RuntimeProbe::Live {
                cwd,
                process_birth: Some(process_birth),
                ..
            } if cwd == &record.cwd
                && record.process_birth.as_deref() == Some(process_birth.as_str())
        )
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
                cwd, process_birth, ..
            }) => {
                cwd != runtime_record.cwd
                    || process_birth.as_deref() != runtime_record.process_birth.as_deref()
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
        let _ = runtime.park();
        let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
        return Err(ActionError::Runtime(error));
    }
    let process_birth = match runtime.probe()? {
        RuntimeProbe::Live {
            cwd,
            process_birth: Some(process_birth),
            ..
        } if cwd == record.cwd => process_birth,
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            let _ = runtime.park();
            let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    };
    if let Err(error) =
        registry.record_runtime_process_birth(record.runtime_id, record.revision, &process_birth)
    {
        let _ = runtime.park();
        let _ = registry.mark_runtime_recovery_required(record.runtime_id, record.revision);
        return Err(ActionError::State(error));
    }
    if let Err(error) = runtime.release_launch() {
        let _ = runtime.park();
        if let Ok(Some(current)) = registry.runtime_for_workstream(record.workstream_id)
            && current.runtime_id == record.runtime_id
        {
            let _ = registry.mark_runtime_recovery_required(current.runtime_id, current.revision);
        }
        return Err(ActionError::Runtime(error));
    }
    Ok(())
}

fn launch_reserved_opencode_runtime(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    record: &crate::state::RuntimeRecord,
    endpoint: &OpenCodeEndpoint,
    session: &ProviderSessionId,
    handle_revision: Revision,
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
        let _ = runtime.park();
        return Err(ActionError::Runtime(error));
    }
    let (pane_pid, process_birth) = match runtime.probe()? {
        RuntimeProbe::Live {
            pane_pid,
            cwd,
            process_birth: Some(process_birth),
            ..
        } if cwd == record.cwd => (pane_pid, process_birth),
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            let _ = runtime.park();
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    };
    registry.record_runtime_process_birth(record.runtime_id, record.revision, &process_birth)?;
    runtime.release_launch()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !opencode::endpoint_owned_by_process(endpoint, pane_pid, &process_birth) {
        if Instant::now() >= deadline {
            let _ = runtime.park();
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
        thread::sleep(Duration::from_millis(50));
    }
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
        handle_revision,
    };
    spawn_opencode_observer(registry, &observer)?;
    Ok(())
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

fn runtime_launch_program(
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
    let record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    let opencode_handle = match record.provider {
        ProviderKind::Codex => None,
        ProviderKind::OpenCode => registry.opencode_runtime_handle(record.runtime_id)?,
    };
    if let Some(handle) = opencode_handle.as_ref() {
        stop_opencode_observer(handle)?;
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    runtime.park()?;
    if opencode_handle.is_some() {
        registry.delete_opencode_runtime_handle(record.runtime_id, &record.tmux_generation)?;
    }
    registry.park_runtime(record.runtime_id, record.revision)?;
    workstream_revision(registry, workstream_id)
}

fn stop_opencode_observer(handle: &crate::state::OpenCodeRuntimeHandle) -> Result<(), ActionError> {
    let Some(pid) = handle.observer_pid else {
        return Ok(());
    };
    let probe = LinuxProcessProbe;
    let Some(expected_birth) = handle.observer_birth.as_deref() else {
        return Err(ActionError::RuntimeProbeAmbiguous);
    };
    if expected_birth.is_empty() {
        return Err(ActionError::RuntimeProbeAmbiguous);
    }
    // A missing or changed birth token proves that the persisted helper is
    // already gone or that the PID was reused.  It is safe to continue
    // parking, but never safe to signal that PID.
    if !observer_identity_matches(&probe, pid, expected_birth) {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use nix::{errno::Errno, sys::signal, unistd::Pid};
        let result = signal::kill(
            Pid::from_raw(i32::try_from(pid).map_err(|_| ActionError::RuntimeProbeAmbiguous)?),
            signal::Signal::SIGTERM,
        );
        if let Err(error) = result {
            if error == Errno::ESRCH {
                return Ok(());
            }
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
        let deadline = Instant::now() + PARK_CONFIRM_TIMEOUT;
        while observer_identity_matches(&probe, pid, expected_birth) {
            if Instant::now() >= deadline {
                return Err(ActionError::RuntimeProbeAmbiguous);
            }
            thread::sleep(PARK_CONFIRM_POLL_INTERVAL);
        }
    }
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
    let archive_revision = if overview.lifecycle != WorkstreamLifecycle::Parked
        && overview
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.status != crate::domain::RuntimeStatus::Stopped)
    {
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

/// Builds the only native provider command permitted for a managed Runtime.
#[must_use]
pub fn codex_launch_program(
    cwd: &Path,
    binding: Option<&ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    let mut program = vec![
        "codex".into(),
        "--profile".into(),
        "wsnav-observer".into(),
        "-C".into(),
        cwd.as_os_str().to_owned(),
    ];
    if let Some(binding) = binding {
        program.push("resume".into());
        program.push(binding.native_session_id.native_id().to_owned().into());
    }
    program
}

/// Builds the recovery-only native Codex command. Deliberately omit a session
/// identifier when no authoritative binding survived: Codex then presents its
/// own resume picker, and only the observed `source=resume` selection may bind
/// the managed Runtime.
#[must_use]
pub fn codex_recovery_program(
    cwd: &Path,
    binding: Option<&ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    let mut program = codex_launch_program(cwd, None);
    program.push("resume".into());
    if let Some(binding) = binding {
        program.push(binding.native_session_id.native_id().to_owned().into());
    }
    program
}

/// Builds the environment owned by a managed Codex Runtime.
///
/// Remote starts use one-shot non-interactive SSH commands. Those commands can
/// have a POSIX locale even when the terminal that later attaches is UTF-8.
/// Set the locale only for the owned Codex process (and its hook children), so
/// its terminal renderer has a stable UTF-8 contract without changing the
/// user's shell or an unmanaged provider session.
fn managed_codex_environment() -> BTreeMap<OsString, OsString> {
    const UTF8_LOCALE: &str = "C.UTF-8";

    BTreeMap::from([
        ("LANG".into(), UTF8_LOCALE.into()),
        ("LC_CTYPE".into(), UTF8_LOCALE.into()),
        ("LC_ALL".into(), UTF8_LOCALE.into()),
    ])
}

fn ensure_workstream_revision(
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

fn require_codex_provider(provider: ProviderKind) -> Result<(), ActionError> {
    if provider == ProviderKind::Codex {
        Ok(())
    } else {
        Err(ActionError::UnsupportedProvider(provider))
    }
}

fn workstream_revision(
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

fn workstream_overview(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<crate::state::WorkstreamOverview, ActionError> {
    registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(ActionError::UnknownWorkstream)
}

fn active_workstream_overview(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<crate::state::WorkstreamOverview, ActionError> {
    let overview = workstream_overview(registry, workstream_id)?;
    if overview.archived_at_millis.is_some() {
        return Err(ActionError::WorkstreamArchived);
    }
    Ok(overview)
}

fn observer_profile(root: &crate::state::StateRoot) -> Result<ObserverProfile, ActionError> {
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

fn reconcile_observer_trust_with_manager(
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
    #[error("managed Workstream fork requires provider recovery")]
    ForkRecoveryRequired,
    #[error("fork source is no longer the exact live settled Workstream")]
    ForkSourceUnavailable,
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

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        fmt::Write as _,
        fs,
        path::{Path, PathBuf},
    };

    use super::*;

    fn registry() -> (tempfile::TempDir, crate::state::HostRegistry, WorkstreamId) {
        let temporary = tempfile::tempdir().unwrap();
        let root = crate::state::StateRoot::create(temporary.path()).unwrap();
        let mut registry = crate::state::HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_project_root(
                Path::new("/disposable/repository"),
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();
        (temporary, registry, registered.workstream_id)
    }

    #[test]
    fn completed_native_review_promotes_pending_observer_before_a_managed_action() {
        let temporary = tempfile::tempdir().unwrap();
        let root = crate::state::StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = crate::state::HostRegistry::open(&root).unwrap();
        let manager = ObserverProfile::new(
            temporary.path().join("codex-home"),
            temporary.path().join("bin/wsnav"),
            root.base(),
        );
        let ownership = manager.install("owner".to_owned(), None).unwrap();
        registry
            .record_codex_integration(ownership, IntegrationLifecycle::TrustPending)
            .unwrap();

        reconcile_observer_trust_with_manager(&mut registry, &manager).unwrap();
        assert_eq!(
            registry.codex_integration().unwrap().unwrap().lifecycle,
            IntegrationLifecycle::TrustPending
        );

        let mut trust = String::from("\n[hooks.state]\n");
        for hook in ["session_start", "user_prompt_submit", "stop", "session_end"] {
            let key =
                serde_json::to_string(&format!("{}:{hook}:0:0", manager.path().display())).unwrap();
            writeln!(
                trust,
                "\n[hooks.state.{key}]\ntrusted_hash = \"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""
            )
            .unwrap();
        }
        fs::write(manager.path(), format!("{}{}", manager.rendered(), trust)).unwrap();

        reconcile_observer_trust_with_manager(&mut registry, &manager).unwrap();
        assert_eq!(
            registry.codex_integration().unwrap().unwrap().lifecycle,
            IntegrationLifecycle::Ready
        );
    }

    #[test]
    fn archive_and_restore_without_a_runtime_never_start_codex() {
        let (temporary, mut registry, workstream_id) = registry();
        let root = crate::state::StateRoot::create(temporary.path()).unwrap();

        let archived_revision =
            archive(&root, &mut registry, workstream_id, Revision::INITIAL).unwrap();
        let archived = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        assert!(archived.archived_at_millis.is_some());
        assert!(archived.runtime.is_none());
        assert!(matches!(
            start(&root, &mut registry, workstream_id, Some(archived_revision)),
            Err(ActionError::WorkstreamArchived)
        ));
        assert!(matches!(
            park(&root, &mut registry, workstream_id, Some(archived_revision)),
            Err(ActionError::WorkstreamArchived)
        ));
        assert!(matches!(
            archive(&root, &mut registry, workstream_id, archived_revision),
            Err(ActionError::WorkstreamAlreadyArchived)
        ));

        let restored_revision = restore(&mut registry, workstream_id, archived_revision).unwrap();
        let restored = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        assert_eq!(restored.archived_at_millis, None);
        assert!(restored.runtime.is_none());
        assert_eq!(restored.revision, restored_revision);
    }

    #[test]
    fn managed_codex_environment_has_only_the_explicit_utf8_locale() {
        let environment = managed_codex_environment();

        for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
            assert_eq!(
                environment.get(&OsString::from(key)),
                Some(&OsString::from("C.UTF-8"))
            );
        }
        assert_eq!(environment.len(), 3);
    }

    #[test]
    fn native_recovery_uses_an_exact_binding_or_the_native_picker() {
        let cwd = Path::new("/disposable/repository");
        let binding = ProviderBinding {
            runtime_id: RuntimeId::new(),
            provider: crate::domain::ProviderKind::Codex,
            native_session_id: crate::domain::ProviderSessionId::codex("known-session").unwrap(),
            start_source: "resume".to_owned(),
            last_settled_turn_id: Some("settled-turn".to_owned()),
            observed_thread_name: None,
            name_state: NameState::Unavailable,
            predecessor_native_session_id: None,
            predecessor_effective_name: None,
            revision: Revision::INITIAL,
        };

        assert_eq!(
            codex_recovery_program(cwd, Some(&binding)),
            vec![
                "codex".into(),
                "--profile".into(),
                "wsnav-observer".into(),
                "-C".into(),
                cwd.as_os_str().to_owned(),
                "resume".into(),
                "known-session".into(),
            ]
        );
        assert_eq!(
            codex_recovery_program(cwd, None),
            vec![
                "codex".into(),
                "--profile".into(),
                "wsnav-observer".into(),
                "-C".into(),
                cwd.as_os_str().to_owned(),
                "resume".into(),
            ]
        );
    }

    #[test]
    fn native_provider_is_wrapped_by_the_private_launch_barrier() {
        let runtime_id = RuntimeId::new();
        let wrapped = runtime_launch_program(
            Path::new("/state"),
            runtime_id,
            vec!["codex".into(), "--profile".into(), "wsnav-observer".into()],
        )
        .unwrap();

        assert_eq!(
            &wrapped[1..],
            &[
                OsString::from("--state-root"),
                OsString::from("/state"),
                OsString::from("_runtime_launch"),
                OsString::from(runtime_id.to_string()),
                OsString::from("--"),
                OsString::from("codex"),
                OsString::from("--profile"),
                OsString::from("wsnav-observer"),
            ]
        );
    }

    #[test]
    fn conclusive_private_runtime_loss_becomes_recovery_required_before_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let root = crate::state::StateRoot::create(temporary.path()).unwrap();
        let mut registry = crate::state::HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_project_root(
                Path::new("/disposable/repository"),
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        registry
            .record_runtime_process_birth(runtime.runtime_id, runtime.revision, "birth-a")
            .unwrap();

        reconcile_lost_runtimes(&root, &mut registry).unwrap();

        let overview = registry.workstream_overviews().unwrap().remove(0);
        assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
        assert_eq!(
            overview.runtime.as_ref().map(|runtime| runtime.status),
            Some(crate::domain::RuntimeStatus::Unknown)
        );
        assert!(
            overview
                .attention
                .as_ref()
                .and_then(|attention| attention.recovery_unseen_since_revision)
                .is_some()
        );
    }

    #[test]
    fn live_runtime_is_accepted_only_when_its_recorded_identity_matches() {
        let record = crate::state::RuntimeRecord {
            runtime_id: RuntimeId::new(),
            workstream_id: WorkstreamId::new(),
            provider: crate::domain::ProviderKind::Codex,
            tmux_generation: "generation".to_owned(),
            tmux_session: "session".to_owned(),
            cwd: PathBuf::from("/disposable/repository"),
            process_birth: Some("birth-a".to_owned()),
            status: crate::domain::RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        };
        let exact = RuntimeProbe::Live {
            pane_id: "%1".to_owned(),
            pane_pid: 1,
            cwd: record.cwd.clone(),
            process_birth: Some("birth-a".to_owned()),
        };

        assert!(attachment_runtime_matches(&record, &exact));
        assert!(matches_recorded_runtime(&record, &exact, false));
        assert!(!matches_recorded_runtime(&record, &exact, true));
        assert!(!matches_recorded_runtime(
            &record,
            &RuntimeProbe::Live {
                pane_id: "%1".to_owned(),
                pane_pid: 1,
                cwd: PathBuf::from("/another/checkout"),
                process_birth: Some("birth-a".to_owned()),
            },
            false,
        ));
        assert!(!matches_recorded_runtime(
            &record,
            &RuntimeProbe::Live {
                pane_id: "%1".to_owned(),
                pane_pid: 1,
                cwd: record.cwd.clone(),
                process_birth: Some("birth-b".to_owned()),
            },
            false,
        ));
        assert!(!attachment_runtime_matches(&record, &RuntimeProbe::Missing));
        assert!(!attachment_runtime_matches(
            &record,
            &RuntimeProbe::Unknown {
                diagnostic: "probe unavailable".to_owned(),
            },
        ));
    }

    #[test]
    fn codex_attachment_requires_a_recorded_process_birth() {
        let record = crate::state::RuntimeRecord {
            runtime_id: RuntimeId::new(),
            workstream_id: WorkstreamId::new(),
            provider: crate::domain::ProviderKind::Codex,
            tmux_generation: "generation".to_owned(),
            tmux_session: "session".to_owned(),
            cwd: PathBuf::from("/disposable/repository"),
            process_birth: None,
            status: crate::domain::RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        };
        let live = RuntimeProbe::Live {
            pane_id: "%1".to_owned(),
            pane_pid: 1,
            cwd: record.cwd.clone(),
            process_birth: Some("birth-a".to_owned()),
        };

        assert!(!attachment_runtime_matches(&record, &live));
    }

    #[test]
    fn independent_creation_reuses_its_request_without_a_git_effect() {
        let (_temporary, mut registry, source) = registry();
        let first = registry
            .create_independent_workstream(
                "independent-action",
                source,
                Revision::INITIAL,
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();
        let replay = registry
            .create_independent_workstream(
                "independent-action",
                source,
                Revision::INITIAL,
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();

        assert_eq!(first, replay);
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == first.workstream_id)
            .unwrap();
        assert_eq!(
            overview.project_repository_path,
            PathBuf::from("/disposable/repository")
        );
    }

    #[test]
    fn independent_creation_keeps_the_project_root_without_touching_files() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        fs::create_dir(&repository).unwrap();
        fs::write(repository.join("source-only.txt"), "do not copy\n").unwrap();

        let root = crate::state::StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = crate::state::HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_project_root(&repository, crate::domain::ProviderKind::Codex)
            .unwrap();
        let created = registry
            .create_independent_workstream(
                "independent-system-git",
                registered.workstream_id,
                Revision::INITIAL,
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();
        let destination_root = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == created.workstream_id)
            .unwrap()
            .project_repository_path;

        assert_eq!(destination_root, repository);
        assert!(repository.join("source-only.txt").is_file());
        assert_eq!(created.origin, crate::domain::WorkstreamOrigin::Independent);
    }

    #[test]
    fn independent_creation_survives_one_provider_start_failure_without_fallback() {
        let (temporary, mut registry, source_workstream_id) = registry();
        let root = crate::state::StateRoot::create(temporary.path()).unwrap();
        let readiness_calls = Cell::new(0);
        let starter_calls = Cell::new(0);
        let starter_provider = Cell::new(None);
        let selected_provider = ProviderKind::Codex;

        let result = start_independent_workstream_with(
            &root,
            &mut registry,
            IndependentStartSpec {
                source_workstream_id,
                expected_revision: Some(Revision::INITIAL),
                request_key: "independent-start-failure",
                provider: selected_provider,
            },
            |registry, provider| {
                readiness_calls.set(readiness_calls.get() + 1);
                assert_eq!(provider, selected_provider);
                assert_eq!(
                    registry
                        .workstream_overviews()
                        .unwrap()
                        .iter()
                        .filter(|overview| overview.provider == selected_provider)
                        .count(),
                    1
                );
                Ok(())
            },
            |_root, registry, workstream_id, expected_revision, provider| {
                starter_calls.set(starter_calls.get() + 1);
                starter_provider.set(Some(provider));
                let created = registry
                    .workstream_overviews()
                    .unwrap()
                    .into_iter()
                    .find(|overview| overview.workstream_id == workstream_id)
                    .unwrap();
                assert_eq!(created.provider, selected_provider);
                assert_eq!(expected_revision, Some(created.revision));
                let reserved = registry
                    .reserve_runtime_with_provider(workstream_id, provider)
                    .unwrap();
                registry
                    .mark_runtime_recovery_required(reserved.runtime_id, reserved.revision)
                    .unwrap();
                Err(ActionError::Runtime(
                    crate::runtime::RuntimeError::TmuxRejected(
                        "fixture provider launch failed".to_owned(),
                    ),
                ))
            },
        );

        assert!(matches!(
            result,
            Err(ActionError::Runtime(
                crate::runtime::RuntimeError::TmuxRejected(ref diagnostic)
            )) if diagnostic == "fixture provider launch failed"
        ));
        assert_eq!(readiness_calls.get(), 1);
        assert_eq!(starter_calls.get(), 1);
        assert_eq!(starter_provider.get(), Some(selected_provider));

        let independent = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id != source_workstream_id)
            .expect("durable independent Workstream remains visible");
        assert_eq!(independent.provider, selected_provider);
        assert_eq!(independent.archived_at_millis, None);
        assert_eq!(independent.lifecycle, WorkstreamLifecycle::RecoveryRequired);
        let runtime = independent
            .runtime
            .expect("failed launch retains its Runtime record");
        assert_eq!(runtime.provider, selected_provider);
        assert_eq!(runtime.status, crate::domain::RuntimeStatus::Unknown);
    }

    struct FixedBirth(Option<String>);

    impl ProcessProbe for FixedBirth {
        fn process_birth(&self, _pid: u32) -> Option<String> {
            self.0.clone()
        }
    }

    #[test]
    fn observer_cleanup_refuses_missing_or_reused_birth_without_signalling() {
        assert!(!observer_identity_matches(&FixedBirth(None), 77, "birth-a"));
        assert!(!observer_identity_matches(
            &FixedBirth(Some("birth-b".to_owned())),
            77,
            "birth-a"
        ));
        assert!(!observer_identity_matches(
            &FixedBirth(Some("birth-a".to_owned())),
            77,
            ""
        ));
        assert!(observer_identity_matches(
            &FixedBirth(Some("birth-a".to_owned())),
            77,
            "birth-a"
        ));
    }

    #[test]
    fn spawned_observer_ready_requires_the_exact_live_pid_and_birth() {
        let handle = crate::state::OpenCodeRuntimeHandle {
            runtime_id: RuntimeId::new(),
            runtime_generation: "generation".to_owned(),
            endpoint_host: crate::provider::opencode::LOOPBACK_HOST.to_owned(),
            endpoint_port: 4321,
            version: crate::provider::opencode::SUPPORTED_VERSION.to_owned(),
            native_session_id: ProviderSessionId::new(ProviderKind::OpenCode, "session").unwrap(),
            observer_pid: Some(77),
            observer_birth: Some("birth-a".to_owned()),
            observer_status: crate::state::OpenCodeObserverStatus::Ready,
            revision: Revision::INITIAL,
        };
        assert!(spawned_observer_identity_matches(
            &handle,
            77,
            "birth-a",
            &FixedBirth(Some("birth-a".to_owned())),
        ));
        assert!(!spawned_observer_identity_matches(
            &handle,
            77,
            "birth-a",
            &FixedBirth(None),
        ));
        assert!(!spawned_observer_identity_matches(
            &handle,
            78,
            "birth-a",
            &FixedBirth(Some("birth-a".to_owned())),
        ));
    }
}
