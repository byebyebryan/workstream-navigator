//! Dormant D17 post-exec reconciliation boundary.
//!
//! The reconciler can only turn an already recorded `provider_exec_started`
//! attempt into `provider_exec_proven`, or finish the marker half of an
//! already durable proof; it never launches, signals, attaches, or otherwise
//! controls a provider.

#![allow(
    dead_code,
    reason = "the D17 reconciler remains unreachable until the atomic Navigator cutover"
)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    domain::{OperationId, ProviderKind},
    provisional::{ProvisionalPhase, ProvisionalSlot, SlotError, read_marker, update_marker},
    runtime::{PrivateRuntime, ProcessGroupProbe},
    state::d16::{OnboardingProviderExecEvidence, OnboardingProviderExecTarget},
    state::{D16State, ProvisionalLease, StateError},
};

/// Exact native executable expected after the helper's final `execve`. The
/// path is private proof input and is neither persisted nor returned in a
/// public snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpectedProviderExecutable {
    provider: ProviderKind,
    canonical_path: PathBuf,
}

impl ExpectedProviderExecutable {
    pub(crate) fn new(provider: ProviderKind, path: &Path) -> Result<Self, ReconcileError> {
        let canonical_path =
            fs::canonicalize(path).map_err(|_| ReconcileError::ExecutableUnavailable)?;
        let metadata =
            fs::metadata(&canonical_path).map_err(|_| ReconcileError::ExecutableUnavailable)?;
        if !metadata.is_file() {
            return Err(ReconcileError::ExecutableUnavailable);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(ReconcileError::ExecutableUnavailable);
            }
        }
        Ok(Self {
            provider,
            canonical_path,
        })
    }
}

/// Read-only process-executable evidence. A missing process is distinct from
/// an inaccessible or malformed process and neither can prove provider exec.
pub(crate) trait ProviderExecutableProbe {
    fn executable_for_pid(&self, pid: u32) -> Result<Option<PathBuf>, ReconcileError>;
}

/// Bounded reconciliation errors. They intentionally never render private
/// paths, command lines, shell state, tokens, or provider output.
#[derive(Debug, Error)]
pub(crate) enum ReconcileError {
    #[error("D17 provisional slot evidence is unavailable")]
    Slot(#[from] SlotError),
    #[error("D17 provider-exec state is unavailable")]
    State(#[from] StateError),
    #[error("the expected native provider executable is unavailable")]
    ExecutableUnavailable,
    #[error("the exact provisional slot is not ready for provider-exec proof")]
    SlotNotReady,
    #[error("the provisional handoff identity is unavailable")]
    HandoffIdentityUnavailable,
    #[error("the provider identity does not match the reserved D17 Runtime")]
    ProviderIdentityMismatch,
    #[error("the native provider cwd does not match its registered D17 worktree root")]
    ProviderCwdMismatch,
    #[error("the provider executable does not match the expected native executable")]
    ProviderExecutableMismatch,
}

/// Commits exact post-exec proof for one already Runtime-owned provisional
/// slot. The caller supplies only a pre-resolved expected executable and a
/// read-only executable probe; all marker, pane/process-group, state revision,
/// Runtime generation, cwd, and provider checks are repeated here. If the
/// durable proof committed before a marker-write failure, this instead repairs
/// the exact marker phase without re-executing or probing the provider.
pub(crate) fn prove_provider_exec(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    runtime: &PrivateRuntime<'_>,
    process_group_probe: &dyn ProcessGroupProbe,
    expected_executable: &ExpectedProviderExecutable,
    executable_probe: &dyn ProviderExecutableProbe,
) -> Result<(), ReconcileError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let slot = read_marker(state.root(), presentation_directory)?;
    let operation_id = slot
        .handoff_request()
        .map(OperationId::from)
        .ok_or(ReconcileError::HandoffIdentityUnavailable)?;
    if slot.phase() == ProvisionalPhase::ProviderExecProven {
        let target =
            state.d17_onboarding_exec_proven_target_current(provisional_lease, operation_id)?;
        validate_slot_target(&slot, &target, expected_executable)?;
        return Ok(());
    }
    if slot.phase() != ProvisionalPhase::RuntimeOwnedLaunching {
        return Err(ReconcileError::SlotNotReady);
    }
    match state.d17_onboarding_exec_proven_target_current(provisional_lease, operation_id) {
        Ok(target) => {
            validate_slot_target(&slot, &target, expected_executable)?;
            return complete_proven_marker(state, provisional_lease, presentation_directory, &slot);
        }
        Err(StateError::OnboardingOperationUnavailable) => {}
        Err(error) => return Err(error.into()),
    }
    let live = slot.revalidate_live_shell(runtime, process_group_probe)?;
    let target = state.d17_onboarding_exec_proof_target_current(provisional_lease, operation_id)?;
    validate_slot_target(&slot, &target, expected_executable)?;
    if target.project_root() != live.cwd {
        return Err(ReconcileError::ProviderCwdMismatch);
    }
    let actual = executable_probe
        .executable_for_pid(live.shell_pid)?
        .ok_or(ReconcileError::ProviderExecutableMismatch)?;
    let actual =
        fs::canonicalize(actual).map_err(|_| ReconcileError::ProviderExecutableMismatch)?;
    if actual != expected_executable.canonical_path {
        return Err(ReconcileError::ProviderExecutableMismatch);
    }
    let evidence = OnboardingProviderExecEvidence::new(live.shell_pid, live.shell_birth)?;
    state.record_d17_provider_exec_proven_current(
        provisional_lease,
        target.ownership(),
        &evidence,
    )?;
    complete_proven_marker(state, provisional_lease, presentation_directory, &slot)
}

fn validate_slot_target(
    slot: &ProvisionalSlot,
    target: &OnboardingProviderExecTarget,
    expected_executable: &ExpectedProviderExecutable,
) -> Result<(), ReconcileError> {
    if target.ownership().runtime_id != slot.candidate_runtime_id()
        || target.provider() != expected_executable.provider
    {
        return Err(ReconcileError::ProviderIdentityMismatch);
    }
    Ok(())
}

fn complete_proven_marker(
    state: &D16State,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
) -> Result<(), ReconcileError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let mut proven_slot = slot.clone();
    proven_slot.prove_provider_exec()?;
    update_marker(state.root(), presentation_directory, slot, &proven_slot)?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    Ok(())
}
