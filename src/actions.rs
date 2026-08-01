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
        OperationId, OperationKind, OperationPhase, Revision, RuntimeId, WorkstreamId,
        WorkstreamLifecycle,
    },
    provider::codex::app_server::{EphemeralAppServer, ForkReconciliation},
    provider::codex::profile::{ObserverProfile, ProfileError},
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{HostRegistry, IntegrationLifecycle, ProviderBinding, StateError},
    worktree::{GitWorktree, ManagedWorktree, SystemGitWorktree, WorktreeEvidence},
};

const PARK_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);
const PARK_CONFIRM_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The durable outcome of a start-or-resume request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    Started,
    AlreadyLive,
}

/// Creates an independent managed Checkout from a Workstream's configured
/// project base, then starts its first native Codex Runtime.
///
/// The source identifies only a `ProjectLocation` and expected revision. No
/// source checkout files, branch, provider conversation, or prompt data is
/// copied into the destination.
///
/// # Errors
///
/// Returns an error when the source revision is stale, the configured base is
/// unavailable, Git evidence is ambiguous, observer setup prevents the native
/// start, or the managed operation requires recovery.
pub fn start_independent_workstream(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
) -> Result<WorkstreamId, ActionError> {
    let created = create_managed_workstream(
        registry,
        source_workstream_id,
        expected_revision,
        request_key,
        OperationKind::Start,
        &SystemGitWorktree,
    )?;
    let _ = start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

/// Creates a managed checkout and forks an active Codex Workstream at its last
/// completed turn, without interrupting or waiting for the source's current
/// turn. The provider fork is recorded before it is sent and is never retried
/// after an ambiguous result.
///
/// # Errors
///
/// Returns an error when the selected source lacks a live settled boundary,
/// Git or provider evidence is not exact, observer setup prevents the
/// destination launch, or recovery is required instead of a retry.
pub fn fork_workstream(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
) -> Result<WorkstreamId, ActionError> {
    let prepared = prepare_managed_worktree(
        registry,
        source_workstream_id,
        expected_revision,
        request_key,
        OperationKind::Fork,
        &SystemGitWorktree,
    )?;
    if prepared.plan.operation.phase == OperationPhase::Committed {
        let _ = start(root, registry, prepared.plan.workstream_id, None)?;
        return Ok(prepared.plan.workstream_id);
    }
    if prepared.plan.operation.phase == OperationPhase::RecoveryRequired {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }

    let provider_fork_already_attempted = prepared.plan.fork_attempted_at_millis.is_some();
    if provider_fork_already_attempted {
        // A recorded provider attempt has already crossed the only fork-call
        // boundary. The source may now legitimately be parked or replaced;
        // reconcile only the destination after confirming its checkout.
        ensure_managed_worktree_evidence(registry, &prepared, &SystemGitWorktree)?;
    } else {
        if ensure_live_fork_source(root, registry, &prepared.plan).is_err() {
            let _ = registry.mark_managed_workstream_recovery(&prepared.plan);
            return Err(ActionError::ManagedWorkstreamRecoveryRequired);
        }
        ensure_managed_worktree_evidence(registry, &prepared, &SystemGitWorktree)?;
        // The source can park, clear, or be replaced while Git creates the
        // target; check the exact same Runtime/binding evidence again before
        // the one permitted provider fork call.
        if ensure_live_fork_source(root, registry, &prepared.plan).is_err() {
            let _ = registry.mark_managed_workstream_recovery(&prepared.plan);
            return Err(ActionError::ManagedWorkstreamRecoveryRequired);
        }
    }
    let prepared_plan = if provider_fork_already_attempted {
        prepared.plan
    } else {
        registry.record_managed_fork_attempt(&prepared.plan)?
    };
    let source_session_id = prepared_plan
        .source_native_session_id
        .as_deref()
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
            &prepared_plan.worktree_path,
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
            let _ = registry.mark_managed_workstream_recovery(&prepared_plan);
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
    let created = registry
        .commit_forked_managed_workstream(&prepared_plan, &destination.native_session_id)?;
    let _ = start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

/// Reopens one exact interrupted Start or Fork operation without its original
/// request key. Recovery is always evidence-led: it never recreates a missing
/// Git effect, and a recorded provider fork attempt is reconciled rather than
/// retried.
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
    let plan = registry.managed_workstream_plan(operation_id)?;
    if !matches!(
        plan.operation.phase,
        OperationPhase::ExternalEffectStarted
            | OperationPhase::AwaitingReconciliation
            | OperationPhase::RecoveryRequired
    ) {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }
    match plan.operation.kind {
        OperationKind::Start => recover_independent_operation(root, registry, &plan),
        OperationKind::Fork => recover_fork_operation(root, registry, plan),
    }
}

fn recover_independent_operation(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    plan: &crate::state::ManagedWorkstreamPlan,
) -> Result<WorkstreamId, ActionError> {
    if plan.origin != crate::domain::WorkstreamOrigin::Independent {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }
    ensure_recorded_managed_worktree_evidence(registry, plan, &SystemGitWorktree)?;
    let created = registry.commit_recovered_managed_workstream(plan)?;
    let _ = start(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
    )?;
    Ok(created.workstream_id)
}

fn recover_fork_operation(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    plan: crate::state::ManagedWorkstreamPlan,
) -> Result<WorkstreamId, ActionError> {
    if plan.origin != crate::domain::WorkstreamOrigin::Fork {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }
    ensure_recorded_managed_worktree_evidence(registry, &plan, &SystemGitWorktree)?;

    let provider_fork_already_attempted = plan.fork_attempted_at_millis.is_some();
    let prepared = if provider_fork_already_attempted {
        plan
    } else {
        if ensure_live_fork_source(root, registry, &plan).is_err() {
            require_managed_operation_recovery(registry, &plan);
            return Err(ActionError::ManagedWorkstreamRecoveryRequired);
        }
        // The marker is the exact boundary after which no path may issue a
        // second provider fork. A recovered unmarked plan may cross it once.
        registry.record_managed_fork_attempt(&plan)?
    };
    let source_session_id = prepared
        .source_native_session_id
        .as_deref()
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let settled_turn_id = prepared
        .last_settled_turn_id
        .as_deref()
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let app_server = EphemeralAppServer::default();
    let destination = if provider_fork_already_attempted {
        reconcile_fork(&app_server, &prepared, source_session_id, settled_turn_id)
    } else {
        match app_server.fork_thread(source_session_id, settled_turn_id, &prepared.worktree_path) {
            Ok(destination) => Ok(destination),
            Err(_) => reconcile_fork(&app_server, &prepared, source_session_id, settled_turn_id),
        }
    };
    let destination = match destination {
        Ok(destination) => destination,
        Err(error) => {
            require_managed_operation_recovery(registry, &prepared);
            return Err(error);
        }
    };
    let created = registry
        .commit_recovered_forked_managed_workstream(&prepared, &destination.native_session_id)?;
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
    prepared: &crate::state::ManagedWorkstreamPlan,
    source_session_id: &str,
    settled_turn_id: &str,
) -> Result<crate::provider::codex::app_server::ForkedThread, ActionError> {
    let attempted_at_millis = prepared
        .fork_attempted_at_millis
        .ok_or(ActionError::ManagedWorkstreamRecoveryRequired)?;
    match app_server.reconcile_fork(source_session_id, settled_turn_id, attempted_at_millis) {
        Ok(ForkReconciliation::Found(destination)) => Ok(destination),
        Ok(ForkReconciliation::Absent | ForkReconciliation::Ambiguous) | Err(_) => {
            // Do not invoke `thread/fork` again. This durable operation is now
            // operator-recovery-only until exact provider evidence exists.
            // The original plan has the operation revision, which remains
            // current after `record_managed_fork_attempt` only through the
            // updated `prepared` value passed here.
            Err(ActionError::ManagedWorkstreamRecoveryRequired)
        }
    }
}

fn ensure_live_fork_source(
    root: &crate::state::StateRoot,
    registry: &HostRegistry,
    prepared: &crate::state::ManagedWorkstreamPlan,
) -> Result<(), ActionError> {
    let runtime_id = prepared
        .source_runtime_id
        .ok_or(ActionError::ForkSourceUnavailable)?;
    let source_session_id = prepared
        .source_native_session_id
        .as_deref()
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
        .filter(|binding| binding.native_session_id == source_session_id)
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

fn create_managed_workstream(
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
    kind: OperationKind,
    git: &dyn GitWorktree,
) -> Result<crate::state::ManagedWorkstream, ActionError> {
    if kind != OperationKind::Start {
        return Err(ActionError::ManagedWorkstreamKindUnavailable);
    }
    let prepared = prepare_managed_worktree(
        registry,
        source_workstream_id,
        expected_revision,
        request_key,
        kind,
        git,
    )?;
    if prepared.plan.operation.phase == OperationPhase::Committed {
        return registry
            .commit_managed_workstream(&prepared.plan)
            .map_err(Into::into);
    }
    ensure_managed_worktree_evidence(registry, &prepared, git)?;
    registry
        .commit_managed_workstream(&prepared.plan)
        .map_err(Into::into)
}

fn prepare_managed_worktree(
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: String,
    kind: OperationKind,
    git: &dyn GitWorktree,
) -> Result<crate::state::ManagedWorkstreamPreparation, ActionError> {
    let location = registry.workstream_git_location(source_workstream_id)?;
    if expected_revision.is_some_and(|expected| expected != location.source_revision) {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    let base_commit = git.resolve_commit(&location.repository_path, &location.default_base_ref)?;
    registry
        .prepare_managed_workstream(
            request_key,
            kind,
            source_workstream_id,
            location.source_revision,
            base_commit,
        )
        .map_err(Into::into)
}

fn ensure_managed_worktree_evidence(
    registry: &mut HostRegistry,
    prepared: &crate::state::ManagedWorkstreamPreparation,
    git: &dyn GitWorktree,
) -> Result<(), ActionError> {
    if prepared.plan.operation.phase == OperationPhase::RecoveryRequired {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }
    if prepared.plan.operation.phase != OperationPhase::ExternalEffectStarted {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }
    if prepared.newly_prepared {
        let worktree = ManagedWorktree {
            repository: prepared.plan.repository_path.clone(),
            path: prepared.plan.worktree_path.clone(),
            branch: prepared.plan.branch.clone(),
            base_commit: prepared.plan.base_commit.clone(),
        };
        // A nonzero Git exit after the external effect began is not enough to
        // retry. Exact durable evidence below is the only safe way to commit.
        let _ = git.create(&worktree);
    }
    ensure_recorded_managed_worktree_evidence(registry, &prepared.plan, git)
}

fn ensure_recorded_managed_worktree_evidence(
    registry: &mut HostRegistry,
    prepared: &crate::state::ManagedWorkstreamPlan,
    git: &dyn GitWorktree,
) -> Result<(), ActionError> {
    if !matches!(
        prepared.operation.phase,
        OperationPhase::ExternalEffectStarted | OperationPhase::RecoveryRequired
    ) {
        return Err(ActionError::ManagedWorkstreamRecoveryRequired);
    }
    let worktree = ManagedWorktree {
        repository: prepared.repository_path.clone(),
        path: prepared.worktree_path.clone(),
        branch: prepared.branch.clone(),
        base_commit: prepared.base_commit.clone(),
    };
    let evidence = git.evidence(&worktree);
    match evidence {
        Ok(WorktreeEvidence::Exact) => Ok(()),
        Ok(WorktreeEvidence::Absent | WorktreeEvidence::Mismatch) | Err(_) => {
            require_managed_operation_recovery(registry, prepared);
            Err(ActionError::ManagedWorkstreamRecoveryRequired)
        }
    }
}

fn require_managed_operation_recovery(
    registry: &mut HostRegistry,
    prepared: &crate::state::ManagedWorkstreamPlan,
) {
    if prepared.operation.phase != OperationPhase::RecoveryRequired {
        let _ = registry.mark_managed_workstream_recovery(prepared);
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
    ensure_workstream_revision(registry, workstream_id, expected_revision)?;
    if workstream_lifecycle(registry, workstream_id)? == WorkstreamLifecycle::RecoveryRequired {
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
    let record = registry.reserve_runtime(workstream_id)?;
    launch_reserved_runtime(
        root,
        registry,
        &record,
        codex_launch_program(&record.cwd, prior_binding.as_ref()),
    )?;
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
    ensure_workstream_revision(registry, workstream_id, expected_revision)?;
    if workstream_lifecycle(registry, workstream_id)? != WorkstreamLifecycle::RecoveryRequired {
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
    let record = registry.reserve_runtime_recovery(workstream_id)?;
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

/// Parks one live Runtime while preserving its provider history and checkout.
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
    ensure_workstream_revision(registry, workstream_id, expected_revision)?;
    let record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    runtime.park()?;
    registry.park_runtime(record.runtime_id, record.revision)?;
    workstream_revision(registry, workstream_id)
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
        program.push(binding.native_session_id.clone().into());
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
        program.push(binding.native_session_id.clone().into());
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

fn workstream_lifecycle(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<WorkstreamLifecycle, ActionError> {
    registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .map(|overview| overview.lifecycle)
        .ok_or(ActionError::UnknownWorkstream)
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
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("workstream {0} has no runtime")]
    NoRuntime(WorkstreamId),
    #[error("observer profile is not installed; open wsnav to activate it")]
    ObserverNotInstalled,
    #[error(
        "observer profile trust is pending; open wsnav and complete native Codex /hooks review"
    )]
    ObserverNotReady,
    #[error("private runtime probe is ambiguous; refusing to create another Codex process")]
    RuntimeProbeAmbiguous,
    #[error("private runtime disappeared; select native recovery before continuing")]
    NativeRecoveryRequired,
    #[error("workstream is not awaiting native recovery")]
    NativeRecoveryUnavailable,
    #[error("workstream is unknown")]
    UnknownWorkstream,
    #[error("workstream revision changed; refresh before acting")]
    WorkstreamRevisionConflict,
    #[error("managed Workstream operation requires recovery; Git was not retried")]
    ManagedWorkstreamRecoveryRequired,
    #[error("fork source is no longer the exact live settled Workstream")]
    ForkSourceUnavailable,
    #[error("requested managed Workstream action is not available")]
    ManagedWorkstreamKindUnavailable,
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    Worktree(#[from] crate::worktree::WorktreeError),
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
        process::{Command, Stdio},
    };

    use super::*;

    struct FakeGit {
        create_calls: Cell<u8>,
        evidence: WorktreeEvidence,
    }

    impl GitWorktree for FakeGit {
        fn resolve_commit(
            &self,
            _repository: &Path,
            _reference: &str,
        ) -> Result<String, crate::worktree::WorktreeError> {
            Ok("a".repeat(40))
        }

        fn create(
            &self,
            _worktree: &ManagedWorktree,
        ) -> Result<(), crate::worktree::WorktreeError> {
            self.create_calls
                .set(self.create_calls.get().saturating_add(1));
            Ok(())
        }

        fn evidence(
            &self,
            _worktree: &ManagedWorktree,
        ) -> Result<WorktreeEvidence, crate::worktree::WorktreeError> {
            Ok(self.evidence)
        }
    }

    fn registry() -> (tempfile::TempDir, crate::state::HostRegistry, WorkstreamId) {
        let temporary = tempfile::tempdir().unwrap();
        let root = crate::state::StateRoot::create(temporary.path()).unwrap();
        let mut registry = crate::state::HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir".to_owned(),
                "main".to_owned(),
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

    fn git(repository: &Path, arguments: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repository)
                .args(arguments)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
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
            native_session_id: "known-session".to_owned(),
            start_source: "resume".to_owned(),
            last_settled_turn_id: Some("settled-turn".to_owned()),
            observed_thread_name: None,
            name_state: crate::provider::codex::names::NameState::Unavailable,
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
            .register_external_workstream(
                PathBuf::from("/disposable/repository"),
                "common-dir".to_owned(),
                "main".to_owned(),
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
    }

    #[test]
    fn independent_creation_commits_only_exact_git_evidence_and_never_retries() {
        let (_temporary, mut registry, source) = registry();
        let git = FakeGit {
            create_calls: Cell::new(0),
            evidence: WorktreeEvidence::Exact,
        };

        let first = create_managed_workstream(
            &mut registry,
            source,
            Some(Revision::INITIAL),
            "independent-action".to_owned(),
            OperationKind::Start,
            &git,
        )
        .unwrap();
        let replay = create_managed_workstream(
            &mut registry,
            source,
            None,
            "independent-action".to_owned(),
            OperationKind::Start,
            &git,
        )
        .unwrap();

        assert_eq!(first, replay);
        assert_eq!(git.create_calls.get(), 1);
    }

    #[test]
    fn ambiguous_git_evidence_marks_the_operation_for_recovery_without_retry() {
        let (_temporary, mut registry, source) = registry();
        let git = FakeGit {
            create_calls: Cell::new(0),
            evidence: WorktreeEvidence::Absent,
        };

        assert!(matches!(
            create_managed_workstream(
                &mut registry,
                source,
                Some(Revision::INITIAL),
                "independent-ambiguous".to_owned(),
                OperationKind::Start,
                &git,
            ),
            Err(ActionError::ManagedWorkstreamRecoveryRequired)
        ));
        assert!(matches!(
            create_managed_workstream(
                &mut registry,
                source,
                None,
                "independent-ambiguous".to_owned(),
                OperationKind::Start,
                &git,
            ),
            Err(ActionError::ManagedWorkstreamRecoveryRequired)
        ));
        assert_eq!(git.create_calls.get(), 1);
    }

    #[test]
    fn independent_creation_uses_the_recorded_commit_without_copying_source_files() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "-b", "main"]);
        git(
            &repository,
            &["config", "user.email", "wsnav@example.invalid"],
        );
        git(&repository, &["config", "user.name", "WSNav Test"]);
        fs::write(repository.join("committed.txt"), "base\n").unwrap();
        git(&repository, &["add", "committed.txt"]);
        git(&repository, &["commit", "-m", "base"]);
        fs::write(repository.join("source-only.txt"), "do not copy\n").unwrap();

        let root = crate::state::StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = crate::state::HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_external_workstream(
                repository.clone(),
                repository.join(".git").to_string_lossy().into_owned(),
                "HEAD".to_owned(),
            )
            .unwrap();
        let created = create_managed_workstream(
            &mut registry,
            registered.workstream_id,
            Some(Revision::INITIAL),
            "independent-system-git".to_owned(),
            OperationKind::Start,
            &SystemGitWorktree,
        )
        .unwrap();
        let destination = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == created.workstream_id)
            .unwrap()
            .checkout_path;

        assert!(destination.join("committed.txt").is_file());
        assert!(!destination.join("source-only.txt").exists());
        assert!(repository.join("source-only.txt").is_file());
        assert_eq!(created.origin, crate::domain::WorkstreamOrigin::Independent);
    }
}
