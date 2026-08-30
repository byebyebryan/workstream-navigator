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

/// The bounded result of asking a provider to reconcile one previously
/// recorded Fork effect.  Provider adapters may have richer internal
/// responses, but orchestration only needs an exact destination or a closed
/// refusal to infer one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ForkReconciliationResult {
    Found(ProviderSessionId),
    Absent,
    Ambiguous,
}

/// Typed provider-effect seam for the current Fork orchestration.
///
/// Production uses [`NativeForkEffects`], which delegates to the concrete
/// Codex/OpenCode adapters and private Runtime launcher.  Deterministic tests
/// implement this narrow trait to exercise the same durable
/// prepare/attempt/commit/recovery path without invoking a provider, tmux, or
/// ordinary user state.  The source callback is evidence-only and is called
/// before a non-idempotent provider boundary may be crossed.
pub(super) trait ForkActionEffects {
    fn source_available(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        plan: &crate::state::ForkPlan,
    ) -> Result<(), ActionError>;

    fn provider_ready(
        &mut self,
        registry: &HostRegistry,
        provider: ProviderKind,
    ) -> Result<(), ActionError>;

    fn codex_fork(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        plan: &crate::state::ForkPlan,
    ) -> Result<ProviderSessionId, ActionError>;

    /// Best-effort native title used only after an ordinary immediate Fork
    /// returned one exact destination. Recovery and reconciliation deliberately
    /// do not call this effect because the destination may already be user
    /// renamed or may have been found after an uncertain provider response.
    fn codex_set_provisional_name(&mut self, destination: &ProviderSessionId, name: &str);

    fn codex_reconcile(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        plan: &crate::state::ForkPlan,
    ) -> Result<ForkReconciliationResult, ActionError>;

    fn opencode_fork(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        plan: &crate::state::ForkPlan,
    ) -> Result<ProviderSessionId, ActionError>;

    fn start(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        workstream_id: WorkstreamId,
        expected_revision: Option<Revision>,
    ) -> Result<(), ActionError>;
}

struct NativeForkEffects {
    app_server: EphemeralAppServer,
}

impl NativeForkEffects {
    fn new() -> Self {
        Self {
            app_server: EphemeralAppServer::default(),
        }
    }
}

impl ForkActionEffects for NativeForkEffects {
    fn source_available(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        plan: &crate::state::ForkPlan,
    ) -> Result<(), ActionError> {
        ensure_live_fork_source(root, registry, plan)
    }

    fn provider_ready(
        &mut self,
        registry: &HostRegistry,
        provider: ProviderKind,
    ) -> Result<(), ActionError> {
        if provider == ProviderKind::OpenCode {
            crate::provider::require_fork_eligible(registry, provider)
                .map_err(ActionError::ProviderReadiness)
        } else {
            require_codex_provider(provider)
        }
    }

    fn codex_fork(
        &mut self,
        _root: &crate::state::StateRoot,
        _registry: &mut HostRegistry,
        prepared: &crate::state::ForkPlan,
    ) -> Result<ProviderSessionId, ActionError> {
        let source_session_id = prepared
            .source_native_session_id
            .as_ref()
            .map(ProviderSessionId::native_id)
            .ok_or(ActionError::ForkSourceUnavailable)?;
        let settled_turn_id = prepared
            .last_settled_turn_id
            .as_deref()
            .ok_or(ActionError::ForkSourceUnavailable)?;
        let destination = self
            .app_server
            .fork_thread(source_session_id, settled_turn_id, &prepared.project_root)
            .map_err(ActionError::AppServer)?;
        ProviderSessionId::codex(destination.native_session_id)
            .map_err(|error| ActionError::State(crate::state::StateError::from(error)))
    }

    fn codex_set_provisional_name(&mut self, destination: &ProviderSessionId, name: &str) {
        let _ = self
            .app_server
            .set_thread_name(destination.native_id(), name);
    }

    fn codex_reconcile(
        &mut self,
        _root: &crate::state::StateRoot,
        _registry: &mut HostRegistry,
        prepared: &crate::state::ForkPlan,
    ) -> Result<ForkReconciliationResult, ActionError> {
        let source_session_id = prepared
            .source_native_session_id
            .as_ref()
            .map(ProviderSessionId::native_id)
            .ok_or(ActionError::ForkSourceUnavailable)?;
        let settled_turn_id = prepared
            .last_settled_turn_id
            .as_deref()
            .ok_or(ActionError::ForkSourceUnavailable)?;
        match reconcile_fork(
            &self.app_server,
            prepared,
            source_session_id,
            settled_turn_id,
        ) {
            Ok(ForkReconciliationResult::Found(destination)) => {
                Ok(ForkReconciliationResult::Found(destination))
            }
            Ok(ForkReconciliationResult::Absent) => Ok(ForkReconciliationResult::Absent),
            Ok(ForkReconciliationResult::Ambiguous) => Ok(ForkReconciliationResult::Ambiguous),
            Err(error) => Err(error),
        }
    }

    fn opencode_fork(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        prepared: &crate::state::ForkPlan,
    ) -> Result<ProviderSessionId, ActionError> {
        fork_opencode_session(root, registry, prepared)
    }

    fn start(
        &mut self,
        root: &crate::state::StateRoot,
        registry: &mut HostRegistry,
        workstream_id: WorkstreamId,
        expected_revision: Option<Revision>,
    ) -> Result<(), ActionError> {
        start(root, registry, workstream_id, expected_revision).map(|_| ())
    }
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
/// Returns an error when the source revision is stale or observer readiness
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

/// Forks an active Workstream at its last completed provider turn without
/// interrupting or waiting for the source's current turn. The destination
/// starts at the same registered project root; this action never creates or
/// validates a Git worktree. The provider fork is recorded before it is sent
/// and is never retried after an ambiguous result.
///
/// # Errors
///
/// Returns an error when the selected source lacks a live settled boundary,
/// provider evidence is not exact, observer readiness prevents the destination
/// launch, or recovery is required instead of a retry.
pub fn fork_workstream(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
) -> Result<WorkstreamId, ActionError> {
    let mut effects = NativeForkEffects::new();
    fork_workstream_with_effects(
        root,
        registry,
        source_workstream_id,
        expected_revision,
        request_key,
        &mut effects,
    )
}

/// Runs the current Fork state machine with typed provider effects supplied by
/// the caller.  The production entry point wires [`NativeForkEffects`] to the
/// Codex and `OpenCode` adapters; deterministic tests use the same path with
/// disposable effects and can therefore prove transaction ordering without
/// launching a provider or private Runtime.
pub(super) fn fork_workstream_with_effects(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
    effects: &mut dyn ForkActionEffects,
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
        effects.start(root, registry, prepared.plan.workstream_id, None)?;
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
    effects.provider_ready(registry, source.provider)?;
    if !provider_fork_already_attempted {
        if effects
            .source_available(root, registry, &prepared.plan)
            .is_err()
        {
            let _ = registry.mark_fork_recovery(&prepared.plan);
            return Err(ActionError::ForkRecoveryRequired);
        }
        // The source can park, clear, or be replaced between the initial
        // snapshot and the one permitted provider fork call.
        if effects
            .source_available(root, registry, &prepared.plan)
            .is_err()
        {
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
        let destination = match effects.opencode_fork(root, registry, &prepared_plan) {
            Ok(destination) => destination,
            Err(_error) => return Err(mark_opencode_fork_unknown(registry, &prepared_plan)),
        };
        let created = registry.commit_fork(&prepared_plan, destination.native_id())?;
        effects.start(
            root,
            registry,
            created.workstream_id,
            Some(created.revision),
        )?;
        return Ok(created.workstream_id);
    }
    let destination = if provider_fork_already_attempted {
        effects.codex_reconcile(root, registry, &prepared_plan)
    } else {
        match effects.codex_fork(root, registry, &prepared_plan) {
            Ok(destination) => {
                if let Some(name) =
                    provisional_fork_name(prepared_plan.source_native_name.as_deref())
                {
                    effects.codex_set_provisional_name(&destination, &name);
                }
                Ok(ForkReconciliationResult::Found(destination))
            }
            Err(_) => effects.codex_reconcile(root, registry, &prepared_plan),
        }
    };
    let destination = match destination {
        Ok(ForkReconciliationResult::Found(destination)) => destination,
        Ok(ForkReconciliationResult::Absent | ForkReconciliationResult::Ambiguous) => {
            let _ = registry.mark_fork_recovery(&prepared_plan);
            return Err(ActionError::ForkRecoveryRequired);
        }
        Err(error) => {
            let _ = registry.mark_fork_recovery(&prepared_plan);
            return Err(error);
        }
    };
    let created = registry.commit_fork(&prepared_plan, destination.native_id())?;
    effects.start(
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
    expected_revision: Option<Revision>,
) -> Result<WorkstreamId, ActionError> {
    let mut effects = NativeForkEffects::new();
    recover_managed_operation_with_effects(
        root,
        registry,
        operation_id,
        expected_revision,
        &mut effects,
    )
}

/// Runs the exact current managed-operation recovery path with injected
/// provider effects.  The same revision checks, one-shot attempt marker,
/// provider-specific failure state, and post-commit start ordering are used by
/// the production wrapper and deterministic action tests.
pub(super) fn recover_managed_operation_with_effects(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    operation_id: OperationId,
    expected_revision: Option<Revision>,
    effects: &mut dyn ForkActionEffects,
) -> Result<WorkstreamId, ActionError> {
    let plan = registry.fork_plan(operation_id)?;
    if expected_revision.is_some_and(|expected| expected != plan.operation.revision) {
        return Err(ActionError::OperationRevisionConflict);
    }
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
        return recover_opencode_fork_operation_with_effects(root, registry, &plan, effects);
    }
    effects.provider_ready(registry, plan.provider)?;
    recover_fork_operation_with_effects(root, registry, plan, effects)
}

fn recover_fork_operation_with_effects(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    plan: crate::state::ForkPlan,
    effects: &mut dyn ForkActionEffects,
) -> Result<WorkstreamId, ActionError> {
    if plan.origin != crate::domain::WorkstreamOrigin::Fork {
        return Err(ActionError::ForkRecoveryRequired);
    }
    let provider_fork_already_attempted = plan.fork_attempted_at_millis.is_some();
    let prepared = if provider_fork_already_attempted {
        plan
    } else {
        if effects.source_available(root, registry, &plan).is_err() {
            require_fork_recovery(registry, &plan);
            return Err(ActionError::ForkRecoveryRequired);
        }
        // The marker is the exact boundary after which no path may issue a
        // second provider fork. A recovered unmarked plan may cross it once.
        registry.record_fork_attempt(&plan)?
    };
    let destination = if provider_fork_already_attempted {
        effects.codex_reconcile(root, registry, &prepared)
    } else {
        match effects.codex_fork(root, registry, &prepared) {
            Ok(destination) => Ok(ForkReconciliationResult::Found(destination)),
            Err(_) => effects.codex_reconcile(root, registry, &prepared),
        }
    };
    let destination = match destination {
        Ok(ForkReconciliationResult::Found(destination)) => destination,
        Ok(ForkReconciliationResult::Absent | ForkReconciliationResult::Ambiguous) => {
            require_fork_recovery(registry, &prepared);
            return Err(ActionError::ForkRecoveryRequired);
        }
        Err(error) => {
            require_fork_recovery(registry, &prepared);
            return Err(error);
        }
    };
    let created = registry.commit_recovered_fork(&prepared, destination.native_id())?;
    effects.start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

fn recover_opencode_fork_operation_with_effects(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    plan: &crate::state::ForkPlan,
    effects: &mut dyn ForkActionEffects,
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
    effects.provider_ready(registry, plan.provider)?;
    if effects.source_available(root, registry, plan).is_err() {
        require_fork_recovery(registry, plan);
        return Err(ActionError::ForkRecoveryRequired);
    }
    let prepared = registry.record_fork_attempt(plan)?;
    let destination = match effects.opencode_fork(root, registry, &prepared) {
        Ok(destination) => destination,
        Err(_error) => return Err(mark_opencode_fork_unknown(registry, &prepared)),
    };
    let created = registry.commit_recovered_fork(&prepared, destination.native_id())?;
    effects.start(
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
) -> Result<ForkReconciliationResult, ActionError> {
    let attempted_at_millis = prepared
        .fork_attempted_at_millis
        .ok_or(ActionError::ForkRecoveryRequired)?;
    match app_server.reconcile_fork(source_session_id, settled_turn_id, attempted_at_millis) {
        Ok(ForkReconciliation::Found(destination)) => {
            let destination = ProviderSessionId::codex(destination.native_session_id)
                .map_err(|error| ActionError::State(crate::state::StateError::from(error)))?;
            Ok(ForkReconciliationResult::Found(destination))
        }
        Ok(ForkReconciliation::Absent) => Ok(ForkReconciliationResult::Absent),
        Ok(ForkReconciliation::Ambiguous) => Ok(ForkReconciliationResult::Ambiguous),
        Err(_) => {
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
