//! Dormant system adapter for the D17 account-shell gate.
//!
//! This is the only composition point that knows how to reopen inherited
//! account-shell discovery paths against the real private state, tmux, Linux
//! process table, and boot clock. The future hidden CLI may render its opaque
//! capability on stdout, but this module itself never writes terminal output
//! or exposes a command route. Its dormant direct-exec adapters are only
//! callable after the future atomic Navigator cutover.

#![allow(
    dead_code,
    reason = "the D17 account-shell control remains unreachable until the atomic Navigator cutover"
)]

use std::{
    ffi::OsString,
    path::PathBuf,
    process::{self, Command},
};

use thiserror::Error;

use crate::{
    d17_account_shell::{AccountShellContext, AccountShellError},
    d17_broker::{
        PrepareContext, PreparedHandoff, PresentationBinding, SystemWorktreeInspector,
        WorktreeInspector,
    },
    d17_clock::{D17Clock, SystemD17Clock},
    d17_helper::{
        advance_codex_to_provider_exec_fence, begin_provider_preparation,
        record_codex_exec_failed_known_absent, record_opencode_created_session,
        record_opencode_effect_recovery_required, record_opencode_exec_recovery_required,
        record_opencode_external_effect_started, record_opencode_provider_exec_started,
        record_opencode_runtime_handle, record_opencode_session_recovery_required,
        record_provider_preparation_recovery_required,
    },
    d17_reconcile::{
        ExpectedProviderExecutable, LinuxProviderExecutableProbe, ReconcileError,
        finalize_opencode_observer_ready, prove_provider_exec,
    },
    d17_shell_gate::{
        ShellGateContext, ShellGateDecision, ShellGateError, ShellGateInvocation,
        classify_shell_gate, prepare_managed_shell_gate, validate_invocation,
    },
    domain::{ProviderKind, RandomIdGenerator},
    onboarding::{ShellCommandDecision, classify_shell_command},
    presentation::Presentation,
    provider::codex::profile::{OBSERVER_PROFILE_NAME, ObserverProfile, ProfileInspection},
    provisional::read_marker,
    runtime::{LinuxProcessProbe, PrivateRuntime, ProcessGroupProbe, SystemTmux},
    state::{D16State, IntegrationLifecycle, StateRoot, open_d17_current_only},
};

/// The only two outcomes a shell wrapper needs from the gate. An unmanaged
/// result is intentionally side-effect-free; a managed result contains the
/// opaque one-shot capability for the future helper's private channel.
pub(crate) enum AccountShellGateOutcome {
    ExplicitlyUnmanaged,
    Prepared(PreparedHandoff),
}

impl std::fmt::Debug for AccountShellGateOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExplicitlyUnmanaged => formatter.write_str("ExplicitlyUnmanaged"),
            Self::Prepared(_) => formatter.write_str("Prepared(<opaque>)"),
        }
    }
}

/// Bounded system-adapter failures. No private discovery path, token, command,
/// process identifier, or tmux diagnostic crosses this boundary.
#[derive(Debug, Error)]
pub(crate) enum AccountShellGateError {
    #[error("D17 account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("D17 shell state is unavailable")]
    State,
    #[error("D17 shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("D17 shell handoff is unavailable")]
    Gate(#[from] ShellGateError),
}

/// Bounded failure of the dormant final Codex account-shell exec path.
#[derive(Debug, Error)]
pub(crate) enum AccountShellCodexLaunchError {
    #[error("D17 account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("D17 shell state is unavailable")]
    State,
    #[error("D17 shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("D17 shell command is unavailable")]
    Command,
    #[error("D17 native Codex executable is unavailable")]
    Executable,
    #[error("D17 native Codex observer profile is unavailable")]
    Observer,
    #[error("D17 provider launch state is unavailable")]
    Helper,
    #[error("D17 native Codex exec failed")]
    Exec,
}

/// Bounded failure of the dormant final `OpenCode` account-shell exec path.
/// Its `OpenCode` variant is an in-process control-flow boundary only; the
/// hidden CLI will render one fixed diagnostic rather than its source detail.
#[derive(Debug, Error)]
pub(crate) enum AccountShellOpenCodeLaunchError {
    #[error("D17 account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("D17 shell state is unavailable")]
    State,
    #[error("D17 shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("D17 shell command is unavailable")]
    Command,
    #[error("D17 native OpenCode executable is unavailable")]
    Executable,
    #[error("D17 OpenCode preparation is unavailable")]
    OpenCode(#[from] crate::provider::opencode::OpenCodeError),
    #[error("D17 provider launch state is unavailable")]
    Helper,
    #[error("D17 native OpenCode exec failed")]
    Exec,
}

/// Bounded result of a dormant presentation-owned post-exec reconciliation.
/// This path performs no provider I/O or process control: it can only record
/// exact native-exec proof already visible in the adopted private pane.
#[derive(Debug, Error)]
pub(crate) enum ProviderExecReconciliationError {
    #[error("D17 presentation context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("D17 provider-exec state is unavailable")]
    State,
    #[error("D17 provider-exec proof is unavailable")]
    Reconcile(#[from] ReconcileError),
    #[error("D17 OpenCode observer handoff is unavailable")]
    Observer,
}

/// Reopens the private D17 presentation marker before any account-shell path
/// may be used to open state. The marker's opaque identity is carried through
/// all later broker/helper fences; the inherited environment remains only
/// discovery input.
fn presentation_binding_from_account_context(
    account_context: &AccountShellContext,
) -> Result<PresentationBinding, AccountShellError> {
    let context = Presentation::d17_context_from_directory(
        account_context.state_root(),
        account_context.presentation_directory(),
    )
    .map_err(|_| AccountShellError::ContextUnavailable)?;
    PresentationBinding::new(context.presentation_id(), context.presentation_revision())
        .map_err(|_| AccountShellError::ContextUnavailable)
}

/// Reopens one presentation-private D17 marker and reconciles only its
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
    let mut state =
        open_d17_current_only(&root).map_err(|_| ProviderExecReconciliationError::State)?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
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
        .d17_onboarding_exec_proof_target_current(&provisional_lease, operation_id)
        .map_err(ReconcileError::from)?;
    if target.provider() != ProviderKind::OpenCode {
        return Err(ReconcileError::ProviderIdentityMismatch.into());
    }
    let mut registry = state
        .into_d17_host_registry()
        .map_err(|_| ProviderExecReconciliationError::State)?;
    let record = registry
        .observer_runtime_by_id(target.ownership().runtime_id)
        .map_err(|_| ProviderExecReconciliationError::State)?
        .ok_or(ProviderExecReconciliationError::Observer)?;
    let handle = registry
        .observer_opencode_runtime_handle(target.ownership().runtime_id)
        .map_err(|_| ProviderExecReconciliationError::State)?
        .ok_or(ProviderExecReconciliationError::Observer)?;
    crate::actions::spawn_d17_opencode_observer(&mut registry, root.base(), &record, &handle)
        .map_err(|_| ProviderExecReconciliationError::Observer)?;
    drop(registry);
    let mut state =
        open_d17_current_only(&root).map_err(|_| ProviderExecReconciliationError::State)?;
    finalize_opencode_observer_ready(
        &mut state,
        &provisional_lease,
        account_context.presentation_directory(),
        &runtime,
        &process_probe,
        &process_probe,
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
    let mut state = open_d17_current_only(&root).map_err(|_| AccountShellGateError::State)?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
        .map_err(|_| AccountShellGateError::State)?;
    let process_probe = LinuxProcessProbe;
    let caller_pid = process::id();
    let caller_group = process_probe
        .process_group_checked(caller_pid)
        .map_err(|_| AccountShellGateError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellGateError::InvocationIdentityUnavailable)?;
    let tmux = SystemTmux::default();
    let clock = SystemD17Clock;
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
        worktree_inspector: &crate::d17_broker::SystemWorktreeInspector,
    };
    let prepared = prepare_managed_shell_gate(&mut state, &provisional_lease, &command, &context)?;
    Ok(AccountShellGateOutcome::Prepared(prepared))
}

/// Replaces the exact provisional shell with its already grammar-normalized
/// native Codex command. All state, marker, process, worktree, clock, and
/// executable evidence is revalidated while holding `provisional.lock`.
///
/// A successful Unix `execve` never returns. If it returns an operating-system
/// error, the exact known-absent journal transition is attempted before the
/// bounded failure reaches the shell. This routine has no CLI route until the
/// atomic D17 Navigator cutover.
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
    let mut state =
        open_d17_current_only(&root).map_err(|_| AccountShellCodexLaunchError::State)?;
    let codex_home = d17_codex_observer_home(&state)?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
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
    let clock = SystemD17Clock;
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
        worktree_inspector: &crate::d17_broker::SystemWorktreeInspector,
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
/// before a dormant D17 helper can consume a Codex launch capability. The
/// record's profile parent, rather than inherited shell state, becomes the
/// explicit `CODEX_HOME` for the final native exec.
fn d17_codex_observer_home(state: &D16State) -> Result<PathBuf, AccountShellCodexLaunchError> {
    let integration = state
        .d17_codex_integration()
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

/// Builds the only D17-managed Codex native command. The closed grammar has
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
        .expect("the D17 native Codex program is constructed from an exact executable");
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
/// A successful Unix `execve` never returns. This routine has no CLI route
/// until the atomic D17 Navigator cutover.
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
    let mut state =
        open_d17_current_only(&root).map_err(|_| AccountShellOpenCodeLaunchError::State)?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
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
        .inspect_containing_worktree(&live.cwd)
        .map_err(|_| AccountShellOpenCodeLaunchError::Helper)?;
    let clock = SystemD17Clock;
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
        .expect("the D17 native program is constructed from an exact executable");
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
    use std::ffi::OsString;
    use std::fs;

    use super::{
        AccountShellGateOutcome, AccountShellOpenCodeLaunchError, ProviderExecReconciliationError,
        codex_observer_program, exec_opencode_from_account_shell, gate_from_account_shell,
        reconcile_provider_exec_from_presentation,
    };
    use crate::{
        d17_account_shell::AccountShellError, d17_reconcile::ExpectedProviderExecutable,
        domain::ProviderKind,
    };

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
