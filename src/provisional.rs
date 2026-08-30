//! presentation-private provisional-slot authority.
//!
//! This module owns only the bounded marker contract for an unregistered
//! candidate Runtime. It composes with the stable host lease, presentation
//! proof, and broker/helper boundaries but cannot itself adopt a managed
//! Runtime.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{OperationId, Revision, RuntimeId},
    presentation::{
        Presentation, ProvisionalInventory, ProvisionalInventoryError,
        classify_provisional_inventory,
    },
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, ProcessGroupProbe, RuntimePaths,
        RuntimeProbe, RuntimeStartup, SystemTmux,
    },
    state::{CurrentState, ProvisionalLease, StateError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SlotGeneration(Uuid);

impl SlotGeneration {
    #[must_use]
    pub(crate) const fn new(value: Uuid) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProvisionalPhase {
    Materializing,
    Materialized,
    HandoffIssued,
    RuntimeOwnedLaunching,
    ProviderExecProven,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(test)]
pub(crate) enum CleanupAuthority {
    ExactProvisional,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProvisionalSlot {
    presentation_id: Uuid,
    presentation_revision: Revision,
    lease_generation: i64,
    candidate_runtime_id: RuntimeId,
    runtime_paths: RuntimePaths,
    seed_cwd: PathBuf,
    slot_generation: SlotGeneration,
    shell_evidence: Option<ProvisionalShellEvidence>,
    phase: ProvisionalPhase,
    handoff_request: Option<Uuid>,
}

#[derive(Debug, Eq, Error, PartialEq)]
pub(crate) enum SlotError {
    #[error("provisional state root is unavailable")]
    StateRootUnavailable,
    #[error("provisional seed cwd is unavailable")]
    SeedCwdUnavailable,
    #[error("provisional seed cwd is not a directory")]
    SeedCwdNotDirectory,
    #[error("provisional lease generation is invalid")]
    InvalidLeaseGeneration,
    #[error("provisional handoff is unavailable")]
    HandoffUnavailable,
    #[error("provisional shell evidence is unavailable")]
    ShellEvidenceUnavailable,
    #[error("provisional shell evidence is invalid")]
    InvalidShellEvidence,
    #[error("provisional shell cwd does not match its seed")]
    ShellCwdMismatch,
    #[error("provisional handoff does not match the slot")]
    HandoffMismatch,
    #[error("provider exec proof is unavailable")]
    ProviderExecProofUnavailable,
    #[error("provisional marker could not be encoded")]
    MarkerEncoding,
    #[error("provisional marker is oversized")]
    MarkerOversized,
    #[error("provisional marker is malformed")]
    MarkerMalformed,
    #[error("provisional marker runtime paths do not match the candidate")]
    MarkerRuntimePathsMismatch,
    #[error("provisional marker parent is unavailable")]
    MarkerParentUnavailable,
    #[error("provisional marker parent is unsafe")]
    MarkerParentUnsafe,
    #[error("provisional marker already exists")]
    MarkerAlreadyExists,
    #[error("provisional marker is unavailable")]
    MarkerUnavailable,
    #[error("provisional marker ownership changed")]
    MarkerOwnershipChanged,
    #[error("provisional marker I/O failed")]
    MarkerIo,
    #[error("provisional marker transition is invalid")]
    MarkerTransitionInvalid,
}

/// Bounded failure from the host-wide provisional-slot classifier. It
/// retains no marker, Runtime path, journal, shell, or provider detail.
#[derive(Debug, Error)]
pub(crate) enum HostInventoryError {
    #[error("provisional state is unavailable")]
    State(#[from] StateError),
    #[error("provisional inventory is unavailable")]
    Inventory(#[from] ProvisionalInventoryError),
}

/// Bounded refusal from retiring one terminal, Runtime-owned provisional
/// marker. The retained onboarding journal and registered Runtime remain the
/// authority; this operation removes only the presentation-private marker and
/// never attaches, signals, parks, or removes the provider Runtime.
#[derive(Debug, Error)]
pub(crate) enum HostRetirementError {
    #[error("completed onboarding state is unavailable")]
    State(#[from] StateError),
    #[error("completed onboarding marker is unavailable")]
    Slot(#[from] SlotError),
    #[error("completed onboarding identity is unavailable")]
    Identity,
}

/// Bounded refusal from the lease-held shell materialization boundary.
/// It intentionally retains neither the presentation directory nor candidate
/// Runtime path, so a caller cannot turn an unavailable or occupied host into
/// a discovery oracle.
#[derive(Debug, Error)]
pub(crate) enum HostMaterializationError {
    #[error("provisional state is unavailable")]
    Inventory(#[from] HostInventoryError),
    #[error("provisional presentation context is unavailable")]
    Presentation,
    #[error("the provisional lease does not match the candidate")]
    Lease,
    #[error("another provisional shell is already materialized")]
    Occupied,
    #[error("the provisional candidate Runtime is already in use")]
    CandidateInUse,
    #[error("provisional materialization is unavailable")]
    Slot(#[from] SlotError),
}

/// Bounded result of startup/close reconciliation for a pre-effect marker.
/// `RuntimeOwned` means durable helper consumption won the race and the marker
/// was repaired without touching the Runtime; `Cleaned` means exact candidate
/// artifacts and any unconsumed graph were removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreHandoffRecovery {
    Unchanged,
    Cleaned,
    RuntimeOwned,
}

/// Bounded refusal from pre-effect provisional recovery.  It intentionally
/// carries no path, process, operation, or provider detail.
#[derive(Debug, Error)]
pub(crate) enum PreHandoffRecoveryError {
    #[error("provisional recovery state is unavailable")]
    State(#[from] StateError),
    #[error("provisional recovery marker is unavailable")]
    Slot(#[from] SlotError),
    #[error("provisional Runtime evidence is unavailable")]
    Runtime,
    #[error("provisional recovery is ambiguous")]
    Ambiguous,
}

const PROVISIONAL_MARKER_VERSION: u8 = 2;
const MAX_PROVISIONAL_MARKER_BYTES: usize = 8 * 1024;
const MAX_SHELL_BIRTH_BYTES: usize = 256;
const MAX_TMUX_PANE_ID_BYTES: usize = 64;
const PROVISIONAL_SHELL_ARTIFACT_NAMES: [&str; 2] = [".wsnav-provisional-bashrc", ".zshrc"];
pub(crate) const PROVISIONAL_MARKER_FILE: &str = "provisional.json";

/// Rebuilds the complete host-wide singleton inventory while retaining the
/// stable provisional lease. It performs no marker, runtime, tmux, process,
/// provider, or filesystem mutation; callers must revalidate the same lease
/// again immediately before any later materialization step.
pub(crate) fn classify_host_inventory(
    state: &CurrentState,
    provisional_lease: &ProvisionalLease,
) -> Result<ProvisionalInventory, HostInventoryError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let registered_runtime_paths = state.registered_runtime_paths()?;
    let operations = state.onboarding_operation_inventory()?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    classify_provisional_inventory(state.root(), &registered_runtime_paths, &operations)
        .map_err(HostInventoryError::from)
}

/// Reconciles a marker that has not crossed the durable Runtime-owned fence.
/// It is safe to call during every startup/reconnect while the caller
/// holds the host-wide provisional lease.  Live or unknown evidence remains
/// untouched; only a missing private tmux server proves that no provisional
/// shell remains to preserve.
pub(crate) fn reconcile_pre_handoff_under_lease(
    state: &mut CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
) -> Result<PreHandoffRecovery, PreHandoffRecoveryError> {
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    reconcile_pre_handoff_with_runtime(
        state,
        provisional_lease,
        presentation_directory,
        &tmux,
        &process_probe,
    )
}

/// Testable seam for [`reconcile_pre_handoff_under_lease`].  The runtime probe
/// is read-only; no process or tmux control occurs on this path.
#[allow(clippy::too_many_lines)]
pub(crate) fn reconcile_pre_handoff_with_runtime(
    state: &mut CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    tmux: &dyn crate::runtime::TmuxClient,
    process_probe: &dyn crate::runtime::ProcessProbe,
) -> Result<PreHandoffRecovery, PreHandoffRecoveryError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let slot = match read_marker(state.root(), presentation_directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return Ok(PreHandoffRecovery::Unchanged),
        Err(error) => return Err(error.into()),
    };
    let context = Presentation::context_from_directory(state.root(), presentation_directory)
        .map_err(|_| PreHandoffRecoveryError::Ambiguous)?;
    if slot.presentation_id() != context.presentation_id()
        || slot.presentation_revision() != context.presentation_revision()
        || slot.seed_cwd() != context.seed_cwd()
        || slot.runtime_paths()
            != &RuntimePaths::for_runtime(state.root(), slot.candidate_runtime_id())
    {
        return Err(PreHandoffRecoveryError::Ambiguous);
    }
    if matches!(
        slot.phase(),
        ProvisionalPhase::RuntimeOwnedLaunching
            | ProvisionalPhase::ProviderExecProven
            | ProvisionalPhase::Cancelled
    ) {
        return Ok(PreHandoffRecovery::Unchanged);
    }

    let runtime = PrivateRuntime::new(tmux, process_probe, slot.runtime_paths().clone());
    let operation = state.onboarding_marker_operation_current(
        provisional_lease,
        slot.presentation_id(),
        slot.presentation_revision(),
        slot.slot_generation(),
        slot.candidate_runtime_id(),
        slot.handoff_request().map(OperationId::from),
    )?;
    if slot.phase() == ProvisionalPhase::Materializing && operation.is_some() {
        return Err(PreHandoffRecoveryError::Ambiguous);
    }

    let registered_candidate = state
        .registered_runtime_paths()?
        .iter()
        .any(|paths| paths == slot.runtime_paths());

    // Durable Runtime ownership wins any marker-write race.  Repair only the
    // marker phase after proving that the exact candidate is registered; the
    // provider/runtime are never stopped or rolled back.
    if slot.phase() == ProvisionalPhase::HandoffIssued
        && operation.is_some_and(|operation| {
            matches!(
                operation.phase,
                crate::domain::OnboardingPhase::RuntimeOwnedLaunching
                    | crate::domain::OnboardingPhase::ProviderPreparation
                    | crate::domain::OnboardingPhase::ProviderExternalEffectStarted
                    | crate::domain::OnboardingPhase::ProviderExecStarted
                    | crate::domain::OnboardingPhase::KnownAbsentExec
                    | crate::domain::OnboardingPhase::RecoveryRequired
                    | crate::domain::OnboardingPhase::ProviderExecProven
            )
        })
    {
        if !registered_candidate {
            return Err(PreHandoffRecoveryError::Ambiguous);
        }
        let operation = operation.ok_or(PreHandoffRecoveryError::Ambiguous)?;
        let mut repaired = slot.clone();
        repaired.consume_handoff(operation.operation_id.as_uuid())?;
        provisional_lease.revalidate_for_mutation(state.root())?;
        update_marker(state.root(), presentation_directory, &slot, &repaired)?;
        provisional_lease.revalidate_for_mutation(state.root())?;
        return Ok(PreHandoffRecovery::RuntimeOwned);
    }

    let exact_pre_effect_operation = operation.is_some_and(|operation| {
        operation.phase == crate::domain::OnboardingPhase::CapabilityIssued
    });
    if registered_candidate && !exact_pre_effect_operation {
        // A managed Runtime sharing this candidate path is never an
        // attempt-only cleanup target, even if the marker appears pre-effect.
        return Err(PreHandoffRecoveryError::Ambiguous);
    }

    let probe = runtime
        .probe()
        .map_err(|_| PreHandoffRecoveryError::Runtime)?;
    if !matches!(probe, RuntimeProbe::Missing) {
        // Live and unknown servers may still contain the account shell or an
        // externally changed process.  Preserve every artifact and wait for a
        // fresh exact presentation/marker decision.
        return Ok(PreHandoffRecovery::Unchanged);
    }

    // A state-first cancellation may have committed just before a marker or
    // candidate-artifact cleanup was interrupted.  The terminal rollback
    // record is exact cleanup evidence, never a reason to reissue/consume.
    if operation
        .is_some_and(|operation| operation.phase == crate::domain::OnboardingPhase::RolledBack)
    {
        provisional_lease.revalidate_for_mutation(state.root())?;
        remove_exact_provisional_artifacts(state.root(), presentation_directory, &slot)?;
        provisional_lease.revalidate_for_mutation(state.root())?;
        return Ok(PreHandoffRecovery::Cleaned);
    }

    // A missing private server is conclusive no-effect evidence for these
    // phases.  Capability cancellation is transactional; marker/artifacts
    // are removed only after that state boundary commits.
    state.cancel_onboarding_current(
        provisional_lease,
        slot.presentation_id(),
        slot.presentation_revision(),
        slot.slot_generation(),
        slot.candidate_runtime_id(),
        slot.runtime_paths(),
        slot.handoff_request().map(OperationId::from),
    )?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    remove_exact_provisional_artifacts(state.root(), presentation_directory, &slot)?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    Ok(PreHandoffRecovery::Cleaned)
}

/// Cancels the exact pre-effect graph during an authoritative presentation
/// close.  Unlike startup reconciliation, this function permits a live
/// candidate only after the caller has independently revalidated its shell
/// lineage; it still never touches the Runtime itself.
pub(crate) fn cancel_pre_handoff_under_lease(
    state: &mut CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
) -> Result<bool, PreHandoffRecoveryError> {
    let context = Presentation::context_from_directory(state.root(), presentation_directory)
        .map_err(|_| PreHandoffRecoveryError::Ambiguous)?;
    if slot.presentation_id() != context.presentation_id()
        || slot.presentation_revision() != context.presentation_revision()
        || slot.seed_cwd() != context.seed_cwd()
        || slot.runtime_paths()
            != &RuntimePaths::for_runtime(state.root(), slot.candidate_runtime_id())
    {
        return Err(PreHandoffRecoveryError::Ambiguous);
    }
    if !matches!(
        slot.phase(),
        ProvisionalPhase::Materializing
            | ProvisionalPhase::Materialized
            | ProvisionalPhase::HandoffIssued
    ) {
        return Err(PreHandoffRecoveryError::Ambiguous);
    }
    let operation = state.onboarding_marker_operation_current(
        provisional_lease,
        slot.presentation_id(),
        slot.presentation_revision(),
        slot.slot_generation(),
        slot.candidate_runtime_id(),
        slot.handoff_request().map(OperationId::from),
    )?;
    if operation.is_none()
        && state
            .registered_runtime_paths()?
            .iter()
            .any(|paths| paths == slot.runtime_paths())
    {
        return Err(PreHandoffRecoveryError::Ambiguous);
    }
    state
        .cancel_onboarding_current(
            provisional_lease,
            slot.presentation_id(),
            slot.presentation_revision(),
            slot.slot_generation(),
            slot.candidate_runtime_id(),
            slot.runtime_paths(),
            slot.handoff_request().map(OperationId::from),
        )
        .map_err(PreHandoffRecoveryError::from)
}

/// Removes only the marker and the three exact private runtime artifacts for
/// an unregistered candidate.  Every directory entry is checked first; an
/// unexpected or changed entry refuses the whole operation.
pub(crate) fn remove_exact_provisional_artifacts(
    state_root: &Path,
    presentation_directory: &Path,
    expected: &ProvisionalSlot,
) -> Result<(), SlotError> {
    if !matches!(
        expected.phase(),
        ProvisionalPhase::Materializing
            | ProvisionalPhase::Materialized
            | ProvisionalPhase::HandoffIssued
            | ProvisionalPhase::Cancelled
    ) {
        return Err(SlotError::MarkerTransitionInvalid);
    }
    let state_root = canonical_state_root(state_root)?;
    if expected.runtime_paths()
        != &RuntimePaths::for_runtime(&state_root, expected.candidate_runtime_id())
    {
        return Err(SlotError::MarkerRuntimePathsMismatch);
    }
    remove_exact_provisional_runtime_artifacts(&state_root, expected)?;
    remove_exact_marker(&state_root, presentation_directory, expected)
}

/// Proves that the candidate Runtime directory contains only the private
/// artifacts created by the provisional owner.  This is deliberately a
/// separate read-only seam so a caller can complete the proof before asking
/// tmux to stop a live provisional server; `park` must never get a chance to
/// recursively remove an unreviewed directory.
pub(crate) fn validate_exact_provisional_runtime_artifacts(
    state_root: &Path,
    expected: &ProvisionalSlot,
) -> Result<(), SlotError> {
    inspect_exact_provisional_runtime_artifacts(state_root, expected).map(|_| ())
}

type RuntimeArtifactEntries = (Option<fs::Metadata>, Vec<(PathBuf, fs::Metadata)>);

/// Removes only the exact candidate Runtime directory, leaving the marker in
/// place for a caller that still needs its marker inode proof (presentation
/// close uses this ordering before removing the outer presentation).
pub(crate) fn remove_exact_provisional_runtime_artifacts(
    state_root: &Path,
    expected: &ProvisionalSlot,
) -> Result<(), SlotError> {
    let (directory_before, entries) =
        inspect_exact_provisional_runtime_artifacts(state_root, expected)?;
    let Some(directory_before) = directory_before else {
        return Ok(());
    };
    for (path, before) in entries {
        let after = fs::symlink_metadata(&path).map_err(|_| SlotError::MarkerOwnershipChanged)?;
        if !same_file_identity(&before, &after) {
            return Err(SlotError::MarkerOwnershipChanged);
        }
        fs::remove_file(path).map_err(map_marker_io)?;
    }
    let directory = &expected.runtime_paths().directory;
    let directory_after = fs::symlink_metadata(directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SlotError::MarkerOwnershipChanged
        } else {
            SlotError::MarkerIo
        }
    })?;
    if !same_file_identity(&directory_before, &directory_after) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let mut remaining = fs::read_dir(directory).map_err(|_| SlotError::MarkerIo)?;
    if remaining
        .next()
        .transpose()
        .map_err(|_| SlotError::MarkerIo)?
        .is_some()
    {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    fs::remove_dir(directory).map_err(map_marker_io)?;
    Ok(())
}

fn inspect_exact_provisional_runtime_artifacts(
    state_root: &Path,
    expected: &ProvisionalSlot,
) -> Result<RuntimeArtifactEntries, SlotError> {
    let state_root = canonical_state_root(state_root)?;
    if expected.runtime_paths()
        != &RuntimePaths::for_runtime(&state_root, expected.candidate_runtime_id())
    {
        return Err(SlotError::MarkerRuntimePathsMismatch);
    }
    let directory = &expected.runtime_paths().directory;
    let directory_metadata = match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((None, Vec::new()));
        }
        Err(_) => return Err(SlotError::MarkerIo),
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !is_private_owner_directory(&metadata) =>
        {
            return Err(SlotError::MarkerOwnershipChanged);
        }
        Ok(metadata) => metadata,
    };
    let expected_paths = [
        expected.runtime_paths().socket.clone(),
        expected.runtime_paths().config.clone(),
        expected.runtime_paths().launch_barrier(),
        expected
            .runtime_paths()
            .directory
            .join(PROVISIONAL_SHELL_ARTIFACT_NAMES[0]),
        expected
            .runtime_paths()
            .directory
            .join(PROVISIONAL_SHELL_ARTIFACT_NAMES[1]),
    ];
    let entries = fs::read_dir(directory)
        .map_err(|_| SlotError::MarkerIo)?
        .map(|entry| {
            let path = entry.map_err(|_| SlotError::MarkerIo)?.path();
            if !expected_paths.iter().any(|expected| expected == &path) {
                return Err(SlotError::MarkerOwnershipChanged);
            }
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| SlotError::MarkerOwnershipChanged)?;
            if metadata.file_type().is_symlink() {
                return Err(SlotError::MarkerOwnershipChanged);
            }
            let is_socket = path == expected.runtime_paths().socket;
            if (is_socket && !is_private_runtime_socket(&metadata))
                || (!is_socket && !is_private_regular_file(&metadata))
            {
                return Err(SlotError::MarkerOwnershipChanged);
            }
            Ok((path, metadata))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((Some(directory_metadata), entries))
}

/// Retires the exact presentation-private marker after durable onboarding has
/// reached `provider_exec_proven`. The stable host lease spans journal proof,
/// marker identity proof, unlink, and directory durability. The adopted
/// Runtime and terminal operation remain untouched.
pub(crate) fn retire_provider_exec_proven_marker(
    state: &CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    expected: &ProvisionalSlot,
) -> Result<(), HostRetirementError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    expected.validate_lifecycle()?;
    if expected.phase() != ProvisionalPhase::ProviderExecProven
        || expected.runtime_paths()
            != &RuntimePaths::for_runtime(state.root(), expected.candidate_runtime_id())
    {
        return Err(HostRetirementError::Identity);
    }
    let operation_id = expected
        .handoff_request()
        .map(OperationId::from)
        .ok_or(HostRetirementError::Identity)?;
    let target = state.onboarding_exec_proven_target_current(provisional_lease, operation_id)?;
    if target.ownership().operation_id != operation_id
        || target.ownership().runtime_id != expected.candidate_runtime_id()
    {
        return Err(HostRetirementError::Identity);
    }
    if read_marker(state.root(), presentation_directory)? != *expected {
        return Err(HostRetirementError::Identity);
    }
    provisional_lease.revalidate_for_mutation(state.root())?;
    remove_exact_marker(state.root(), presentation_directory, expected)?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    Ok(())
}

/// Proves that this exact unregistered candidate may create its first private
/// artifact while the caller retains the host-wide provisional lease. This
/// performs no marker, runtime, tmux, process, provider, or state mutation.
///
/// The raw materializer is deliberately test-only. Production callers must
/// pass through this proof and revalidate the same lease immediately after
/// their marker-first materialization attempt.
pub(crate) fn validate_fresh_host_materialization(
    state: &CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
) -> Result<(), HostMaterializationError> {
    let context = Presentation::context_from_directory(state.root(), presentation_directory)
        .map_err(|_| HostMaterializationError::Presentation)?;
    if context.presentation_id() != slot.presentation_id()
        || context.presentation_revision() != slot.presentation_revision()
        || context.seed_cwd() != slot.seed_cwd
    {
        return Err(HostMaterializationError::Presentation);
    }
    if provisional_lease.lease_generation() != slot.lease_generation() {
        return Err(HostMaterializationError::Lease);
    }

    match classify_host_inventory(state, provisional_lease)? {
        ProvisionalInventory::Vacant => {}
        ProvisionalInventory::Occupied => return Err(HostMaterializationError::Occupied),
    }
    revalidate_host_materialization_lease(state, provisional_lease)?;

    let registered_runtime_paths = state
        .registered_runtime_paths()
        .map_err(HostInventoryError::from)?;
    if registered_runtime_paths
        .iter()
        .any(|paths| paths == slot.runtime_paths())
    {
        return Err(HostMaterializationError::CandidateInUse);
    }
    revalidate_host_materialization_lease(state, provisional_lease)?;

    match fs::symlink_metadata(&slot.runtime_paths().directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) | Ok(_) => return Err(HostMaterializationError::CandidateInUse),
    }
    revalidate_host_materialization_lease(state, provisional_lease)
}

/// Lease-held materializer that first writes a fixed account-shell startup
/// plan for this exact private Runtime.
#[allow(
    clippy::too_many_arguments,
    reason = "the exact lease, presentation, Runtime, launch, startup, and process evidence must remain visible at one materialization fence"
)]
pub(crate) fn materialize_private_shell_with_startup_under_lease(
    state: &CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
    runtime: &PrivateRuntime<'_>,
    launch: &NativeLaunch,
    startup: &dyn RuntimeStartup,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<ProvisionalSlot, HostMaterializationError> {
    materialize_private_shell_under_lease_inner(
        state,
        provisional_lease,
        presentation_directory,
        slot,
        runtime,
        launch,
        Some(startup),
        process_group_probe,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the exact lease, presentation, Runtime, launch, startup, and process evidence must remain visible at one materialization fence"
)]
fn materialize_private_shell_under_lease_inner(
    state: &CurrentState,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
    runtime: &PrivateRuntime<'_>,
    launch: &NativeLaunch,
    startup: Option<&dyn RuntimeStartup>,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<ProvisionalSlot, HostMaterializationError> {
    validate_fresh_host_materialization(state, provisional_lease, presentation_directory, slot)?;
    let materialized = materialize_private_shell_inner(
        state.root(),
        presentation_directory,
        slot,
        runtime,
        launch,
        startup,
        process_group_probe,
    );
    revalidate_host_materialization_lease(state, provisional_lease)?;
    Ok(materialized?)
}

fn revalidate_host_materialization_lease(
    state: &CurrentState,
    provisional_lease: &ProvisionalLease,
) -> Result<(), HostMaterializationError> {
    provisional_lease
        .revalidate_for_mutation(state.root())
        .map_err(HostInventoryError::from)
        .map_err(HostMaterializationError::from)
}

/// Exact private-pane/process evidence that binds a materialized provisional
/// shell to the marker's final tmux path set. The server's socket/config/session
/// remain in [`RuntimePaths`]; this structure supplies the live pane and shell
/// lineage required before a broker may issue a handoff.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvisionalShellEvidence {
    pane_id: String,
    shell_pid: u32,
    shell_birth: String,
    shell_process_group: u32,
    shell_session: u32,
}

/// Freshly observed private-shell evidence. This is transient broker input;
/// it is compared against the marker before any durable onboarding mutation
/// and is never persisted separately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveProvisionalShell {
    pub(crate) cwd: PathBuf,
    pub(crate) pane_id: String,
    pub(crate) shell_pid: u32,
    pub(crate) shell_birth: String,
    pub(crate) shell_process_group: u32,
    pub(crate) shell_session: u32,
}

impl ProvisionalShellEvidence {
    fn new(
        pane_id: String,
        shell_pid: u32,
        shell_birth: String,
        shell_process_group: u32,
        shell_session: u32,
    ) -> Result<Self, SlotError> {
        let evidence = Self {
            pane_id,
            shell_pid,
            shell_birth,
            shell_process_group,
            shell_session,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    fn validate(&self) -> Result<(), SlotError> {
        if self.shell_pid == 0
            || self.shell_process_group == 0
            || self.shell_session == 0
            || !is_tmux_pane_id(&self.pane_id)
            || !is_bounded_process_birth(&self.shell_birth)
        {
            return Err(SlotError::InvalidShellEvidence);
        }
        Ok(())
    }
}

/// Presentation-private evidence for one unregistered materialized candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProvisionalMarker {
    version: u8,
    presentation_id: Uuid,
    presentation_revision: Revision,
    lease_generation: i64,
    candidate_runtime_id: RuntimeId,
    directory: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    session_name: String,
    seed_cwd: PathBuf,
    slot_generation: Uuid,
    shell_evidence: Option<ProvisionalShellEvidence>,
    phase: ProvisionalPhase,
    handoff_request: Option<Uuid>,
}

impl ProvisionalMarker {
    fn from_slot(slot: &ProvisionalSlot) -> Self {
        Self {
            version: PROVISIONAL_MARKER_VERSION,
            presentation_id: slot.presentation_id,
            presentation_revision: slot.presentation_revision,
            lease_generation: slot.lease_generation,
            candidate_runtime_id: slot.candidate_runtime_id,
            directory: slot.runtime_paths.directory.clone(),
            socket: slot.runtime_paths.socket.clone(),
            config: slot.runtime_paths.config.clone(),
            session_name: slot.runtime_paths.session_name.clone(),
            seed_cwd: slot.seed_cwd.clone(),
            slot_generation: slot.slot_generation.0,
            shell_evidence: slot.shell_evidence.clone(),
            phase: slot.phase,
            handoff_request: slot.handoff_request,
        }
    }

    fn encode(&self) -> Result<Vec<u8>, SlotError> {
        let bytes = serde_json::to_vec(self).map_err(|_| SlotError::MarkerEncoding)?;
        if bytes.len() > MAX_PROVISIONAL_MARKER_BYTES {
            return Err(SlotError::MarkerOversized);
        }
        Ok(bytes)
    }

    fn decode(state_root: &Path, bytes: &[u8]) -> Result<ProvisionalSlot, SlotError> {
        if bytes.len() > MAX_PROVISIONAL_MARKER_BYTES {
            return Err(SlotError::MarkerOversized);
        }
        let marker =
            serde_json::from_slice::<Self>(bytes).map_err(|_| SlotError::MarkerMalformed)?;
        if marker.version != PROVISIONAL_MARKER_VERSION {
            return Err(SlotError::MarkerMalformed);
        }
        let mut slot = ProvisionalSlot::materializing(
            state_root,
            marker.presentation_id,
            marker.presentation_revision,
            marker.lease_generation,
            marker.candidate_runtime_id,
            SlotGeneration(marker.slot_generation),
            &marker.seed_cwd,
        )?;
        if slot.runtime_paths.directory != marker.directory
            || slot.runtime_paths.socket != marker.socket
            || slot.runtime_paths.config != marker.config
            || slot.runtime_paths.session_name != marker.session_name
        {
            return Err(SlotError::MarkerRuntimePathsMismatch);
        }
        slot.shell_evidence = marker.shell_evidence;
        slot.phase = marker.phase;
        slot.handoff_request = marker.handoff_request;
        slot.validate_lifecycle()?;
        Ok(slot)
    }
}

/// Writes one new presentation-private marker for an exact unregistered slot.
/// The marker is create-new, no-follow, private, fsynced, and root-bound. A
/// pre-existing or changed artifact is never overwritten or adopted.
pub(crate) fn write_new_marker(
    state_root: &Path,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
) -> Result<PathBuf, SlotError> {
    slot.validate_lifecycle()?;
    let marker_path = marker_path(state_root, presentation_directory, slot)?;
    let bytes = ProvisionalMarker::from_slot(slot).encode()?;
    let mut file = open_new_private_marker(&marker_path)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        return Err(map_marker_io(error));
    }
    validate_marker_file_matches_path(&file, &marker_path)?;
    sync_directory(presentation_directory)?;
    Ok(marker_path)
}

/// Reads and validates the one exact marker below an already-owned
/// presentation directory. The returned slot remains unregistered; callers
/// must still hold the host provisional lease and corroborate live tmux/process
/// evidence before any mutation.
pub(crate) fn read_marker(
    state_root: &Path,
    presentation_directory: &Path,
) -> Result<ProvisionalSlot, SlotError> {
    let state_root = canonical_state_root(state_root)?;
    let presentation_directory =
        canonical_presentation_directory(&state_root, presentation_directory)?;
    let marker_path = presentation_directory.join(PROVISIONAL_MARKER_FILE);
    let before = fs::symlink_metadata(&marker_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SlotError::MarkerUnavailable
        } else {
            map_marker_io(error)
        }
    })?;
    if !is_private_regular_file(&before) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let file = open_existing_private_marker(&marker_path)?;
    let opened = file.metadata().map_err(map_marker_io)?;
    if !is_private_regular_file(&opened) || !same_file_identity(&before, &opened) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let mut bytes = Vec::new();
    file.take((MAX_PROVISIONAL_MARKER_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(map_marker_io)?;
    if bytes.len() > MAX_PROVISIONAL_MARKER_BYTES {
        return Err(SlotError::MarkerOversized);
    }
    let after = fs::symlink_metadata(&marker_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SlotError::MarkerOwnershipChanged
        } else {
            map_marker_io(error)
        }
    })?;
    if !is_private_regular_file(&after) || !same_file_identity(&opened, &after) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    ProvisionalMarker::decode(&state_root, &bytes)
}

/// Persists one legal provisional-slot phase change while preserving the
/// marker inode. The caller must hold the stable provisional lease; a crash
/// during the in-place replacement leaves malformed evidence that fails closed
/// rather than making a new marker authoritative.
pub(crate) fn update_marker(
    state_root: &Path,
    presentation_directory: &Path,
    expected: &ProvisionalSlot,
    next: &ProvisionalSlot,
) -> Result<(), SlotError> {
    let state_root = canonical_state_root(state_root)?;
    let presentation_directory =
        canonical_presentation_directory(&state_root, presentation_directory)?;
    expected.validate_lifecycle()?;
    ensure_same_slot_identity(expected, next)?;
    expected.phase.transition(next.phase)?;
    let marker_path = presentation_directory.join(PROVISIONAL_MARKER_FILE);
    let before = fs::symlink_metadata(&marker_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SlotError::MarkerUnavailable
        } else {
            map_marker_io(error)
        }
    })?;
    if !is_private_regular_file(&before) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(&marker_path).map_err(map_marker_io)?;
    let opened = file.metadata().map_err(map_marker_io)?;
    if !is_private_regular_file(&opened) || !same_file_identity(&before, &opened) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let persisted = read_marker_from_open_file(&mut file, &state_root)?;
    if persisted != *expected {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let bytes = ProvisionalMarker::from_slot(next).encode()?;
    file.set_len(0).map_err(map_marker_io)?;
    file.seek(SeekFrom::Start(0)).map_err(map_marker_io)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(map_marker_io)?;
    validate_marker_file_matches_path(&file, &marker_path)?;
    sync_directory(&presentation_directory)
}

fn remove_exact_marker(
    state_root: &Path,
    presentation_directory: &Path,
    expected: &ProvisionalSlot,
) -> Result<(), SlotError> {
    let state_root = canonical_state_root(state_root)?;
    let presentation_directory =
        canonical_presentation_directory(&state_root, presentation_directory)?;
    expected.validate_lifecycle()?;
    let marker_path = marker_path(&state_root, &presentation_directory, expected)?;
    let before = fs::symlink_metadata(&marker_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SlotError::MarkerUnavailable
        } else {
            map_marker_io(error)
        }
    })?;
    if !is_private_regular_file(&before) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    let mut file = open_existing_private_marker(&marker_path)?;
    let opened = file.metadata().map_err(map_marker_io)?;
    if !is_private_regular_file(&opened) || !same_file_identity(&before, &opened) {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    if read_marker_from_open_file(&mut file, &state_root)? != *expected {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    validate_marker_file_matches_path(&file, &marker_path)?;
    fs::remove_file(&marker_path).map_err(map_marker_io)?;
    match fs::symlink_metadata(&marker_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(map_marker_io(error)),
        Ok(_) => return Err(SlotError::MarkerOwnershipChanged),
    }
    sync_directory(&presentation_directory)
}

/// Writes a pre-server marker, creates one exact private shell Runtime, and
/// records the resulting live pane/process lineage before the slot can be
/// handed to the broker. Every failure after marker creation deliberately
/// leaves the materializing marker in place for conservative reconciliation;
/// this seam never removes, adopts, attaches, or signals an artifact.
#[cfg(test)]
pub(crate) fn materialize_private_shell(
    state_root: &Path,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
    runtime: &PrivateRuntime<'_>,
    launch: &NativeLaunch,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<ProvisionalSlot, SlotError> {
    materialize_private_shell_inner(
        state_root,
        presentation_directory,
        slot,
        runtime,
        launch,
        None,
        process_group_probe,
    )
}

/// Materializes a provisional shell only after a startup plan bound to this
/// exact candidate Runtime has written its private artifacts. This test-only
/// entrypoint exercises the same inner boundary without host state.
#[cfg(test)]
pub(crate) fn materialize_private_shell_with_startup(
    state_root: &Path,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
    runtime: &PrivateRuntime<'_>,
    launch: &NativeLaunch,
    startup: &dyn RuntimeStartup,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<ProvisionalSlot, SlotError> {
    materialize_private_shell_inner(
        state_root,
        presentation_directory,
        slot,
        runtime,
        launch,
        Some(startup),
        process_group_probe,
    )
}

fn materialize_private_shell_inner(
    state_root: &Path,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
    runtime: &PrivateRuntime<'_>,
    launch: &NativeLaunch,
    startup: Option<&dyn RuntimeStartup>,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<ProvisionalSlot, SlotError> {
    if slot.phase != ProvisionalPhase::Materializing || slot.shell_evidence.is_some() {
        return Err(SlotError::ShellEvidenceUnavailable);
    }
    if runtime.paths() != &slot.runtime_paths {
        return Err(SlotError::MarkerRuntimePathsMismatch);
    }
    let launch_cwd = fs::canonicalize(&launch.cwd).map_err(|_| SlotError::ShellCwdMismatch)?;
    if launch_cwd != slot.seed_cwd {
        return Err(SlotError::ShellCwdMismatch);
    }
    write_new_marker(state_root, presentation_directory, slot)?;
    match startup {
        Some(startup) => runtime.start_with_startup(launch, startup),
        None => runtime.start(launch),
    }
    .map_err(|_| SlotError::ShellEvidenceUnavailable)?;
    let observed = observe_live_shell(runtime, process_group_probe)?;
    if observed.cwd != slot.seed_cwd {
        return Err(SlotError::ShellCwdMismatch);
    }
    let shell_evidence = ProvisionalShellEvidence::new(
        observed.pane_id,
        observed.shell_pid,
        observed.shell_birth,
        observed.shell_process_group,
        observed.shell_session,
    )?;
    let mut materialized = slot.clone();
    materialized.record_shell_evidence(shell_evidence)?;
    update_marker(state_root, presentation_directory, slot, &materialized)?;
    Ok(materialized)
}

fn observe_live_shell(
    runtime: &PrivateRuntime<'_>,
    process_group_probe: &dyn ProcessGroupProbe,
) -> Result<LiveProvisionalShell, SlotError> {
    let RuntimeProbe::Live {
        pane_id,
        pane_pid,
        cwd,
        process_birth: Some(shell_birth),
    } = runtime
        .probe()
        .map_err(|_| SlotError::ShellEvidenceUnavailable)?
    else {
        return Err(SlotError::ShellEvidenceUnavailable);
    };
    let cwd = fs::canonicalize(cwd).map_err(|_| SlotError::ShellEvidenceUnavailable)?;
    let Some(group) = process_group_probe
        .process_group_checked(pane_pid)
        .map_err(|_| SlotError::ShellEvidenceUnavailable)?
    else {
        return Err(SlotError::ShellEvidenceUnavailable);
    };
    ProvisionalShellEvidence::new(
        pane_id.clone(),
        pane_pid,
        shell_birth.clone(),
        group.process_group_id,
        group.session_id,
    )?;
    Ok(LiveProvisionalShell {
        cwd,
        pane_id,
        shell_pid: pane_pid,
        shell_birth,
        shell_process_group: group.process_group_id,
        shell_session: group.session_id,
    })
}

fn marker_path(
    state_root: &Path,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
) -> Result<PathBuf, SlotError> {
    let state_root = canonical_state_root(state_root)?;
    let presentation_directory =
        canonical_presentation_directory(&state_root, presentation_directory)?;
    if slot.runtime_paths != RuntimePaths::for_runtime(&state_root, slot.candidate_runtime_id) {
        return Err(SlotError::MarkerRuntimePathsMismatch);
    }
    Ok(presentation_directory.join(PROVISIONAL_MARKER_FILE))
}

fn canonical_state_root(state_root: &Path) -> Result<PathBuf, SlotError> {
    let state_root = fs::canonicalize(state_root).map_err(|_| SlotError::StateRootUnavailable)?;
    if !state_root.is_dir() {
        return Err(SlotError::StateRootUnavailable);
    }
    Ok(state_root)
}

fn canonical_presentation_directory(
    state_root: &Path,
    presentation_directory: &Path,
) -> Result<PathBuf, SlotError> {
    let original_metadata = fs::symlink_metadata(presentation_directory)
        .map_err(|_| SlotError::MarkerParentUnavailable)?;
    if !original_metadata.is_dir() || original_metadata.file_type().is_symlink() {
        return Err(SlotError::MarkerParentUnsafe);
    }
    let presentation_directory =
        fs::canonicalize(presentation_directory).map_err(|_| SlotError::MarkerParentUnavailable)?;
    let metadata = fs::symlink_metadata(&presentation_directory).map_err(map_marker_io)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || presentation_directory == state_root
        || !presentation_directory.starts_with(state_root)
    {
        return Err(SlotError::MarkerParentUnsafe);
    }
    Ok(presentation_directory)
}

fn open_new_private_marker(path: &Path) -> Result<File, SlotError> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(SlotError::MarkerAlreadyExists);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            SlotError::MarkerAlreadyExists
        } else {
            map_marker_io(error)
        }
    })?;
    set_private_marker_permissions(&file)?;
    validate_marker_file_matches_path(&file, path)?;
    Ok(file)
}

fn open_existing_private_marker(path: &Path) -> Result<File, SlotError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SlotError::MarkerUnavailable
        } else {
            map_marker_io(error)
        }
    })
}

fn read_marker_from_open_file(
    file: &mut File,
    state_root: &Path,
) -> Result<ProvisionalSlot, SlotError> {
    file.seek(SeekFrom::Start(0)).map_err(map_marker_io)?;
    let mut bytes = Vec::new();
    file.take((MAX_PROVISIONAL_MARKER_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(map_marker_io)?;
    if bytes.len() > MAX_PROVISIONAL_MARKER_BYTES {
        return Err(SlotError::MarkerOversized);
    }
    ProvisionalMarker::decode(state_root, &bytes)
}

fn set_private_marker_permissions(file: &File) -> Result<(), SlotError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(map_marker_io)?;
    }
    Ok(())
}

fn validate_marker_file_matches_path(file: &File, path: &Path) -> Result<(), SlotError> {
    let opened = file.metadata().map_err(map_marker_io)?;
    let path_metadata = fs::symlink_metadata(path).map_err(map_marker_io)?;
    if !is_private_regular_file(&opened)
        || !is_private_regular_file(&path_metadata)
        || !same_file_identity(&opened, &path_metadata)
    {
        return Err(SlotError::MarkerOwnershipChanged);
    }
    Ok(())
}

fn is_private_regular_file(metadata: &fs::Metadata) -> bool {
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.uid() == nix::unistd::geteuid().as_raw() && metadata.mode() & 0o777 == 0o600
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_private_owner_directory(metadata: &fs::Metadata) -> bool {
    if !metadata.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.uid() == nix::unistd::geteuid().as_raw() && metadata.mode() & 0o777 == 0o700
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_private_runtime_socket(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::fs::MetadataExt;
        metadata.file_type().is_socket()
            && metadata.uid() == nix::unistd::geteuid().as_raw()
            && matches!(metadata.mode() & 0o777, 0o600 | 0o700)
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn is_tmux_pane_id(value: &str) -> bool {
    let Some(identifier) = value.strip_prefix('%') else {
        return false;
    };
    !identifier.is_empty()
        && identifier.len() <= MAX_TMUX_PANE_ID_BYTES
        && identifier.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_bounded_process_birth(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SHELL_BIRTH_BYTES
        && value
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace())
}

fn same_file_identity(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        first.dev() == second.dev() && first.ino() == second.ino()
    }
    #[cfg(not(unix))]
    {
        first.len() == second.len() && first.modified().ok() == second.modified().ok()
    }
}

fn sync_directory(path: &Path) -> Result<(), SlotError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(map_marker_io)
}

fn map_marker_io(_error: std::io::Error) -> SlotError {
    SlotError::MarkerIo
}

impl ProvisionalSlot {
    /// Creates the pre-server marker state. The caller must persist this
    /// exact marker before attempting private tmux creation so a crash leaves
    /// conservative materialization evidence rather than an adoptable server.
    pub(crate) fn materializing(
        state_root: &Path,
        presentation_id: Uuid,
        presentation_revision: Revision,
        lease_generation: i64,
        candidate_runtime_id: RuntimeId,
        slot_generation: SlotGeneration,
        seed_cwd: &Path,
    ) -> Result<Self, SlotError> {
        if lease_generation <= 0 {
            return Err(SlotError::InvalidLeaseGeneration);
        }
        let state_root =
            fs::canonicalize(state_root).map_err(|_| SlotError::StateRootUnavailable)?;
        let seed_cwd = fs::canonicalize(seed_cwd).map_err(|_| SlotError::SeedCwdUnavailable)?;
        if !seed_cwd.is_dir() {
            return Err(SlotError::SeedCwdNotDirectory);
        }
        Ok(Self {
            presentation_id,
            presentation_revision,
            lease_generation,
            candidate_runtime_id,
            runtime_paths: RuntimePaths::for_runtime(&state_root, candidate_runtime_id),
            seed_cwd,
            slot_generation,
            shell_evidence: None,
            phase: ProvisionalPhase::Materializing,
            handoff_request: None,
        })
    }

    #[must_use]
    pub(crate) const fn presentation_id(&self) -> Uuid {
        self.presentation_id
    }

    #[must_use]
    pub(crate) const fn presentation_revision(&self) -> Revision {
        self.presentation_revision
    }

    #[must_use]
    pub(crate) const fn lease_generation(&self) -> i64 {
        self.lease_generation
    }

    #[must_use]
    pub(crate) const fn candidate_runtime_id(&self) -> RuntimeId {
        self.candidate_runtime_id
    }

    #[must_use]
    pub(crate) fn runtime_paths(&self) -> &RuntimePaths {
        &self.runtime_paths
    }

    /// Returns the canonical presentation seed bound to this exact
    /// provisional candidate. It remains marker-private and is exposed only
    /// for equality checks at authority fences.
    #[must_use]
    pub(crate) fn seed_cwd(&self) -> &Path {
        &self.seed_cwd
    }

    #[must_use]
    pub(crate) const fn slot_generation(&self) -> Uuid {
        self.slot_generation.0
    }

    #[must_use]
    pub(crate) const fn phase(&self) -> ProvisionalPhase {
        self.phase
    }

    #[must_use]
    pub(crate) const fn handoff_request(&self) -> Option<Uuid> {
        self.handoff_request
    }

    /// Repeats the exact private pane/process comparison required before a
    /// broker or helper may use this marker. Current cwd is deliberately live
    /// evidence rather than a stored cwd history.
    pub(crate) fn revalidate_live_shell(
        &self,
        runtime: &PrivateRuntime<'_>,
        process_group_probe: &dyn ProcessGroupProbe,
    ) -> Result<LiveProvisionalShell, SlotError> {
        if !matches!(
            self.phase,
            ProvisionalPhase::Materialized
                | ProvisionalPhase::HandoffIssued
                | ProvisionalPhase::RuntimeOwnedLaunching
        ) || runtime.paths() != &self.runtime_paths
        {
            return Err(SlotError::ShellEvidenceUnavailable);
        }
        let expected = self
            .shell_evidence
            .as_ref()
            .ok_or(SlotError::ShellEvidenceUnavailable)?;
        let observed = observe_live_shell(runtime, process_group_probe)?;
        if expected.pane_id != observed.pane_id
            || expected.shell_pid != observed.shell_pid
            || expected.shell_birth != observed.shell_birth
            || expected.shell_process_group != observed.shell_process_group
            || expected.shell_session != observed.shell_session
        {
            return Err(SlotError::ShellEvidenceUnavailable);
        }
        Ok(observed)
    }

    #[cfg(test)]
    const fn cleanup_authority(&self) -> CleanupAuthority {
        match self.phase {
            ProvisionalPhase::Materializing
            | ProvisionalPhase::Materialized
            | ProvisionalPhase::HandoffIssued => CleanupAuthority::ExactProvisional,
            ProvisionalPhase::RuntimeOwnedLaunching
            | ProvisionalPhase::ProviderExecProven
            | ProvisionalPhase::Cancelled => CleanupAuthority::None,
        }
    }

    #[cfg(test)]
    const fn action_allowed(&self) -> bool {
        matches!(self.phase, ProvisionalPhase::ProviderExecProven)
    }

    fn validate_lifecycle(&self) -> Result<(), SlotError> {
        if let Some(evidence) = &self.shell_evidence {
            evidence.validate()?;
        }
        let expected = match self.phase {
            ProvisionalPhase::Materializing => (false, false),
            ProvisionalPhase::Materialized => (true, false),
            ProvisionalPhase::HandoffIssued
            | ProvisionalPhase::RuntimeOwnedLaunching
            | ProvisionalPhase::ProviderExecProven
            | ProvisionalPhase::Cancelled => (true, true),
        };
        if expected.0 != self.shell_evidence.is_some()
            || expected.1 != self.handoff_request.is_some()
        {
            return Err(SlotError::MarkerTransitionInvalid);
        }
        Ok(())
    }

    fn record_shell_evidence(
        &mut self,
        shell_evidence: ProvisionalShellEvidence,
    ) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::Materializing || self.shell_evidence.is_some() {
            return Err(SlotError::ShellEvidenceUnavailable);
        }
        shell_evidence.validate()?;
        self.shell_evidence = Some(shell_evidence);
        self.phase = ProvisionalPhase::Materialized;
        Ok(())
    }

    pub(crate) fn issue_handoff(&mut self, request: Uuid) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::Materialized
            || self.shell_evidence.is_none()
            || self.handoff_request.is_some()
        {
            return Err(SlotError::HandoffUnavailable);
        }
        self.handoff_request = Some(request);
        self.phase = ProvisionalPhase::HandoffIssued;
        Ok(())
    }

    #[cfg(test)]
    fn cancel_unconsumed(&mut self, request: Uuid) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::HandoffIssued {
            return Err(SlotError::HandoffUnavailable);
        }
        if self.handoff_request != Some(request) {
            return Err(SlotError::HandoffMismatch);
        }
        self.phase = ProvisionalPhase::Cancelled;
        Ok(())
    }

    pub(crate) fn consume_handoff(&mut self, request: Uuid) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::HandoffIssued {
            return Err(SlotError::HandoffUnavailable);
        }
        if self.handoff_request != Some(request) {
            return Err(SlotError::HandoffMismatch);
        }
        self.phase = ProvisionalPhase::RuntimeOwnedLaunching;
        Ok(())
    }

    pub(crate) fn prove_provider_exec(&mut self) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::RuntimeOwnedLaunching {
            return Err(SlotError::ProviderExecProofUnavailable);
        }
        self.phase = ProvisionalPhase::ProviderExecProven;
        Ok(())
    }
}

impl ProvisionalPhase {
    fn transition(self, next: Self) -> Result<(), SlotError> {
        if matches!(
            (self, next),
            (Self::Materializing, Self::Materialized)
                | (Self::Materialized, Self::HandoffIssued)
                | (
                    Self::HandoffIssued,
                    Self::RuntimeOwnedLaunching | Self::Cancelled
                )
                | (Self::RuntimeOwnedLaunching, Self::ProviderExecProven)
        ) {
            Ok(())
        } else {
            Err(SlotError::MarkerTransitionInvalid)
        }
    }
}

fn ensure_same_slot_identity(
    expected: &ProvisionalSlot,
    next: &ProvisionalSlot,
) -> Result<(), SlotError> {
    if expected.presentation_id != next.presentation_id
        || expected.presentation_revision != next.presentation_revision
        || expected.lease_generation != next.lease_generation
        || expected.candidate_runtime_id != next.candidate_runtime_id
        || expected.runtime_paths != next.runtime_paths
        || expected.seed_cwd != next.seed_cwd
        || expected.slot_generation != next.slot_generation
        || (expected.phase != ProvisionalPhase::Materializing
            && expected.shell_evidence != next.shell_evidence)
    {
        return Err(SlotError::MarkerTransitionInvalid);
    }
    next.validate_lifecycle()
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::{BTreeMap, VecDeque},
        ffi::OsString,
        fs,
        path::PathBuf,
        str::FromStr,
        sync::atomic::{AtomicU64, Ordering},
    };

    use uuid::Uuid;

    use super::{
        CleanupAuthority, PROVISIONAL_MARKER_FILE, PreHandoffRecoveryError, ProvisionalMarker,
        ProvisionalPhase, ProvisionalShellEvidence, ProvisionalSlot, SlotError, SlotGeneration,
        materialize_private_shell, materialize_private_shell_with_startup, read_marker,
        reconcile_pre_handoff_with_runtime, update_marker, write_new_marker,
    };
    use crate::{
        domain::{IdGenerator, ProviderKind, Revision, RuntimeId},
        onboarding::{ShellCommandDecision, classify_shell_command},
        presentation::{Presentation, PresentationPaths, create_paths_for_test},
        repository::RepositoryDiscovery,
        runtime::{
            NativeLaunch, PrivateRuntime, ProcessGroupInfo, ProcessGroupProbe, ProcessProbe,
            ProcessProbeError, RuntimeError, RuntimePaths, RuntimeStartup, TmuxClient,
            TmuxInvocation, TmuxResponse,
        },
        state::{
            CurrentState, ProvisionalLease, create_current,
            current::{
                OnboardingPreparation, OnboardingPrepareRequest, onboarding::OnboardingReservation,
            },
        },
    };

    #[derive(Default)]
    struct FakeTmux {
        calls: RefCell<Vec<TmuxInvocation>>,
    }

    impl TmuxClient for FakeTmux {
        fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
            self.calls.borrow_mut().push(invocation.clone());
            Ok(TmuxResponse {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    struct FakeProcessProbe;

    impl ProcessProbe for FakeProcessProbe {
        fn process_birth(&self, _pid: u32) -> Option<String> {
            None
        }
    }

    struct ShellProcessProbe;

    impl ProcessProbe for ShellProcessProbe {
        fn process_birth(&self, pid: u32) -> Option<String> {
            (pid == 4242).then(|| "birth-4242".to_owned())
        }
    }

    struct ShellGroupProbe;

    impl ProcessGroupProbe for ShellGroupProbe {
        fn process_group_checked(
            &self,
            pid: u32,
        ) -> Result<Option<ProcessGroupInfo>, ProcessProbeError> {
            Ok((pid == 4242).then_some(ProcessGroupInfo {
                process_group_id: 4242,
                session_id: 31337,
            }))
        }

        fn process_group_members_checked(
            &self,
            _group: &ProcessGroupInfo,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            Ok(vec![4242])
        }

        fn process_group_members_by_id_checked(
            &self,
            _process_group_id: u32,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            Ok(vec![4242])
        }
    }

    struct PrivateShellStartup;

    impl RuntimeStartup for PrivateShellStartup {
        fn prepare(&self, paths: &RuntimePaths) -> Result<(), RuntimeError> {
            let bootstrap = paths.directory.join("startup-proof");
            fs::write(&bootstrap, b"prepared").map_err(|source| RuntimeError::Io {
                path: bootstrap,
                source,
            })
        }
    }

    struct MaterializationTmux {
        calls: RefCell<Vec<TmuxInvocation>>,
        responses: RefCell<VecDeque<TmuxResponse>>,
        marker_path: PathBuf,
        startup_path: Option<PathBuf>,
    }

    impl MaterializationTmux {
        fn new(marker_path: PathBuf, responses: impl IntoIterator<Item = TmuxResponse>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(responses.into_iter().collect()),
                marker_path,
                startup_path: None,
            }
        }

        fn with_startup(
            marker_path: PathBuf,
            startup_path: PathBuf,
            responses: impl IntoIterator<Item = TmuxResponse>,
        ) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(responses.into_iter().collect()),
                marker_path,
                startup_path: Some(startup_path),
            }
        }
    }

    impl TmuxClient for MaterializationTmux {
        fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
            if self.calls.borrow().is_empty() {
                assert!(
                    self.marker_path.is_file(),
                    "the marker must be durable before the private server starts"
                );
                if let Some(startup_path) = &self.startup_path {
                    assert!(
                        startup_path.is_file(),
                        "the private startup must finish before the private server starts"
                    );
                }
            }
            self.calls.borrow_mut().push(invocation.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| RuntimeError::TmuxRejected("unexpected tmux call".to_owned()))
        }
    }

    fn shell_evidence() -> ProvisionalShellEvidence {
        ProvisionalShellEvidence::new("%17".to_owned(), 4242, "birth-4242".to_owned(), 4242, 31337)
            .unwrap()
    }

    fn fixture() -> (tempfile::TempDir, ProvisionalSlot) {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&state_root).unwrap();
        fs::create_dir(&seed).unwrap();
        let candidate_runtime_id =
            RuntimeId::from_str("01234567-0000-0000-0000-000000000001").unwrap();
        let mut slot = ProvisionalSlot::materializing(
            &state_root,
            Uuid::parse_str("01234567-0000-0000-0000-000000000002").unwrap(),
            Revision::INITIAL,
            1,
            candidate_runtime_id,
            SlotGeneration(Uuid::parse_str("01234567-0000-0000-0000-000000000003").unwrap()),
            &seed,
        )
        .unwrap();
        slot.record_shell_evidence(shell_evidence()).unwrap();
        (temporary, slot)
    }

    fn materializing_fixture() -> (tempfile::TempDir, ProvisionalSlot) {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&state_root).unwrap();
        fs::create_dir(&seed).unwrap();
        let slot = ProvisionalSlot::materializing(
            &state_root,
            Uuid::parse_str("01234567-0000-0000-0000-000000000012").unwrap(),
            Revision::INITIAL,
            1,
            RuntimeId::from_str("01234567-0000-0000-0000-000000000011").unwrap(),
            SlotGeneration(Uuid::parse_str("01234567-0000-0000-0000-000000000013").unwrap()),
            &seed,
        )
        .unwrap();
        (temporary, slot)
    }

    #[derive(Default)]
    struct RecoveryIds(AtomicU64);

    impl IdGenerator for RecoveryIds {
        fn uuid(&self) -> Uuid {
            Uuid::from_u128(0x9000 + u128::from(self.0.fetch_add(1, Ordering::Relaxed)))
        }
    }

    struct RecoveryFixture {
        temporary: tempfile::TempDir,
        state_path: PathBuf,
        presentation_directory: PathBuf,
        state: CurrentState,
        provisional_lease: ProvisionalLease,
        slot: ProvisionalSlot,
        request: Option<OnboardingPrepareRequest>,
        reservation: Option<OnboardingReservation>,
    }

    fn recovery_request(
        state_path: &std::path::Path,
        presentation_id: Uuid,
        presentation_revision: Revision,
        slot_generation: Uuid,
        candidate_runtime_id: RuntimeId,
    ) -> OnboardingPrepareRequest {
        let worktree_root = state_path.join("recovery-worktree");
        fs::create_dir(&worktree_root).unwrap();
        let shell_cwd = worktree_root.join("nested");
        fs::create_dir(&shell_cwd).unwrap();
        let arguments = [OsString::from("--model"), OsString::from("gpt-5.6")];
        let ShellCommandDecision::ManagedFresh(launch) =
            classify_shell_command(ProviderKind::Codex, &arguments).unwrap()
        else {
            panic!("fixture command must be promotable");
        };
        OnboardingPrepareRequest {
            request_key: format!("recovery-request-{candidate_runtime_id}"),
            presentation_id,
            presentation_revision,
            slot_generation,
            candidate_runtime_id,
            runtime_paths: RuntimePaths::for_runtime(state_path, candidate_runtime_id),
            provider: ProviderKind::Codex,
            repository: RepositoryDiscovery {
                project_root: worktree_root,
                display_name: "recovery-worktree".to_owned(),
                remote_identity_fingerprint: Some(format!("git-remote-v1:{}", "a".repeat(64))),
                remote_identity_display: Some("github.com/example/recovery".to_owned()),
            },
            shell_cwd,
            shell_pid: 710,
            shell_birth: "birth-710".to_owned(),
            shell_process_group: 710,
            shell_session: 710,
            argv_digest: launch.argv_digest().to_owned(),
            boot_provenance: format!("wsnav-boot-v1:sha256:{}", "b".repeat(64)),
            now_monotonic_millis: 10,
            expiry_monotonic_millis: 1_010,
        }
    }

    fn recovery_fixture(materialized: bool, handoff: bool) -> RecoveryFixture {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let mut state = create_current(&state_path, &RecoveryIds::default()).unwrap();
        let provisional_lease = state.acquire_provisional_lease().unwrap();
        let presentation_directory = state_path
            .join("presentation")
            .join("presentation-000000000001");
        let presentation_paths = PresentationPaths {
            directory: presentation_directory.clone(),
            socket: presentation_directory.join("tmux.sock"),
            config: presentation_directory.join("tmux.conf"),
            attachment_status: presentation_directory.join("attachment.json"),
            session_name: "wsnav-presentation-000000000001".to_owned(),
        };
        create_paths_for_test(&presentation_paths).unwrap();
        // Reuse the deterministic paths above rather than the random helper
        // used by production owners; the context marker still proves the
        // exact directory/session relationship.
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        let presentation = Presentation::from_control(
            &state_path,
            presentation_paths.socket.clone(),
            presentation_paths.session_name.clone(),
        )
        .unwrap();
        let presentation_id = Uuid::from_u128(0x1700);
        let context = presentation
            .initialize_context(presentation_id, &seed)
            .unwrap();
        let candidate_runtime_id = RuntimeId::from(Uuid::from_u128(0x9000));
        let slot_generation = Uuid::from_u128(0x1701);
        let mut slot = ProvisionalSlot::materializing(
            &state_path,
            presentation_id,
            context.presentation_revision(),
            provisional_lease.lease_generation(),
            candidate_runtime_id,
            SlotGeneration::new(slot_generation),
            &seed,
        )
        .unwrap();
        if materialized {
            slot.record_shell_evidence(shell_evidence()).unwrap();
        }
        let (request, reservation) = if handoff {
            let request = recovery_request(
                &state_path,
                presentation_id,
                context.presentation_revision(),
                slot_generation,
                candidate_runtime_id,
            );
            let reservation = match state
                .prepare_onboarding_current(&provisional_lease, &request, &RecoveryIds::default())
                .unwrap()
            {
                OnboardingPreparation::Issued(reservation) => reservation,
                OnboardingPreparation::Existing(_) => panic!("fixture must issue once"),
            };
            slot.issue_handoff(reservation.operation_id().as_uuid())
                .unwrap();
            (Some(request), Some(reservation))
        } else {
            (None, None)
        };
        write_new_marker(&state_path, &presentation_directory, &slot).unwrap();
        RecoveryFixture {
            temporary,
            state_path,
            presentation_directory,
            state,
            provisional_lease,
            slot,
            request,
            reservation,
        }
    }

    #[derive(Default)]
    struct RecoveryTmux {
        calls: RefCell<Vec<TmuxInvocation>>,
        responses: RefCell<VecDeque<TmuxResponse>>,
    }

    impl RecoveryTmux {
        fn with_responses(responses: impl IntoIterator<Item = TmuxResponse>) -> Self {
            Self {
                calls: RefCell::default(),
                responses: RefCell::new(responses.into_iter().collect()),
            }
        }
    }

    impl TmuxClient for RecoveryTmux {
        fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
            self.calls.borrow_mut().push(invocation.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| RuntimeError::TmuxRejected("unexpected probe call".to_owned()))
        }
    }

    struct RecoveryProcessProbe;

    impl ProcessProbe for RecoveryProcessProbe {
        fn process_birth(&self, _pid: u32) -> Option<String> {
            None
        }
    }

    fn no_server_probe() -> RecoveryTmux {
        RecoveryTmux::with_responses([TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "no server running on the private socket".to_owned(),
        }])
    }

    fn unknown_server_probe() -> RecoveryTmux {
        RecoveryTmux::with_responses([TmuxResponse {
            success: false,
            stdout: String::new(),
            stderr: "can't find session: private".to_owned(),
        }])
    }

    fn live_server_probe(seed: &std::path::Path) -> RecoveryTmux {
        RecoveryTmux::with_responses([
            TmuxResponse {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: "%17\n".to_owned(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: "4242\n".to_owned(),
                stderr: String::new(),
            },
            TmuxResponse {
                success: true,
                stdout: format!("{}\n", seed.display()),
                stderr: String::new(),
            },
        ])
    }

    #[cfg(unix)]
    fn exact_runtime_artifacts(slot: &ProvisionalSlot) -> std::os::unix::net::UnixListener {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(&slot.runtime_paths().directory).unwrap();
        fs::set_permissions(
            slot.runtime_paths().directory.as_path(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let listener =
            std::os::unix::net::UnixListener::bind(slot.runtime_paths().socket.as_path()).unwrap();
        fs::set_permissions(
            slot.runtime_paths().socket.as_path(),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        for path in [
            slot.runtime_paths().config.clone(),
            slot.runtime_paths().launch_barrier(),
            slot.runtime_paths()
                .directory
                .join(".wsnav-provisional-bashrc"),
            slot.runtime_paths().directory.join(".zshrc"),
        ] {
            fs::write(&path, b"fixture").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        listener
    }

    #[test]
    fn recovery_cleans_missing_server_materializing_candidate() {
        let mut fixture = recovery_fixture(false, false);
        let tmux = no_server_probe();
        let result = reconcile_pre_handoff_with_runtime(
            &mut fixture.state,
            &fixture.provisional_lease,
            &fixture.presentation_directory,
            &tmux,
            &RecoveryProcessProbe,
        )
        .unwrap();

        assert_eq!(result, super::PreHandoffRecovery::Cleaned);
        assert!(
            !fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );
        assert!(!fixture.slot.runtime_paths().directory.exists());
    }

    #[test]
    #[cfg(unix)]
    fn recovery_cleans_missing_server_materialized_candidate_artifacts() {
        let mut fixture = recovery_fixture(true, false);
        let _socket = exact_runtime_artifacts(&fixture.slot);
        let tmux = no_server_probe();
        let result = reconcile_pre_handoff_with_runtime(
            &mut fixture.state,
            &fixture.provisional_lease,
            &fixture.presentation_directory,
            &tmux,
            &RecoveryProcessProbe,
        )
        .unwrap();

        assert_eq!(result, super::PreHandoffRecovery::Cleaned);
        assert!(
            !fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );
        assert!(!fixture.slot.runtime_paths().directory.exists());
    }

    #[test]
    #[cfg(unix)]
    fn recovery_cancels_handoff_graph_cleans_artifacts_and_rejects_replay() {
        let mut fixture = recovery_fixture(true, true);
        let _socket = exact_runtime_artifacts(&fixture.slot);
        let token = fixture
            .reservation
            .as_ref()
            .unwrap()
            .capability()
            .token()
            .to_owned();
        let tmux = no_server_probe();
        let result = reconcile_pre_handoff_with_runtime(
            &mut fixture.state,
            &fixture.provisional_lease,
            &fixture.presentation_directory,
            &tmux,
            &RecoveryProcessProbe,
        )
        .unwrap();

        assert_eq!(result, super::PreHandoffRecovery::Cleaned);
        assert!(
            !fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );
        assert!(!fixture.slot.runtime_paths().directory.exists());
        let operation_id = fixture.reservation.as_ref().unwrap().operation_id();
        assert_eq!(
            fixture
                .state
                .onboarding_operation_inventory()
                .unwrap()
                .into_iter()
                .find(|operation| operation.operation_id == operation_id)
                .map(|operation| operation.phase),
            Some(crate::domain::OnboardingPhase::RolledBack)
        );
        let request = fixture.request.as_ref().unwrap();
        assert!(matches!(
            fixture.state.consume_onboarding_current(
                &fixture.provisional_lease,
                request,
                &token,
                request.now_monotonic_millis + 1,
            ),
            Err(crate::state::StateError::OnboardingOperationUnavailable)
        ));
    }

    #[test]
    fn recovery_removes_interrupted_state_first_rollback_on_restart() {
        let mut fixture = recovery_fixture(true, true);
        let operation_id = fixture.reservation.as_ref().unwrap().operation_id();
        let request = fixture.request.as_ref().unwrap();
        fixture
            .state
            .cancel_onboarding_current(
                &fixture.provisional_lease,
                request.presentation_id,
                request.presentation_revision,
                request.slot_generation,
                request.candidate_runtime_id,
                &request.runtime_paths,
                Some(operation_id),
            )
            .unwrap();
        let tmux = no_server_probe();
        let result = reconcile_pre_handoff_with_runtime(
            &mut fixture.state,
            &fixture.provisional_lease,
            &fixture.presentation_directory,
            &tmux,
            &RecoveryProcessProbe,
        )
        .unwrap();

        assert_eq!(result, super::PreHandoffRecovery::Cleaned);
        assert!(
            !fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );
    }

    #[test]
    fn recovery_repairs_runtime_owned_marker_without_stopping_or_deleting() {
        let mut fixture = recovery_fixture(true, true);
        let request = fixture.request.as_ref().unwrap();
        let reservation = fixture.reservation.as_ref().unwrap();
        fixture
            .state
            .consume_onboarding_current(
                &fixture.provisional_lease,
                request,
                reservation.capability().token(),
                request.now_monotonic_millis + 1,
            )
            .unwrap();
        let tmux = no_server_probe();
        let result = reconcile_pre_handoff_with_runtime(
            &mut fixture.state,
            &fixture.provisional_lease,
            &fixture.presentation_directory,
            &tmux,
            &RecoveryProcessProbe,
        )
        .unwrap();

        assert_eq!(result, super::PreHandoffRecovery::RuntimeOwned);
        assert_eq!(
            read_marker(&fixture.state_path, &fixture.presentation_directory)
                .unwrap()
                .phase(),
            ProvisionalPhase::RuntimeOwnedLaunching
        );
        assert!(!fixture.slot.runtime_paths().directory.exists());
        assert_eq!(tmux.calls.borrow().len(), 0);
        assert_eq!(
            fixture.state.onboarding_operation_inventory().unwrap(),
            vec![crate::state::current::OnboardingOperationInventory {
                operation_id: reservation.operation_id(),
                workstream_id: fixture.reservation.as_ref().unwrap().workstream_id(),
                runtime_id: fixture.slot.candidate_runtime_id(),
                phase: crate::domain::OnboardingPhase::RuntimeOwnedLaunching,
            }]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn recovery_preserves_live_unknown_foreign_and_changed_evidence() {
        let mut live_fixture = recovery_fixture(true, false);
        let _socket = {
            #[cfg(unix)]
            {
                exact_runtime_artifacts(&live_fixture.slot)
            }
        };
        let tmux = live_server_probe(&live_fixture.slot.seed_cwd);
        assert_eq!(
            reconcile_pre_handoff_with_runtime(
                &mut live_fixture.state,
                &live_fixture.provisional_lease,
                &live_fixture.presentation_directory,
                &tmux,
                &RecoveryProcessProbe,
            )
            .unwrap(),
            super::PreHandoffRecovery::Unchanged
        );
        assert!(
            live_fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );

        let mut unknown_fixture = recovery_fixture(true, false);
        let _socket = {
            #[cfg(unix)]
            {
                exact_runtime_artifacts(&unknown_fixture.slot)
            }
        };
        let tmux = unknown_server_probe();
        assert_eq!(
            reconcile_pre_handoff_with_runtime(
                &mut unknown_fixture.state,
                &unknown_fixture.provisional_lease,
                &unknown_fixture.presentation_directory,
                &tmux,
                &RecoveryProcessProbe,
            )
            .unwrap(),
            super::PreHandoffRecovery::Unchanged
        );
        assert!(
            unknown_fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );

        let mut foreign_fixture = recovery_fixture(true, false);
        fs::create_dir_all(&foreign_fixture.slot.runtime_paths().directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &foreign_fixture.slot.runtime_paths().directory,
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        fs::write(
            foreign_fixture
                .slot
                .runtime_paths()
                .directory
                .join("foreign-artifact"),
            b"foreign",
        )
        .unwrap();
        let tmux = no_server_probe();
        assert!(matches!(
            reconcile_pre_handoff_with_runtime(
                &mut foreign_fixture.state,
                &foreign_fixture.provisional_lease,
                &foreign_fixture.presentation_directory,
                &tmux,
                &RecoveryProcessProbe,
            ),
            Err(PreHandoffRecoveryError::Slot(
                SlotError::MarkerOwnershipChanged
            ))
        ));
        assert!(
            foreign_fixture
                .presentation_directory
                .join(PROVISIONAL_MARKER_FILE)
                .exists()
        );
        assert!(
            foreign_fixture
                .slot
                .runtime_paths()
                .directory
                .join("foreign-artifact")
                .exists()
        );

        let mut changed_fixture = recovery_fixture(true, false);
        let marker_path = changed_fixture
            .presentation_directory
            .join(PROVISIONAL_MARKER_FILE);
        let mut marker: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
        marker["seed_cwd"] = serde_json::Value::String(
            changed_fixture
                .temporary
                .path()
                .join("changed-seed")
                .to_string_lossy()
                .into_owned(),
        );
        fs::create_dir(changed_fixture.temporary.path().join("changed-seed")).unwrap();
        fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
        let tmux = no_server_probe();
        assert!(matches!(
            reconcile_pre_handoff_with_runtime(
                &mut changed_fixture.state,
                &changed_fixture.provisional_lease,
                &changed_fixture.presentation_directory,
                &tmux,
                &RecoveryProcessProbe,
            ),
            Err(PreHandoffRecoveryError::Ambiguous)
        ));
        assert!(marker_path.exists());
    }

    #[test]
    fn materialization_binds_the_exact_final_runtime_paths_and_canonical_seed() {
        let (temporary, slot) = fixture();
        assert_eq!(
            slot.runtime_paths,
            RuntimePaths::for_runtime(
                temporary.path().join("state").as_path(),
                slot.candidate_runtime_id,
            )
        );
        assert_eq!(
            slot.runtime_paths.session_name,
            "wsnav-01234567-0000-0000-0000-000000000001"
        );
        assert_eq!(
            slot.seed_cwd,
            temporary.path().join("seed").canonicalize().unwrap()
        );
        assert_eq!(slot.phase, ProvisionalPhase::Materialized);
        assert_eq!(slot.shell_evidence, Some(shell_evidence()));
        assert_eq!(slot.cleanup_authority(), CleanupAuthority::ExactProvisional);
        assert!(!slot.action_allowed());
    }

    #[test]
    fn cleanup_can_cancel_only_the_exact_unconsumed_handoff() {
        let (_temporary, mut slot) = fixture();
        let request = Uuid::parse_str("01234567-0000-0000-0000-000000000004").unwrap();
        slot.issue_handoff(request).unwrap();
        assert_eq!(
            slot.cancel_unconsumed(Uuid::new_v4()),
            Err(SlotError::HandoffMismatch)
        );
        slot.cancel_unconsumed(request).unwrap();
        assert_eq!(slot.phase, ProvisionalPhase::Cancelled);
        assert_eq!(slot.cleanup_authority(), CleanupAuthority::None);
        assert_eq!(
            slot.consume_handoff(request),
            Err(SlotError::HandoffUnavailable)
        );
    }

    #[test]
    fn ownership_consume_removes_cleanup_authority_and_fences_actions_until_exec_proof() {
        let (_temporary, mut slot) = fixture();
        let request = Uuid::parse_str("01234567-0000-0000-0000-000000000004").unwrap();
        slot.issue_handoff(request).unwrap();
        slot.consume_handoff(request).unwrap();
        assert_eq!(slot.cleanup_authority(), CleanupAuthority::None);
        assert!(!slot.action_allowed());
        assert_eq!(
            slot.cancel_unconsumed(request),
            Err(SlotError::HandoffUnavailable)
        );
        slot.prove_provider_exec().unwrap();
        assert!(slot.action_allowed());
    }

    #[test]
    fn provisional_shell_uses_the_exact_final_private_runtime_path_set() {
        let (_temporary, slot) = fixture();
        let tmux = FakeTmux::default();
        let process_probe = FakeProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths.clone());
        let launch = NativeLaunch {
            cwd: slot.seed_cwd.clone(),
            program: vec![OsString::from("synthetic-provisional-shell")],
            environment: BTreeMap::new(),
        };

        runtime.start(&launch).unwrap();

        assert!(slot.runtime_paths.directory.is_dir());
        assert!(slot.runtime_paths.config.is_file());
        let calls = tmux.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].socket, slot.runtime_paths.socket);
        assert!(
            calls[0]
                .arguments
                .contains(&OsString::from("synthetic-provisional-shell"))
        );
    }

    #[test]
    fn materializer_persists_pre_server_marker_then_exact_shell_evidence() {
        let (temporary, slot) = materializing_fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        fs::create_dir(&presentation).unwrap();
        let marker_path = presentation.join(PROVISIONAL_MARKER_FILE);
        let tmux = MaterializationTmux::new(
            marker_path.clone(),
            [
                TmuxResponse {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: "%17\n".to_owned(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: "4242\n".to_owned(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: format!("{}\n", slot.seed_cwd.display()),
                    stderr: String::new(),
                },
            ],
        );
        let process_probe = ShellProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths.clone());
        let launch = NativeLaunch {
            cwd: slot.seed_cwd.clone(),
            program: vec![OsString::from("synthetic-provisional-shell")],
            environment: BTreeMap::new(),
        };

        let materialized = materialize_private_shell(
            &state_root,
            &presentation,
            &slot,
            &runtime,
            &launch,
            &ShellGroupProbe,
        )
        .unwrap();

        assert_eq!(materialized.phase, ProvisionalPhase::Materialized);
        assert_eq!(materialized.shell_evidence, Some(shell_evidence()));
        assert_eq!(
            read_marker(&state_root, &presentation).unwrap(),
            materialized
        );
        assert_eq!(tmux.calls.borrow().len(), 5);
    }

    #[test]
    fn materializer_runs_the_bound_private_startup_after_marker_and_before_tmux() {
        let (temporary, slot) = materializing_fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        fs::create_dir(&presentation).unwrap();
        let startup_path = slot.runtime_paths.directory.join("startup-proof");
        let tmux = MaterializationTmux::with_startup(
            presentation.join(PROVISIONAL_MARKER_FILE),
            startup_path.clone(),
            [
                TmuxResponse {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: "%17\n".to_owned(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: "4242\n".to_owned(),
                    stderr: String::new(),
                },
                TmuxResponse {
                    success: true,
                    stdout: format!("{}\n", slot.seed_cwd.display()),
                    stderr: String::new(),
                },
            ],
        );
        let process_probe = ShellProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths.clone());
        let launch = NativeLaunch {
            cwd: slot.seed_cwd.clone(),
            program: vec![OsString::from("synthetic-provisional-shell")],
            environment: BTreeMap::new(),
        };

        let materialized = materialize_private_shell_with_startup(
            &state_root,
            &presentation,
            &slot,
            &runtime,
            &launch,
            &PrivateShellStartup,
            &ShellGroupProbe,
        )
        .unwrap();

        assert_eq!(materialized.phase, ProvisionalPhase::Materialized);
        assert_eq!(fs::read(startup_path).unwrap(), b"prepared");
        assert_eq!(tmux.calls.borrow().len(), 5);
    }

    #[test]
    fn materializer_retains_pre_server_marker_when_private_server_start_fails() {
        let (temporary, slot) = materializing_fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        fs::create_dir(&presentation).unwrap();
        let tmux = MaterializationTmux::new(
            presentation.join(PROVISIONAL_MARKER_FILE),
            [TmuxResponse {
                success: false,
                stdout: String::new(),
                stderr: "synthetic refusal".to_owned(),
            }],
        );
        let process_probe = ShellProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths.clone());
        let launch = NativeLaunch {
            cwd: slot.seed_cwd.clone(),
            program: vec![OsString::from("synthetic-provisional-shell")],
            environment: BTreeMap::new(),
        };

        assert_eq!(
            materialize_private_shell(
                &state_root,
                &presentation,
                &slot,
                &runtime,
                &launch,
                &ShellGroupProbe,
            ),
            Err(SlotError::ShellEvidenceUnavailable)
        );
        assert_eq!(read_marker(&state_root, &presentation).unwrap(), slot);
        assert_eq!(slot.cleanup_authority(), CleanupAuthority::ExactProvisional);
    }

    #[test]
    fn materializer_refuses_a_changed_launch_cwd_before_marker_or_server_creation() {
        let (temporary, slot) = materializing_fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        let other_cwd = temporary.path().join("other");
        fs::create_dir(&presentation).unwrap();
        fs::create_dir(&other_cwd).unwrap();
        let tmux = MaterializationTmux::new(
            presentation.join(PROVISIONAL_MARKER_FILE),
            std::iter::empty(),
        );
        let process_probe = ShellProcessProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths.clone());
        let launch = NativeLaunch {
            cwd: other_cwd,
            program: vec![OsString::from("synthetic-provisional-shell")],
            environment: BTreeMap::new(),
        };

        assert_eq!(
            materialize_private_shell(
                &state_root,
                &presentation,
                &slot,
                &runtime,
                &launch,
                &ShellGroupProbe,
            ),
            Err(SlotError::ShellCwdMismatch)
        );
        assert!(!presentation.join(PROVISIONAL_MARKER_FILE).exists());
        assert!(!slot.runtime_paths.directory.exists());
        assert!(tmux.calls.borrow().is_empty());
    }

    #[test]
    fn unavailable_state_root_refuses_before_a_candidate_claim_exists() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            ProvisionalSlot::materializing(
                &temporary.path().join("missing-state"),
                Uuid::new_v4(),
                Revision::INITIAL,
                1,
                RuntimeId::new(),
                SlotGeneration(Uuid::new_v4()),
                &temporary.path().join("missing-seed"),
            ),
            Err(SlotError::StateRootUnavailable)
        );
    }

    #[test]
    fn marker_round_trip_binds_every_final_runtime_path_and_rejects_path_tampering() {
        let (temporary, slot) = fixture();
        let marker = ProvisionalMarker::from_slot(&slot);
        let bytes = marker.encode().unwrap();
        assert_eq!(
            ProvisionalMarker::decode(temporary.path().join("state").as_path(), &bytes).unwrap(),
            slot
        );

        let mut altered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        altered["socket"] = serde_json::Value::String("elsewhere".to_owned());
        let altered = serde_json::to_vec(&altered).unwrap();
        assert_eq!(
            ProvisionalMarker::decode(temporary.path().join("state").as_path(), &altered),
            Err(SlotError::MarkerRuntimePathsMismatch)
        );

        let mut altered: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        altered["shell_evidence"]["pane_id"] = serde_json::Value::String("foreign".to_owned());
        let altered = serde_json::to_vec(&altered).unwrap();
        assert_eq!(
            ProvisionalMarker::decode(temporary.path().join("state").as_path(), &altered),
            Err(SlotError::InvalidShellEvidence)
        );
    }

    #[test]
    fn marker_storage_is_private_root_bound_and_never_adopts_a_replacement() {
        let (temporary, slot) = fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        fs::create_dir(&presentation).unwrap();

        let marker = write_new_marker(&state_root, &presentation, &slot).unwrap();
        assert_eq!(marker, presentation.join(PROVISIONAL_MARKER_FILE));
        assert_eq!(read_marker(&state_root, &presentation).unwrap(), slot);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&marker).unwrap().mode() & 0o777, 0o600);
        }
        assert_eq!(
            write_new_marker(&state_root, &presentation, &slot),
            Err(SlotError::MarkerAlreadyExists)
        );
        assert_eq!(
            write_new_marker(temporary.path(), &presentation, &slot),
            Err(SlotError::MarkerRuntimePathsMismatch)
        );

        fs::write(&marker, b"not-a-marker").unwrap();
        assert_eq!(
            read_marker(&state_root, &presentation),
            Err(SlotError::MarkerMalformed)
        );
    }

    #[test]
    fn marker_storage_persists_only_one_legal_handoff_lifecycle() {
        let (temporary, slot) = fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        fs::create_dir(&presentation).unwrap();
        write_new_marker(&state_root, &presentation, &slot).unwrap();

        let request = Uuid::parse_str("01234567-0000-0000-0000-000000000004").unwrap();
        let mut issued = slot.clone();
        issued.issue_handoff(request).unwrap();
        update_marker(&state_root, &presentation, &slot, &issued).unwrap();
        assert_eq!(read_marker(&state_root, &presentation).unwrap(), issued);

        let mut owned = issued.clone();
        owned.consume_handoff(request).unwrap();
        update_marker(&state_root, &presentation, &issued, &owned).unwrap();
        assert_eq!(read_marker(&state_root, &presentation).unwrap(), owned);
        assert_eq!(
            update_marker(&state_root, &presentation, &slot, &issued),
            Err(SlotError::MarkerOwnershipChanged)
        );
        assert_eq!(
            update_marker(&state_root, &presentation, &owned, &issued),
            Err(SlotError::MarkerTransitionInvalid)
        );
    }

    #[cfg(unix)]
    #[test]
    fn marker_storage_refuses_a_symlink_before_reading_its_target() {
        use std::os::unix::fs::symlink;

        let (temporary, _slot) = fixture();
        let state_root = temporary.path().join("state");
        let presentation = state_root.join("presentation");
        fs::create_dir(&presentation).unwrap();
        let target = temporary.path().join("foreign-marker");
        fs::write(&target, b"foreign").unwrap();
        symlink(&target, presentation.join(PROVISIONAL_MARKER_FILE)).unwrap();

        assert_eq!(
            read_marker(&state_root, &presentation),
            Err(SlotError::MarkerOwnershipChanged)
        );
    }
}
