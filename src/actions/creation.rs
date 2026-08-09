use super::{
    ActionError, EphemeralAppServer, ForkReconciliation, HostRegistry, LinuxProcessProbe,
    OpenCodeClient, OpenCodeEndpoint, OperationId, OperationKind, OperationPhase, PrivateRuntime,
    ProviderKind, ProviderSessionId, Revision, RuntimePaths, RuntimeProbe, StartOutcome,
    SystemTmux, WorkstreamId, opencode, start,
};
use super::{
    attachment::{PriorOpenCodeRuntime, validate_opencode_live_runtime},
    cleanup::matches_recorded_runtime,
    model::{active_workstream_overview, require_codex_provider, workstream_overview},
};

#[derive(Clone, Copy)]
pub(super) struct IndependentStartSpec<'a> {
    pub(super) source_workstream_id: WorkstreamId,
    pub(super) expected_revision: Option<Revision>,
    pub(super) request_key: &'a str,
    pub(super) provider: ProviderKind,
}

pub(super) fn start_independent_workstream_with<R, S>(
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

/// Forks an active Workstream at its last completed provider turn without
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
    if prepared.plan.operation.phase == OperationPhase::Failed {
        return if prepared.plan.provider == ProviderKind::OpenCode {
            Err(ActionError::OpenCodeForkExternalEffectUnknown)
        } else {
            Err(ActionError::ForkRecoveryRequired)
        };
    }
    if prepared.plan.operation.phase == OperationPhase::RecoveryRequired {
        return Err(ActionError::ForkRecoveryRequired);
    }

    let provider_fork_already_attempted = prepared.plan.fork_attempted_at_millis.is_some();
    if prepared.plan.provider == ProviderKind::OpenCode && provider_fork_already_attempted {
        return Err(mark_opencode_fork_unknown(registry, &prepared.plan));
    }
    if source.provider == ProviderKind::OpenCode {
        crate::provider::require_fork_eligible(registry, source.provider)
            .map_err(ActionError::ProviderReadiness)?;
    } else {
        require_codex_provider(source.provider)?;
    }
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
    if prepared_plan.provider == ProviderKind::OpenCode {
        return finish_opencode_fork(root, registry, &prepared_plan);
    }
    finish_codex_fork(
        root,
        registry,
        &prepared_plan,
        provider_fork_already_attempted,
    )
}

fn finish_opencode_fork(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    prepared: &crate::state::ForkPlan,
) -> Result<WorkstreamId, ActionError> {
    let destination = match fork_opencode_session(root, registry, prepared) {
        Ok(destination) => destination,
        Err(_error) => return Err(mark_opencode_fork_unknown(registry, prepared)),
    };
    let created = registry.commit_fork(prepared, destination.native_id())?;
    let _ = start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

fn finish_codex_fork(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    prepared: &crate::state::ForkPlan,
    provider_fork_already_attempted: bool,
) -> Result<WorkstreamId, ActionError> {
    require_codex_provider(prepared.provider)?;
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
    let destination_result = if provider_fork_already_attempted {
        reconcile_fork(&app_server, prepared, source_session_id, settled_turn_id)
    } else {
        match app_server.fork_thread(source_session_id, settled_turn_id, &prepared.project_root) {
            Ok(destination) => Ok(destination),
            Err(_) => reconcile_fork(&app_server, prepared, source_session_id, settled_turn_id),
        }
    };
    let destination = match destination_result {
        Ok(destination) => destination,
        Err(error) => {
            let _ = registry.mark_fork_recovery(prepared);
            return Err(error);
        }
    };
    // A successful immediate fork is still before the destination TUI starts,
    // so the optional native title has no user rename race. Reconciliation is
    // intentionally different: do not overwrite an unknown later title.
    if !provider_fork_already_attempted
        && let Some(name) = provisional_fork_name(prepared.source_native_name.as_deref())
    {
        let _ = app_server.set_thread_name(&destination.native_session_id, &name);
    }
    let created = registry.commit_fork(prepared, &destination.native_session_id)?;
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
    if plan.operation.phase == OperationPhase::Failed {
        return if plan.provider == ProviderKind::OpenCode {
            Err(ActionError::OpenCodeForkExternalEffectUnknown)
        } else {
            Err(ActionError::ForkRecoveryRequired)
        };
    }
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
    if plan.provider == ProviderKind::OpenCode {
        return recover_opencode_fork_operation(root, registry, &plan);
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

fn recover_opencode_fork_operation(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    plan: &crate::state::ForkPlan,
) -> Result<WorkstreamId, ActionError> {
    if plan.origin != crate::domain::WorkstreamOrigin::Fork {
        return Err(ActionError::ForkRecoveryRequired);
    }
    if plan.fork_attempted_at_millis.is_some() {
        if plan.operation.phase == OperationPhase::RecoveryRequired {
            return Err(ActionError::ForkRecoveryRequired);
        }
        return Err(mark_opencode_fork_unknown(registry, plan));
    }
    crate::provider::require_fork_eligible(registry, ProviderKind::OpenCode)
        .map_err(ActionError::ProviderReadiness)?;
    if ensure_live_fork_source(root, registry, plan).is_err() {
        require_fork_recovery(registry, plan);
        return Err(ActionError::ForkRecoveryRequired);
    }
    let prepared = registry.record_fork_attempt(plan)?;
    let destination = match fork_opencode_session(root, registry, &prepared) {
        Ok(destination) => destination,
        Err(_error) => return Err(mark_opencode_fork_unknown(registry, &prepared)),
    };
    let created = registry.commit_recovered_fork(&prepared, destination.native_id())?;
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

fn fork_opencode_session(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    prepared: &crate::state::ForkPlan,
) -> Result<ProviderSessionId, ActionError> {
    // The durable attempt marker is intentionally written before this call.
    // Revalidate the complete live source again at the narrowest available
    // boundary before issuing the one non-idempotent provider request.
    ensure_live_fork_source(root, registry, prepared)?;
    let runtime_id = prepared
        .source_runtime_id
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let source = prepared
        .source_native_session_id
        .as_ref()
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let handle = registry
        .opencode_runtime_handle(runtime_id)?
        .ok_or(ActionError::ForkSourceUnavailable)?;
    if handle.native_session_id != *source
        || handle.runtime_generation.is_empty()
        || handle.endpoint_host != opencode::LOOPBACK_HOST
    {
        return Err(ActionError::ForkSourceUnavailable);
    }
    let endpoint = OpenCodeEndpoint::loopback(handle.endpoint_port)?;
    let client = OpenCodeClient::new(endpoint);
    let destination = client
        .fork_session(
            source,
            prepared
                .last_settled_turn_id
                .as_deref()
                .ok_or(ActionError::ForkSourceUnavailable)?,
        )
        .map_err(ActionError::OpenCode)?;
    client
        .verify_root_session(&destination, &prepared.project_root)
        .map_err(ActionError::OpenCode)?;
    Ok(destination)
}

fn mark_opencode_fork_unknown(
    registry: &mut HostRegistry,
    prepared: &crate::state::ForkPlan,
) -> ActionError {
    match registry.mark_fork_external_effect_unknown(prepared) {
        Ok(()) => ActionError::OpenCodeForkExternalEffectUnknown,
        Err(error) => ActionError::State(error),
    }
}

fn ensure_live_fork_source(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
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
    let probe = private_runtime.probe()?;
    match &probe {
        RuntimeProbe::Live { .. } if matches_recorded_runtime(&runtime, &probe, false) => {
            if prepared.provider == ProviderKind::OpenCode {
                let Some(handle) = registry.opencode_runtime_handle(runtime_id)? else {
                    return Err(ActionError::ForkSourceUnavailable);
                };
                if handle.native_session_id.native_id() != source_session_id {
                    return Err(ActionError::ForkSourceUnavailable);
                }
                if !matches!(
                    validate_opencode_live_runtime(registry, &runtime, &probe)?,
                    PriorOpenCodeRuntime::AlreadyLive
                ) {
                    return Err(ActionError::ForkSourceUnavailable);
                }
            }
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
