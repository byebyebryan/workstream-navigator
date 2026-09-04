//! System adapter for the account-shell gate.
//!
//! This is the only composition point that knows how to reopen inherited
//! account-shell discovery paths against the real private state, tmux, Linux
//! process table, and boot clock. The routed hidden CLI renders only the opaque
//! capability; this module itself never writes terminal output.

use std::{
    ffi::OsString,
    path::PathBuf,
    process::{self, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    account_shell::{AccountShellContext, AccountShellError},
    app::observer::{
        ObserverActivation, ObserverActivationError, ObserverReadiness,
        finalize_observer_trust_under_lease, observer_readiness, prepare_observer_activation,
    },
    clock::{Clock, SystemClock},
    domain::{OnboardingPhase, ProviderKind, RandomIdGenerator, Revision, RuntimeId},
    onboarding::{ShellCommandDecision, classify_shell_command},
    onboarding_broker::{
        PrepareContext, PreparedHandoff, PresentationBinding, SystemWorktreeInspector,
        WorktreeInspector,
    },
    onboarding_helper::{
        advance_codex_to_provider_exec_fence, begin_provider_preparation,
        record_codex_exec_failed_known_absent, record_opencode_created_session,
        record_opencode_effect_recovery_required, record_opencode_exec_recovery_required,
        record_opencode_external_effect_started, record_opencode_provider_exec_started,
        record_opencode_runtime_handle, record_opencode_session_recovery_required,
        record_provider_preparation_recovery_required,
    },
    presentation::Presentation,
    provider::codex::profile::{OBSERVER_PROFILE_NAME, ObserverProfile, ProfileInspection},
    provider_reconcile::{
        ExpectedProviderExecutable, LinuxProviderExecutableProbe, ReconcileError,
        finalize_opencode_observer_ready, prove_provider_exec,
    },
    provisional::{
        HostRetirementError, SlotError, read_marker, retire_provider_exec_proven_marker,
    },
    review::ReviewDirectory,
    runtime::{
        LinuxProcessProbe, PrivateRuntime, ProcessGroupProbe, RuntimePaths, RuntimeProbe,
        SystemTmux,
    },
    shell_gate::{
        ShellGateContext, ShellGateDecision, ShellGateError, ShellGateInvocation,
        classify_shell_gate, prepare_managed_shell_gate, validate_invocation,
    },
    state::{CurrentState, IntegrationLifecycle, RuntimeRecord, StateRoot, open_current},
};

/// The only two outcomes a shell wrapper needs from the gate. An unmanaged
/// result is intentionally side-effect-free; a managed result contains the
/// opaque one-shot capability for the future helper's private channel.
pub(crate) enum AccountShellGateOutcome {
    ExplicitlyUnmanaged,
    ObserverReadinessRequired,
    Prepared(PreparedHandoff),
}

impl std::fmt::Debug for AccountShellGateOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExplicitlyUnmanaged => formatter.write_str("ExplicitlyUnmanaged"),
            Self::ObserverReadinessRequired => formatter.write_str("ObserverReadinessRequired"),
            Self::Prepared(_) => formatter.write_str("Prepared(<opaque>)"),
        }
    }
}

/// Bounded system-adapter failures. No private discovery path, token, command,
/// process identifier, or tmux diagnostic crosses this boundary.
#[derive(Debug, Error)]
pub(crate) enum AccountShellGateError {
    #[error("account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("shell state is unavailable")]
    State,
    #[error("shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("Codex observer readiness is unavailable")]
    ObserverReadinessUnavailable,
    #[error("shell handoff is unavailable")]
    Gate(#[from] ShellGateError),
}

/// Bounded failure for the account-shell-only interactive observer flow.  No
/// provider diagnostic, argv, path, or process identity is rendered by this
/// error: the wrapper owns the fixed user-facing fallback text.
#[derive(Debug, Error)]
pub(crate) enum AccountShellObserverSetupError {
    #[error("account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("observer state is unavailable")]
    State,
    #[error("observer invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("observer provisional shell is unavailable")]
    Shell,
    #[error("observer readiness is unavailable")]
    Observer(#[from] ObserverActivationError),
    #[error("native Codex executable is unavailable")]
    Executable,
    #[error("native observer review is unavailable")]
    Review,
    #[error("observer native trust remains pending")]
    TrustPending,
}

/// Bounded failure of the final Codex account-shell exec path.
#[derive(Debug, Error)]
pub(crate) enum AccountShellCodexLaunchError {
    #[error("account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("shell state is unavailable")]
    State,
    #[error("shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("shell command is unavailable")]
    Command,
    #[error("native Codex executable is unavailable")]
    Executable,
    #[error("native Codex observer profile is unavailable")]
    Observer,
    #[error("provider launch state is unavailable")]
    Helper,
    #[error("native Codex exec failed")]
    Exec,
}

/// Bounded failure of the final `OpenCode` account-shell exec path.
/// Its `OpenCode` variant is an in-process control-flow boundary only; the
/// hidden CLI will render one fixed diagnostic rather than its source detail.
#[derive(Debug, Error)]
pub(crate) enum AccountShellOpenCodeLaunchError {
    #[error("account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("shell state is unavailable")]
    State,
    #[error("shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("shell command is unavailable")]
    Command,
    #[error("native OpenCode executable is unavailable")]
    Executable,
    #[error("OpenCode preparation is unavailable")]
    OpenCode(#[from] crate::provider::opencode::OpenCodeError),
    #[error("provider launch state is unavailable")]
    Helper,
    #[error("native OpenCode exec failed")]
    Exec,
}

/// Bounded result of presentation-owned post-exec reconciliation.
/// This path performs no provider I/O or process control: it can only record
/// exact native-exec proof already visible in the adopted private pane.
#[derive(Debug, Error)]
pub(crate) enum ProviderExecReconciliationError {
    #[error("presentation context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("provider-exec state is unavailable")]
    State,
    #[error("provider-exec proof is unavailable")]
    Reconcile(#[from] ReconcileError),
    #[error("completed onboarding retirement is unavailable")]
    Retirement(#[from] HostRetirementError),
    #[error("OpenCode observer handoff is unavailable")]
    Observer,
}

/// Immutable identity carried by the outer provisional attachment helper.
/// It is captured before the unregistered shell can become a Runtime and is
/// used only to prove the later retired handoff belongs to that same pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProvisionalAttachmentIdentity {
    pub(crate) presentation_id: uuid::Uuid,
    pub(crate) presentation_revision: Revision,
    pub(crate) slot_generation: uuid::Uuid,
    pub(crate) candidate_runtime_id: RuntimeId,
}

/// Bounded refusal while determining whether a returned provisional client
/// became the exact managed Runtime it originally carried. This path is
/// deliberately read-only: attachment-end cleanup remains in `actions`.
#[derive(Debug, Error)]
pub(crate) enum ProvisionalAttachmentReconciliationError {
    #[error("provisional attachment context is unavailable")]
    Context,
    #[error("provisional attachment state is unavailable")]
    State,
    #[error("provisional attachment handoff is unavailable")]
    Handoff,
}

const PROVISIONAL_ATTACH_RECONCILIATION_TIMEOUT: Duration = Duration::from_millis(500);
const PROVISIONAL_ATTACH_RECONCILIATION_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Finds the managed Runtime that an outer provisional attachment may
/// reconcile after its native tmux client returns.
///
/// A present matching marker is an ordinary unpromoted detach and deliberately
/// returns `None`. Once the marker has been retired, the original pane gains
/// cleanup authority only by joining its immutable presentation/slot identity
/// to one `provider_exec_proven` onboarding journal and then to the matching
/// registered Runtime generation. The caller still repeats exact exit proof
/// and revision-fenced parking through `actions`.
pub(crate) fn retired_provisional_attachment_record(
    root: &StateRoot,
    presentation: &Presentation,
    expected: ProvisionalAttachmentIdentity,
) -> Result<Option<RuntimeRecord>, ProvisionalAttachmentReconciliationError> {
    let context = presentation
        .context()
        .map_err(|_| ProvisionalAttachmentReconciliationError::Context)?;
    if context.presentation_id() != expected.presentation_id
        || context.presentation_revision() != expected.presentation_revision
    {
        return Err(ProvisionalAttachmentReconciliationError::Context);
    }

    match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => {
            if slot.presentation_id() != expected.presentation_id
                || slot.presentation_revision() != expected.presentation_revision
                || slot.slot_generation() != expected.slot_generation
                || slot.candidate_runtime_id() != expected.candidate_runtime_id
            {
                return Err(ProvisionalAttachmentReconciliationError::Context);
            }
            // The shell remains provisional. Its detached client cannot
            // mutate Runtime or Workstream state.
            return Ok(None);
        }
        Err(SlotError::MarkerUnavailable) => {}
        Err(_) => return Err(ProvisionalAttachmentReconciliationError::State),
    }

    let mut state =
        open_current(root).map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let Some(operation) = state
        .onboarding_marker_operation_current(
            &provisional_lease,
            expected.presentation_id,
            expected.presentation_revision,
            expected.slot_generation,
            expected.candidate_runtime_id,
            None,
        )
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?
    else {
        return Ok(None);
    };
    if operation.phase != OnboardingPhase::ProviderExecProven {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    }
    let target = state
        .onboarding_exec_proven_target_current(&provisional_lease, operation.operation_id)
        .map_err(|_| ProvisionalAttachmentReconciliationError::Handoff)?;
    let ownership = target.ownership();
    if ownership.operation_id != operation.operation_id
        || ownership.runtime_id != expected.candidate_runtime_id
    {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    }
    provisional_lease
        .revalidate_for_mutation(state.root())
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let registry = state
        .into_host_registry()
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let record = registry
        .runtime_by_id(expected.candidate_runtime_id)
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?
        .ok_or(ProvisionalAttachmentReconciliationError::Handoff)?;
    if !retired_provisional_runtime_matches(
        expected.candidate_runtime_id,
        ownership,
        target.runtime_generation(),
        &record,
    ) || crate::runtime::RuntimePaths::for_record(
        root.base(),
        record.runtime_id,
        &record.tmux_session,
    )
    .map_err(|_| ProvisionalAttachmentReconciliationError::Handoff)?
        != crate::runtime::RuntimePaths::for_runtime(root.base(), expected.candidate_runtime_id)
    {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    }
    Ok(Some(record))
}

/// Waits through the ordering window between a native provider exit and
/// retirement of its presentation-private marker. A live exact provisional
/// shell returns immediately and remains state-free. A retained dead pane may
/// remain pending while the durable provider proof and marker retirement join;
/// only the later exact proven target can authorize clean-exit classification.
pub(crate) fn await_retired_provisional_attachment_record(
    root: &StateRoot,
    presentation: &Presentation,
    expected: ProvisionalAttachmentIdentity,
    runtime: &PrivateRuntime<'_>,
) -> Result<Option<RuntimeRecord>, ProvisionalAttachmentReconciliationError> {
    let deadline = Instant::now() + PROVISIONAL_ATTACH_RECONCILIATION_TIMEOUT;
    await_retired_provisional_attachment_record_with(
        || retired_provisional_attachment_record(root, presentation, expected),
        || {
            provisional_attachment_requires_reconciliation_wait(
                root,
                presentation,
                expected,
                runtime,
            )
        },
        || Instant::now() < deadline,
        || thread::sleep(PROVISIONAL_ATTACH_RECONCILIATION_POLL_INTERVAL),
    )
}

fn await_retired_provisional_attachment_record_with<R, C, D, W>(
    mut read_record: R,
    mut requires_wait: C,
    mut before_deadline: D,
    mut wait: W,
) -> Result<Option<RuntimeRecord>, ProvisionalAttachmentReconciliationError>
where
    R: FnMut() -> Result<Option<RuntimeRecord>, ProvisionalAttachmentReconciliationError>,
    C: FnMut() -> Result<bool, ProvisionalAttachmentReconciliationError>,
    D: FnMut() -> bool,
    W: FnMut(),
{
    loop {
        if let Some(record) = read_record()? {
            return Ok(Some(record));
        }
        if !requires_wait()? {
            return Ok(None);
        }
        if !before_deadline() {
            return Ok(None);
        }
        wait();
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the exact marker, journal, Runtime, and retained-pane wait fence stays auditable"
)]
fn provisional_attachment_requires_reconciliation_wait(
    root: &StateRoot,
    presentation: &Presentation,
    expected: ProvisionalAttachmentIdentity,
    runtime: &PrivateRuntime<'_>,
) -> Result<bool, ProvisionalAttachmentReconciliationError> {
    let context = presentation
        .context()
        .map_err(|_| ProvisionalAttachmentReconciliationError::Context)?;
    if context.presentation_id() != expected.presentation_id
        || context.presentation_revision() != expected.presentation_revision
    {
        return Err(ProvisionalAttachmentReconciliationError::Context);
    }
    let slot = match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return Ok(false),
        Err(_) => return Err(ProvisionalAttachmentReconciliationError::State),
    };
    if slot.presentation_id() != expected.presentation_id
        || slot.presentation_revision() != expected.presentation_revision
        || slot.slot_generation() != expected.slot_generation
        || slot.candidate_runtime_id() != expected.candidate_runtime_id
    {
        return Err(ProvisionalAttachmentReconciliationError::Context);
    }
    if !matches!(
        slot.phase(),
        crate::provisional::ProvisionalPhase::RuntimeOwnedLaunching
            | crate::provisional::ProvisionalPhase::ProviderExecProven
    ) {
        return Ok(false);
    }
    let operation_id = slot
        .handoff_request()
        .map(crate::domain::OperationId::from)
        .ok_or(ProvisionalAttachmentReconciliationError::Handoff)?;
    let mut state =
        open_current(root).map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let Some(operation) = state
        .onboarding_marker_operation_current(
            &provisional_lease,
            expected.presentation_id,
            expected.presentation_revision,
            expected.slot_generation,
            expected.candidate_runtime_id,
            Some(operation_id),
        )
        .map_err(|_| ProvisionalAttachmentReconciliationError::Handoff)?
    else {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    };
    let target = if operation.phase == OnboardingPhase::ProviderExecProven {
        let target = state
            .onboarding_exec_proven_target_current(&provisional_lease, operation_id)
            .map_err(|_| ProvisionalAttachmentReconciliationError::Handoff)?;
        let ownership = target.ownership();
        if ownership.operation_id != operation_id
            || ownership.runtime_id != expected.candidate_runtime_id
            || target.runtime_generation().is_empty()
        {
            return Err(ProvisionalAttachmentReconciliationError::Handoff);
        }
        Some(target)
    } else {
        None
    };
    provisional_lease
        .revalidate_for_mutation(state.root())
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let registry = state
        .into_host_registry()
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?;
    let record = registry
        .runtime_by_id(expected.candidate_runtime_id)
        .map_err(|_| ProvisionalAttachmentReconciliationError::State)?
        .ok_or(ProvisionalAttachmentReconciliationError::Handoff)?;
    if record.status != crate::domain::RuntimeStatus::Starting
        || RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)
            .map_err(|_| ProvisionalAttachmentReconciliationError::Handoff)?
            != RuntimePaths::for_runtime(root.base(), expected.candidate_runtime_id)
    {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    }
    drop(registry);
    drop(provisional_lease);
    let Ok(probe) = runtime.probe() else {
        // The marker/runtime identity is exact, but provider topology is not
        // currently readable. Keep this bounded and state-free while the
        // marker/journal join may still converge.
        return Ok(true);
    };
    let potentially_dead = !matches!(
        probe,
        RuntimeProbe::Missing
            | RuntimeProbe::Live {
                process_birth: Some(_),
                ..
            }
    );
    if !potentially_dead {
        return Ok(false);
    }
    let Some(target) = target else {
        // The exact Runtime is retained and appears dead, but the durable
        // proof is not yet available. Keep polling without granting cleanup
        // authority; a timeout remains an untouched refusal.
        return Ok(true);
    };
    let ownership = target.ownership();
    if record.workstream_id != ownership.workstream_id
        || record.provider != target.provider()
        || record.tmux_generation != target.runtime_generation()
    {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    }
    let (Some(provider_pid), Some(provider_birth)) =
        (record.provider_pid, record.process_birth.as_deref())
    else {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    };
    if provider_birth.is_empty() {
        return Err(ProvisionalAttachmentReconciliationError::Handoff);
    }
    match runtime.provider_exit_status_with_promoted_cwd(
        provider_pid,
        &record.cwd,
        target.project_root(),
    ) {
        Ok(0) => Ok(true),
        Ok(_) | Err(_) => Ok(false),
    }
}

fn retired_provisional_runtime_matches(
    expected_runtime_id: RuntimeId,
    ownership: crate::state::current::OnboardingOwnership,
    expected_runtime_generation: &str,
    record: &RuntimeRecord,
) -> bool {
    ownership.runtime_id == expected_runtime_id
        && record.runtime_id == expected_runtime_id
        && record.workstream_id == ownership.workstream_id
        && record.tmux_generation == expected_runtime_generation
}

/// Reopens the private presentation marker before any account-shell path
/// may be used to open state. The marker's opaque identity is carried through
/// all later broker/helper fences; the inherited environment remains only
/// discovery input.
fn presentation_binding_from_account_context(
    account_context: &AccountShellContext,
) -> Result<PresentationBinding, AccountShellError> {
    let context = Presentation::context_from_directory(
        account_context.state_root(),
        account_context.presentation_directory(),
    )
    .map_err(|_| AccountShellError::ContextUnavailable)?;
    PresentationBinding::new(context.presentation_id(), context.presentation_revision())
        .map_err(|_| AccountShellError::ContextUnavailable)
}

/// Reopens one presentation-private marker and reconciles only its
/// already-owned provider exec. Codex can complete direct proof immediately.
/// `OpenCode` remains action-fenced until this controller establishes its exact
/// detached observer and proves it remains live. Neither path constructs or
/// launches a native provider command.
pub(crate) fn reconcile_provider_exec_from_presentation(
    state_root: &std::path::Path,
    presentation_directory: &std::path::Path,
) -> Result<(), ProviderExecReconciliationError> {
    let account_context = AccountShellContext::new(state_root, presentation_directory)?;
    let presentation_binding = presentation_binding_from_account_context(&account_context)
        .map_err(ProviderExecReconciliationError::Context)?;
    let root = StateRoot::select(account_context.state_root());
    let mut state = open_current(&root).map_err(|_| ProviderExecReconciliationError::State)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| ProviderExecReconciliationError::State)?;
    let slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(ReconcileError::from)?;
    presentation_binding.validate_slot(&slot).map_err(|_| {
        ProviderExecReconciliationError::Context(AccountShellError::ContextUnavailable)
    })?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    prove_provider_exec(
        &mut state,
        &provisional_lease,
        account_context.presentation_directory(),
        &runtime,
        &process_probe,
        &LinuxProviderExecutableProbe,
    )?;
    let slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(ReconcileError::from)?;
    if slot.phase() == crate::provisional::ProvisionalPhase::ProviderExecProven {
        retire_provider_exec_proven_marker(
            &state,
            &provisional_lease,
            account_context.presentation_directory(),
            &slot,
        )?;
        return Ok(());
    }
    if slot.phase() != crate::provisional::ProvisionalPhase::RuntimeOwnedLaunching {
        return Err(ReconcileError::SlotNotReady.into());
    }
    let operation_id = slot
        .handoff_request()
        .map(crate::domain::OperationId::from)
        .ok_or(ReconcileError::HandoffIdentityUnavailable)?;
    let target = state
        .onboarding_exec_proof_target_current(&provisional_lease, operation_id)
        .map_err(ReconcileError::from)?;
    if target.provider() != ProviderKind::OpenCode {
        return Err(ReconcileError::ProviderIdentityMismatch.into());
    }
    let mut registry = state
        .into_host_registry()
        .map_err(|_| ProviderExecReconciliationError::State)?;
    let record = registry
        .runtime_by_id(target.ownership().runtime_id)
        .map_err(|_| ProviderExecReconciliationError::State)?
        .ok_or(ProviderExecReconciliationError::Observer)?;
    let handle = registry
        .opencode_runtime_handle(target.ownership().runtime_id)
        .map_err(|_| ProviderExecReconciliationError::State)?
        .ok_or(ProviderExecReconciliationError::Observer)?;
    crate::actions::spawn_runtime_opencode_observer(&mut registry, root.base(), &record, &handle)
        .map_err(|_| ProviderExecReconciliationError::Observer)?;
    drop(registry);
    let mut state = open_current(&root).map_err(|_| ProviderExecReconciliationError::State)?;
    finalize_opencode_observer_ready(
        &mut state,
        &provisional_lease,
        account_context.presentation_directory(),
        &runtime,
        &process_probe,
        &process_probe,
    )?;
    let slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(ReconcileError::from)?;
    retire_provider_exec_proven_marker(
        &state,
        &provisional_lease,
        account_context.presentation_directory(),
        &slot,
    )?;
    Ok(())
}

/// Runs the complete real-host gate after a shell wrapper has supplied its own
/// PID. Provider command classification is deliberately first, so an
/// explicitly unmanaged command does not read inherited context, open state,
/// acquire a lease, inspect tmux, or sample the process table.
pub(crate) fn gate_from_account_shell(
    provider: ProviderKind,
    arguments: &[OsString],
    shell_leader_pid: u32,
) -> Result<AccountShellGateOutcome, AccountShellGateError> {
    let ShellGateDecision::Managed(command) = classify_shell_gate(provider, arguments)? else {
        return Ok(AccountShellGateOutcome::ExplicitlyUnmanaged);
    };
    let account_context = AccountShellContext::from_environment()?;
    let presentation_binding = presentation_binding_from_account_context(&account_context)?;
    let root = StateRoot::select(account_context.state_root());
    let mut state = open_current(&root).map_err(|_| AccountShellGateError::State)?;
    // Readiness is a pure preflight.  A fresh or trust-pending observer must
    // return before the provisional lease and broker can issue HandoffIssued;
    // the wrapper may then offer the explicit interactive setup flow while
    // retaining its original argv only in shell memory.
    if command.provider() == ProviderKind::Codex {
        let evidence = observer_readiness(&root, &state)
            .map_err(|_| AccountShellGateError::ObserverReadinessUnavailable)?;
        match evidence.readiness {
            ObserverReadiness::Ready => {}
            readiness if readiness.needs_interactive_setup() => {
                return Ok(AccountShellGateOutcome::ObserverReadinessRequired);
            }
            _ => return Err(AccountShellGateError::ObserverReadinessUnavailable),
        }
    }
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| AccountShellGateError::State)?;
    let process_probe = LinuxProcessProbe;
    let caller_pid = process::id();
    let caller_group = process_probe
        .process_group_checked(caller_pid)
        .map_err(|_| AccountShellGateError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellGateError::InvocationIdentityUnavailable)?;
    let tmux = SystemTmux::default();
    let clock = SystemClock;
    let ids = RandomIdGenerator;
    let context = ShellGateContext {
        presentation_directory: account_context.presentation_directory(),
        presentation_binding,
        invocation: ShellGateInvocation {
            shell_leader_pid,
            caller_pid,
            caller_group,
        },
        tmux: &tmux,
        process_probe: &process_probe,
        process_group_probe: &process_probe,
        clock: &clock,
        id_generator: &ids,
        worktree_inspector: &crate::onboarding_broker::SystemWorktreeInspector,
    };
    let prepared = prepare_managed_shell_gate(&mut state, &provisional_lease, &command, &context)?;
    Ok(AccountShellGateOutcome::Prepared(prepared))
}

/// Performs the explicit Codex observer setup requested by the account-shell
/// wrapper.  The original provider argv never enters this route: it remains
/// in the shell function and is retried only after this function returns
/// success.  The native Codex review therefore owns every visible review byte
/// in the same provider pane.
#[allow(
    clippy::too_many_lines,
    reason = "The account-shell observer handoff keeps every exact revalidation boundary auditable in one flow."
)]
pub(crate) fn prepare_observer_from_account_shell(
    shell_leader_pid: u32,
) -> Result<(), AccountShellObserverSetupError> {
    if shell_leader_pid == 0 {
        return Err(AccountShellObserverSetupError::InvocationIdentityUnavailable);
    }
    let account_context = AccountShellContext::from_environment()?;
    let presentation_binding = presentation_binding_from_account_context(&account_context)?;
    let root = StateRoot::select(account_context.state_root());
    let mut state = open_current(&root).map_err(|_| AccountShellObserverSetupError::State)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| AccountShellObserverSetupError::State)?;
    let slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    presentation_binding
        .validate_slot(&slot)
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    if slot.phase() != crate::provisional::ProvisionalPhase::Materialized {
        return Err(AccountShellObserverSetupError::Shell);
    }
    let process_probe = LinuxProcessProbe;
    let caller_pid = process::id();
    let caller_group = process_probe
        .process_group_checked(caller_pid)
        .map_err(|_| AccountShellObserverSetupError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellObserverSetupError::InvocationIdentityUnavailable)?;
    let tmux = SystemTmux::default();
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    let live = slot
        .revalidate_live_shell(&runtime, &process_probe)
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    validate_invocation(
        &live,
        ShellGateInvocation {
            shell_leader_pid,
            caller_pid,
            caller_group,
        },
    )
    .map_err(|_| AccountShellObserverSetupError::InvocationIdentityUnavailable)?;

    let evidence = observer_readiness(&root, &state)?;
    if evidence.readiness == ObserverReadiness::Ready {
        return Ok(());
    }
    if !evidence.readiness.needs_interactive_setup() {
        return Err(AccountShellObserverSetupError::Observer(
            ObserverActivationError::NotReady,
        ));
    }
    let activation = prepare_observer_activation(&root, &mut state, &provisional_lease, &evidence)?;
    let expected = match activation {
        ObserverActivation::Ready(_) => return Ok(()),
        ObserverActivation::ReviewRequired(integration) => integration,
    };
    // Do not hold the host-wide provisional lock while a human performs native
    // trust review.  The exact slot and presentation binding are captured and
    // revalidated below before lifecycle state can become Ready.
    let post_activation_slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    if post_activation_slot != slot {
        return Err(AccountShellObserverSetupError::Shell);
    }
    provisional_lease
        .revalidate_for_mutation(root.base())
        .map_err(|_| AccountShellObserverSetupError::State)?;
    drop(state);
    drop(provisional_lease);

    let mut review_directory = ReviewDirectory::create(
        account_context.presentation_directory(),
        slot.presentation_id(),
        slot.presentation_revision(),
    )
    .map_err(|_| AccountShellObserverSetupError::Review)?;
    let review_context = Presentation::context_from_directory(
        account_context.state_root(),
        account_context.presentation_directory(),
    )
    .map_err(|_| AccountShellObserverSetupError::Review)?;
    if review_context.presentation_id() != slot.presentation_id()
        || review_context.presentation_revision() != slot.presentation_revision()
    {
        return Err(AccountShellObserverSetupError::Review);
    }
    let review_result = run_native_observer_review(&expected, &review_directory.path());
    let cleanup_result = review_directory.cleanup();
    if cleanup_result.is_err() {
        return Err(AccountShellObserverSetupError::Review);
    }
    let status = review_result?;

    let mut state = open_current(&root).map_err(|_| AccountShellObserverSetupError::State)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| AccountShellObserverSetupError::State)?;
    let current_slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    presentation_binding
        .validate_slot(&current_slot)
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    if current_slot != slot {
        return Err(AccountShellObserverSetupError::Shell);
    }
    let runtime = PrivateRuntime::new(&tmux, &process_probe, current_slot.runtime_paths().clone());
    let live = current_slot
        .revalidate_live_shell(&runtime, &process_probe)
        .map_err(|_| AccountShellObserverSetupError::Shell)?;
    let caller_group = process_probe
        .process_group_checked(process::id())
        .map_err(|_| AccountShellObserverSetupError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellObserverSetupError::InvocationIdentityUnavailable)?;
    validate_invocation(
        &live,
        ShellGateInvocation {
            shell_leader_pid,
            caller_pid: process::id(),
            caller_group,
        },
    )
    .map_err(|_| AccountShellObserverSetupError::InvocationIdentityUnavailable)?;
    let _ready = finalize_observer_trust_under_lease(&root, state, &provisional_lease, &expected)
        .map_err(AccountShellObserverSetupError::Observer)?;
    if !status.success() {
        return Err(AccountShellObserverSetupError::TrustPending);
    }
    Ok(())
}

fn run_native_observer_review(
    integration: &crate::state::CodexIntegration,
    review_directory: &std::path::Path,
) -> Result<std::process::ExitStatus, AccountShellObserverSetupError> {
    let path = std::env::var_os("PATH").ok_or(AccountShellObserverSetupError::Executable)?;
    let executable = ExpectedProviderExecutable::resolve_from_path(ProviderKind::Codex, &path)
        .map_err(|_| AccountShellObserverSetupError::Executable)?;
    let codex_home = integration
        .ownership
        .canonical_path
        .parent()
        .filter(|path| path.is_absolute())
        .ok_or(AccountShellObserverSetupError::Review)?;
    let arguments = [
        "--profile".to_owned(),
        OBSERVER_PROFILE_NAME.to_owned(),
        "-C".to_owned(),
        review_directory.to_string_lossy().into_owned(),
    ];
    let program = executable.native_program(&arguments);
    let (program, arguments) = program
        .split_first()
        .ok_or(AccountShellObserverSetupError::Executable)?;
    Command::new(program)
        .args(arguments)
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|_| AccountShellObserverSetupError::Review)
}

/// Replaces the exact provisional shell with its already grammar-normalized
/// native Codex command. All state, marker, process, worktree, clock, and
/// executable evidence is revalidated while holding `provisional.lock`.
///
/// A successful Unix `execve` never returns. If it returns an operating-system
/// error, the exact known-absent journal transition is attempted before the
/// bounded failure reaches the shell. This routine has no CLI route until the
/// atomic Navigator startup.
pub(crate) fn exec_codex_from_account_shell(
    capability: &str,
    arguments: &[OsString],
) -> Result<(), AccountShellCodexLaunchError> {
    let ShellCommandDecision::ManagedFresh(launch) =
        classify_shell_command(ProviderKind::Codex, arguments)
            .map_err(|_| AccountShellCodexLaunchError::Command)?
    else {
        return Err(AccountShellCodexLaunchError::Command);
    };
    let path = std::env::var_os("PATH").ok_or(AccountShellCodexLaunchError::Executable)?;
    let executable = ExpectedProviderExecutable::resolve_from_path(ProviderKind::Codex, &path)
        .map_err(|_| AccountShellCodexLaunchError::Executable)?;
    let account_context = AccountShellContext::from_environment()?;
    let presentation_binding = presentation_binding_from_account_context(&account_context)?;
    let root = StateRoot::select(account_context.state_root());
    let mut state = open_current(&root).map_err(|_| AccountShellCodexLaunchError::State)?;
    let codex_home = codex_observer_home(&state)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| AccountShellCodexLaunchError::State)?;
    let process_probe = LinuxProcessProbe;
    let caller_pid = process::id();
    let caller_group = process_probe
        .process_group_checked(caller_pid)
        .map_err(|_| AccountShellCodexLaunchError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellCodexLaunchError::InvocationIdentityUnavailable)?;
    let tmux = SystemTmux::default();
    let slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(|_| AccountShellCodexLaunchError::Helper)?;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    let live = slot
        .revalidate_live_shell(&runtime, &process_probe)
        .map_err(|_| AccountShellCodexLaunchError::Helper)?;
    validate_invocation(
        &live,
        ShellGateInvocation {
            shell_leader_pid: caller_pid,
            caller_pid,
            caller_group,
        },
    )
    .map_err(|_| AccountShellCodexLaunchError::InvocationIdentityUnavailable)?;
    let clock = SystemClock;
    let now_monotonic_millis = clock
        .now_monotonic_millis()
        .map_err(|_| AccountShellCodexLaunchError::Helper)?;
    let boot_provenance = clock
        .boot_provenance()
        .map_err(|_| AccountShellCodexLaunchError::Helper)?;
    let ids = RandomIdGenerator;
    let context = PrepareContext {
        presentation_directory: account_context.presentation_directory(),
        presentation_binding,
        runtime: &runtime,
        process_group_probe: &process_probe,
        provider: ProviderKind::Codex,
        arguments,
        now_monotonic_millis,
        expiry_monotonic_millis: now_monotonic_millis
            .checked_add(60_000)
            .ok_or(AccountShellCodexLaunchError::Helper)?,
        boot_provenance: &boot_provenance,
        id_generator: &ids,
        worktree_inspector: &crate::onboarding_broker::SystemWorktreeInspector,
    };
    let exec_fence = advance_codex_to_provider_exec_fence(
        &mut state,
        &provisional_lease,
        &context,
        capability,
        now_monotonic_millis,
        executable.identity(),
    )
    .map_err(|_| AccountShellCodexLaunchError::Helper)?;
    let error = exec_codex_program(
        &codex_observer_program(&executable, launch.arguments()),
        &codex_home,
    );
    record_codex_exec_failed_known_absent(&mut state, &provisional_lease, &context, exec_fence)
        .map_err(|_| AccountShellCodexLaunchError::Helper)?;
    let _ = error;
    Err(AccountShellCodexLaunchError::Exec)
}

/// Requires the exact current-binary observer declaration and native trust
/// before the helper can consume a Codex launch capability. The
/// record's profile parent, rather than inherited shell state, becomes the
/// explicit `CODEX_HOME` for the final native exec.
fn codex_observer_home(state: &CurrentState) -> Result<PathBuf, AccountShellCodexLaunchError> {
    let integration = state
        .codex_integration()
        .map_err(|_| AccountShellCodexLaunchError::State)?
        .ok_or(AccountShellCodexLaunchError::Observer)?;
    if integration.lifecycle != IntegrationLifecycle::Ready {
        return Err(AccountShellCodexLaunchError::Observer);
    }
    let codex_home = integration
        .ownership
        .canonical_path
        .parent()
        .filter(|path| path.is_absolute())
        .map(PathBuf::from)
        .ok_or(AccountShellCodexLaunchError::Observer)?;
    let executable = std::env::current_exe().map_err(|_| AccountShellCodexLaunchError::Observer)?;
    let profile = ObserverProfile::new(codex_home.clone(), executable, state.root());
    if profile.inspect(Some(&integration.ownership)).ok() != Some(ProfileInspection::Ready) {
        return Err(AccountShellCodexLaunchError::Observer);
    }
    Ok(codex_home)
}

/// Builds the only -managed Codex native command. The closed grammar has
/// already rejected caller-provided profile flags, so this exact profile is
/// the single selected Codex configuration layer.
fn codex_observer_program(
    executable: &ExpectedProviderExecutable,
    arguments: &[String],
) -> Vec<OsString> {
    let mut program = executable.native_program(&[]);
    program.extend([
        OsString::from("--profile"),
        OsString::from(OBSERVER_PROFILE_NAME),
    ]);
    program.extend(arguments.iter().map(OsString::from));
    program
}

#[cfg(unix)]
fn exec_codex_program(program: &[OsString], codex_home: &std::path::Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;

    let (executable, arguments) = program
        .split_first()
        .expect("the native Codex program is constructed from an exact executable");
    let mut command = Command::new(executable);
    command.args(arguments).env("CODEX_HOME", codex_home);
    command.exec()
}

#[cfg(not(unix))]
fn exec_codex_program(_program: &[OsString], _codex_home: &std::path::Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native Codex exec is unavailable",
    )
}

/// Replaces the exact provisional shell with a native `OpenCode` command after
/// creating and binding one blank root session. The potentially non-idempotent
/// `/session` POST is preceded by a durable external-effect fence; every
/// later failure transitions the same operation to recovery-required rather
/// than guessing that the session is absent or retrying it.
///
/// A successful Unix `execve` never returns. The hidden launch-helper CLI is
/// the only routed caller.
#[allow(
    clippy::too_many_lines,
    reason = "the exact lease-held OpenCode failure boundaries must remain reviewable in one linear handoff"
)]
pub(crate) fn exec_opencode_from_account_shell(
    capability: &str,
    arguments: &[OsString],
) -> Result<(), AccountShellOpenCodeLaunchError> {
    let ShellCommandDecision::ManagedFresh(launch) =
        classify_shell_command(ProviderKind::OpenCode, arguments)
            .map_err(|_| AccountShellOpenCodeLaunchError::Command)?
    else {
        return Err(AccountShellOpenCodeLaunchError::Command);
    };
    let path = std::env::var_os("PATH").ok_or(AccountShellOpenCodeLaunchError::Executable)?;
    let expected = ExpectedProviderExecutable::resolve_from_path(ProviderKind::OpenCode, &path)
        .map_err(|_| AccountShellOpenCodeLaunchError::Executable)?;
    let executable = expected
        .native_program(&[])
        .into_iter()
        .next()
        .expect("the exact native program contains its executable");
    let account_context = AccountShellContext::from_environment()?;
    let presentation_binding = presentation_binding_from_account_context(&account_context)?;
    let root = StateRoot::select(account_context.state_root());
    let mut state = open_current(&root).map_err(|_| AccountShellOpenCodeLaunchError::State)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| AccountShellOpenCodeLaunchError::State)?;
    let process_probe = LinuxProcessProbe;
    let caller_pid = process::id();
    let caller_group = process_probe
        .process_group_checked(caller_pid)
        .map_err(|_| AccountShellOpenCodeLaunchError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellOpenCodeLaunchError::InvocationIdentityUnavailable)?;
    let tmux = SystemTmux::default();
    let slot = read_marker(state.root(), account_context.presentation_directory())
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    let live = slot
        .revalidate_live_shell(&runtime, &process_probe)
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    validate_invocation(
        &live,
        ShellGateInvocation {
            shell_leader_pid: caller_pid,
            caller_pid,
            caller_group,
        },
    )
    .map_err(|_| AccountShellOpenCodeLaunchError::InvocationIdentityUnavailable)?;
    let worktree_inspector = SystemWorktreeInspector;
    let repository = worktree_inspector
        .discover_containing_worktree(&live.cwd)
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    let clock = SystemClock;
    let now_monotonic_millis = clock
        .now_monotonic_millis()
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    let boot_provenance = clock
        .boot_provenance()
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    let ids = RandomIdGenerator;
    let context = PrepareContext {
        presentation_directory: account_context.presentation_directory(),
        presentation_binding,
        runtime: &runtime,
        process_group_probe: &process_probe,
        provider: ProviderKind::OpenCode,
        arguments,
        now_monotonic_millis,
        expiry_monotonic_millis: now_monotonic_millis
            .checked_add(60_000)
            .ok_or(AccountShellOpenCodeLaunchError::Helper)?,
        boot_provenance: &boot_provenance,
        id_generator: &ids,
        worktree_inspector: &worktree_inspector,
    };
    let preparation = begin_provider_preparation(
        &mut state,
        &provisional_lease,
        &context,
        capability,
        now_monotonic_millis,
        expected.identity(),
    )
    .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    let preparation_ownership = preparation.ownership();
    let endpoint = match crate::provider::opencode::reserve_loopback_port()
        .and_then(crate::provider::opencode::OpenCodeEndpoint::loopback)
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            record_provider_preparation_recovery_required(
                &mut state,
                &provisional_lease,
                &context,
                preparation_ownership,
            )
            .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
            return Err(error.into());
        }
    };
    let mut effect_fence = None;
    let created = crate::provider::opencode::create_blank_session_with_before_create_and_health(
        &executable,
        &repository.project_root,
        endpoint.clone(),
        || {
            let fence = record_opencode_external_effect_started(
                &mut state,
                &provisional_lease,
                &context,
                preparation,
            )
            .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
            effect_fence = Some(fence);
            Ok(())
        },
    );
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            if let Some(fence) = effect_fence.as_ref() {
                record_opencode_effect_recovery_required(
                    &mut state,
                    &provisional_lease,
                    &context,
                    fence,
                )
                .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
            } else {
                record_provider_preparation_recovery_required(
                    &mut state,
                    &provisional_lease,
                    &context,
                    preparation_ownership,
                )
                .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
            }
            return Err(error);
        }
    };
    let effect_fence = effect_fence.expect(
        "a successfully returned OpenCode session requires the pre-POST external-effect fence",
    );
    let Ok(session_fence) = record_opencode_created_session(
        &mut state,
        &provisional_lease,
        &context,
        &effect_fence,
        created.session,
    ) else {
        record_opencode_effect_recovery_required(
            &mut state,
            &provisional_lease,
            &context,
            &effect_fence,
        )
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
        return Err(AccountShellOpenCodeLaunchError::Helper);
    };
    let Ok(handle_fence) = record_opencode_runtime_handle(
        &mut state,
        &provisional_lease,
        &context,
        &session_fence,
        endpoint.port,
        &created.version,
    ) else {
        record_opencode_session_recovery_required(
            &mut state,
            &provisional_lease,
            &context,
            &session_fence,
        )
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
        return Err(AccountShellOpenCodeLaunchError::Helper);
    };
    let Ok(exec_fence) = record_opencode_provider_exec_started(
        &mut state,
        &provisional_lease,
        &context,
        &handle_fence,
    ) else {
        record_opencode_session_recovery_required(
            &mut state,
            &provisional_lease,
            &context,
            &session_fence,
        )
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
        return Err(AccountShellOpenCodeLaunchError::Helper);
    };
    let mut program = crate::provider::opencode::native_command(
        executable,
        &repository.project_root,
        &endpoint,
        handle_fence.session(),
    );
    program.extend(launch.arguments().iter().map(OsString::from));
    let _ = exec_program(&program);
    record_opencode_exec_recovery_required(&mut state, &provisional_lease, &context, exec_fence)
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    Err(AccountShellOpenCodeLaunchError::Exec)
}

#[cfg(unix)]
fn exec_program(program: &[OsString]) -> std::io::Error {
    use std::os::unix::process::CommandExt;

    let (executable, arguments) = program
        .split_first()
        .expect("the native program is constructed from an exact executable");
    let mut command = Command::new(executable);
    command.args(arguments);
    command.exec()
}

#[cfg(not(unix))]
fn exec_program(_program: &[OsString]) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native exec is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::{cell::Cell, ffi::OsString};

    use super::{
        AccountShellGateOutcome, AccountShellOpenCodeLaunchError, ProviderExecReconciliationError,
        codex_observer_program, exec_opencode_from_account_shell, gate_from_account_shell,
        reconcile_provider_exec_from_presentation,
    };
    use crate::{
        account_shell::AccountShellError,
        domain::{
            LocationId, OperationId, ProviderKind, Revision, RuntimeId, RuntimeStatus, WorkstreamId,
        },
        presentation::Presentation,
        provider_reconcile::ExpectedProviderExecutable,
        provisional::{ProvisionalSlot, SlotGeneration, read_marker, write_new_marker},
        state::{
            RuntimeRecord, StateRoot, create_current, current::OnboardingOwnership, open_current,
        },
    };

    use super::{
        ProvisionalAttachmentIdentity, await_retired_provisional_attachment_record_with,
        retired_provisional_attachment_record, retired_provisional_runtime_matches,
    };

    fn retained_test_runtime() -> RuntimeRecord {
        RuntimeRecord {
            runtime_id: RuntimeId::from(uuid::Uuid::from_u128(51)),
            workstream_id: WorkstreamId::from(uuid::Uuid::from_u128(52)),
            provider: ProviderKind::Codex,
            tmux_generation: "generation-a".to_owned(),
            tmux_session: "wsnav-generation-a".to_owned(),
            cwd: std::env::temp_dir(),
            provider_pid: Some(99),
            process_birth: Some("birth-99".to_owned()),
            status: RuntimeStatus::Starting,
            revision: Revision::INITIAL,
        }
    }

    #[test]
    fn provisional_attachment_waits_for_launching_proof_then_marker_retirement() {
        let durable_proof = Cell::new(false);
        let marker_retired = Cell::new(false);
        let proof_checks = Cell::new(0_u8);
        let reads = Cell::new(0_u8);
        let waits = Cell::new(0_u8);
        let expected = retained_test_runtime();
        let result = await_retired_provisional_attachment_record_with(
            || {
                let attempt = reads.get();
                reads.set(attempt.saturating_add(1));
                Ok(marker_retired.get().then(|| expected.clone()))
            },
            || {
                match proof_checks.get() {
                    0 => assert!(!durable_proof.get()),
                    1 => assert!(durable_proof.get() && !marker_retired.get()),
                    _ => panic!("marker retirement should be observed next"),
                }
                proof_checks.set(proof_checks.get().saturating_add(1));
                Ok(true)
            },
            || true,
            || {
                let attempt = waits.get();
                waits.set(attempt.saturating_add(1));
                if attempt == 0 {
                    durable_proof.set(true);
                } else {
                    marker_retired.set(true);
                }
            },
        )
        .unwrap();

        assert_eq!(result, Some(expected));
        assert_eq!(reads.get(), 3);
        assert_eq!(waits.get(), 2);
    }

    #[test]
    fn provisional_attachment_timeout_without_proof_stays_untouched() {
        let waits = Cell::new(0_u8);
        let result = await_retired_provisional_attachment_record_with(
            || Ok(None),
            || Ok(true),
            || waits.get() < 2,
            || waits.set(waits.get().saturating_add(1)),
        )
        .unwrap();

        assert_eq!(result, None);
        assert_eq!(waits.get(), 2);
    }

    #[test]
    fn provisional_attachment_live_detach_stays_state_free() {
        let waits = Cell::new(0_u8);
        let result = await_retired_provisional_attachment_record_with(
            || Ok(None),
            || Ok(false),
            || panic!("live detach must not enter the polling deadline"),
            || waits.set(waits.get().saturating_add(1)),
        )
        .unwrap();

        assert_eq!(result, None);
        assert_eq!(waits.get(), 0);
    }

    #[test]
    fn retired_provisional_attachment_requires_the_original_runtime_and_generation() {
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(1));
        let workstream_id = WorkstreamId::from(uuid::Uuid::from_u128(2));
        let ownership = OnboardingOwnership {
            operation_id: OperationId::from(uuid::Uuid::from_u128(3)),
            location_id: LocationId::from(uuid::Uuid::from_u128(4)),
            workstream_id,
            runtime_id,
            operation_revision: Revision::INITIAL,
        };
        let mut record = RuntimeRecord {
            runtime_id,
            workstream_id,
            provider: ProviderKind::Codex,
            tmux_generation: "generation-a".to_owned(),
            tmux_session: format!("wsnav-{runtime_id}"),
            cwd: std::env::temp_dir(),
            provider_pid: Some(99),
            process_birth: Some("birth-99".to_owned()),
            status: RuntimeStatus::Attention,
            revision: Revision::INITIAL,
        };

        assert!(retired_provisional_runtime_matches(
            runtime_id,
            ownership,
            "generation-a",
            &record,
        ));
        record.tmux_generation = "generation-b".to_owned();
        assert!(!retired_provisional_runtime_matches(
            runtime_id,
            ownership,
            "generation-a",
            &record,
        ));
        record.tmux_generation = "generation-a".to_owned();
        record.runtime_id = RuntimeId::from(uuid::Uuid::from_u128(5));
        assert!(!retired_provisional_runtime_matches(
            runtime_id,
            ownership,
            "generation-a",
            &record,
        ));
    }

    #[test]
    fn unpromoted_provisional_attachment_end_keeps_the_marker_and_registry_unchanged() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        let executable = temporary.path().join("wsnav-fixture");
        fs::create_dir(&seed).unwrap();
        fs::write(&executable, "#!/bin/sh\nexec sleep 60\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        drop(create_current(&state_path, &crate::domain::RandomIdGenerator).unwrap());
        let presentation = Presentation::fresh_with_executable(&state_path, executable);
        presentation
            .start(uuid::Uuid::from_u128(11), &seed)
            .unwrap();
        let root = StateRoot::select(&state_path);
        let mut state = open_current(&root).unwrap();
        let lease = state.acquire_provisional_lease().unwrap();
        let slot = ProvisionalSlot::materializing(
            state.root(),
            uuid::Uuid::from_u128(11),
            Revision::INITIAL,
            lease.lease_generation(),
            RuntimeId::from(uuid::Uuid::from_u128(12)),
            SlotGeneration::new(uuid::Uuid::from_u128(13)),
            &seed,
        )
        .unwrap();
        write_new_marker(state.root(), &presentation.paths().directory, &slot).unwrap();
        drop(lease);
        drop(state);

        assert_eq!(
            retired_provisional_attachment_record(
                &root,
                &presentation,
                ProvisionalAttachmentIdentity {
                    presentation_id: slot.presentation_id(),
                    presentation_revision: slot.presentation_revision(),
                    slot_generation: slot.slot_generation(),
                    candidate_runtime_id: slot.candidate_runtime_id(),
                },
            )
            .unwrap(),
            None,
        );
        let state = open_current(&root).unwrap();
        assert_eq!(
            read_marker(state.root(), &presentation.paths().directory).unwrap(),
            slot
        );
        assert!(state.registered_runtime_paths().unwrap().is_empty());
        drop(state);
        presentation.close().unwrap();
    }

    #[test]
    fn unmanaged_commands_return_before_the_real_host_context_is_required() {
        let outcome =
            gate_from_account_shell(ProviderKind::Codex, &[OsString::from("--version")], 1)
                .unwrap();
        assert!(matches!(
            outcome,
            AccountShellGateOutcome::ExplicitlyUnmanaged
        ));
    }

    #[test]
    fn opencode_exec_refuses_provider_owned_queries_before_host_discovery() {
        assert!(matches!(
            exec_opencode_from_account_shell("unreachable", &[OsString::from("--version")]),
            Err(AccountShellOpenCodeLaunchError::Command)
        ));
    }

    #[test]
    fn managed_codex_exec_forces_the_exact_observer_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("codex");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let executable = ExpectedProviderExecutable::new(ProviderKind::Codex, &executable).unwrap();

        assert_eq!(
            codex_observer_program(&executable, &["--model".to_owned(), "gpt-5.6".to_owned()],),
            vec![
                executable.native_program(&[])[0].clone(),
                "--profile".into(),
                "wsnav-observer".into(),
                "--model".into(),
                "gpt-5.6".into(),
            ]
        );
    }

    #[test]
    fn post_exec_reconciliation_refuses_an_outside_presentation_before_state_open() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let outside = temporary.path().join("outside");
        fs::create_dir(&state_root).unwrap();
        fs::create_dir(&outside).unwrap();
        assert!(matches!(
            reconcile_provider_exec_from_presentation(&state_root, &outside),
            Err(ProviderExecReconciliationError::Context(
                AccountShellError::ContextUnavailable
            ))
        ));
    }
}
