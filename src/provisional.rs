//! D17 test-only provisional ownership model.
//!
//! The production marker, lease, broker, and helper will consume this lifecycle
//! contract only at the later atomic cutover. Keeping it test-only now proves
//! that D16 has no hidden dependency on D17 onboarding behavior.

use std::{
    fs,
    path::{Path, PathBuf},
};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{Revision, RuntimeId},
    runtime::RuntimePaths,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SlotGeneration(Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProvisionalPhase {
    Materialized,
    HandoffIssued,
    RuntimeOwnedLaunching,
    ProviderExecProven,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanupAuthority {
    ExactProvisional,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProvisionalSlot {
    presentation_id: Uuid,
    presentation_revision: Revision,
    candidate_runtime_id: RuntimeId,
    runtime_paths: RuntimePaths,
    seed_cwd: PathBuf,
    slot_generation: SlotGeneration,
    phase: ProvisionalPhase,
    handoff_request: Option<Uuid>,
}

#[derive(Debug, Eq, Error, PartialEq)]
enum SlotError {
    #[error("provisional state root is unavailable")]
    StateRootUnavailable,
    #[error("provisional seed cwd is unavailable")]
    SeedCwdUnavailable,
    #[error("provisional seed cwd is not a directory")]
    SeedCwdNotDirectory,
    #[error("provisional handoff is unavailable")]
    HandoffUnavailable,
    #[error("provisional handoff does not match the slot")]
    HandoffMismatch,
    #[error("provider exec proof is unavailable")]
    ProviderExecProofUnavailable,
}

impl ProvisionalSlot {
    fn materialized(
        state_root: &Path,
        presentation_id: Uuid,
        presentation_revision: Revision,
        candidate_runtime_id: RuntimeId,
        slot_generation: SlotGeneration,
        seed_cwd: &Path,
    ) -> Result<Self, SlotError> {
        let state_root =
            fs::canonicalize(state_root).map_err(|_| SlotError::StateRootUnavailable)?;
        let seed_cwd = fs::canonicalize(seed_cwd).map_err(|_| SlotError::SeedCwdUnavailable)?;
        if !seed_cwd.is_dir() {
            return Err(SlotError::SeedCwdNotDirectory);
        }
        Ok(Self {
            presentation_id,
            presentation_revision,
            candidate_runtime_id,
            runtime_paths: RuntimePaths::for_runtime(&state_root, candidate_runtime_id),
            seed_cwd,
            slot_generation,
            phase: ProvisionalPhase::Materialized,
            handoff_request: None,
        })
    }

    const fn cleanup_authority(&self) -> CleanupAuthority {
        match self.phase {
            ProvisionalPhase::Materialized | ProvisionalPhase::HandoffIssued => {
                CleanupAuthority::ExactProvisional
            }
            ProvisionalPhase::RuntimeOwnedLaunching
            | ProvisionalPhase::ProviderExecProven
            | ProvisionalPhase::Cancelled => CleanupAuthority::None,
        }
    }

    const fn action_allowed(&self) -> bool {
        matches!(self.phase, ProvisionalPhase::ProviderExecProven)
    }

    fn issue_handoff(&mut self, request: Uuid) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::Materialized || self.handoff_request.is_some() {
            return Err(SlotError::HandoffUnavailable);
        }
        self.handoff_request = Some(request);
        self.phase = ProvisionalPhase::HandoffIssued;
        Ok(())
    }

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

    fn consume_handoff(&mut self, request: Uuid) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::HandoffIssued {
            return Err(SlotError::HandoffUnavailable);
        }
        if self.handoff_request != Some(request) {
            return Err(SlotError::HandoffMismatch);
        }
        self.phase = ProvisionalPhase::RuntimeOwnedLaunching;
        Ok(())
    }

    fn prove_provider_exec(&mut self) -> Result<(), SlotError> {
        if self.phase != ProvisionalPhase::RuntimeOwnedLaunching {
            return Err(SlotError::ProviderExecProofUnavailable);
        }
        self.phase = ProvisionalPhase::ProviderExecProven;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, str::FromStr};

    use uuid::Uuid;

    use super::{CleanupAuthority, ProvisionalPhase, ProvisionalSlot, SlotError, SlotGeneration};
    use crate::{
        domain::{Revision, RuntimeId},
        runtime::RuntimePaths,
    };

    fn fixture() -> (tempfile::TempDir, ProvisionalSlot) {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&state_root).unwrap();
        fs::create_dir(&seed).unwrap();
        let candidate_runtime_id =
            RuntimeId::from_str("01234567-0000-0000-0000-000000000001").unwrap();
        let slot = ProvisionalSlot::materialized(
            &state_root,
            Uuid::parse_str("01234567-0000-0000-0000-000000000002").unwrap(),
            Revision::INITIAL,
            candidate_runtime_id,
            SlotGeneration(Uuid::parse_str("01234567-0000-0000-0000-000000000003").unwrap()),
            &seed,
        )
        .unwrap();
        (temporary, slot)
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
    fn unavailable_state_root_refuses_before_a_candidate_claim_exists() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            ProvisionalSlot::materialized(
                &temporary.path().join("missing-state"),
                Uuid::new_v4(),
                Revision::INITIAL,
                RuntimeId::new(),
                SlotGeneration(Uuid::new_v4()),
                &temporary.path().join("missing-seed"),
            ),
            Err(SlotError::StateRootUnavailable)
        );
    }
}
