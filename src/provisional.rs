//! D17 test-only provisional ownership model.
//!
//! The production marker, lease, broker, and helper will consume this lifecycle
//! contract only at the later atomic cutover. Keeping it test-only now proves
//! that D16 has no hidden dependency on D17 onboarding behavior.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
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
    #[error("provisional marker could not be encoded")]
    MarkerEncoding,
    #[error("provisional marker is oversized")]
    MarkerOversized,
    #[error("provisional marker is malformed")]
    MarkerMalformed,
    #[error("provisional marker runtime paths do not match the candidate")]
    MarkerRuntimePathsMismatch,
}

const PROVISIONAL_MARKER_VERSION: u8 = 1;
const MAX_PROVISIONAL_MARKER_BYTES: usize = 8 * 1024;

/// Presentation-private evidence for one unregistered materialized candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProvisionalMarker {
    version: u8,
    presentation_id: Uuid,
    presentation_revision: Revision,
    candidate_runtime_id: RuntimeId,
    directory: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    session_name: String,
    seed_cwd: PathBuf,
    slot_generation: Uuid,
}

impl ProvisionalMarker {
    fn from_slot(slot: &ProvisionalSlot) -> Self {
        Self {
            version: PROVISIONAL_MARKER_VERSION,
            presentation_id: slot.presentation_id,
            presentation_revision: slot.presentation_revision,
            candidate_runtime_id: slot.candidate_runtime_id,
            directory: slot.runtime_paths.directory.clone(),
            socket: slot.runtime_paths.socket.clone(),
            config: slot.runtime_paths.config.clone(),
            session_name: slot.runtime_paths.session_name.clone(),
            seed_cwd: slot.seed_cwd.clone(),
            slot_generation: slot.slot_generation.0,
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
        let slot = ProvisionalSlot::materialized(
            state_root,
            marker.presentation_id,
            marker.presentation_revision,
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
        Ok(slot)
    }
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
    use std::{cell::RefCell, collections::BTreeMap, ffi::OsString, fs, str::FromStr};

    use uuid::Uuid;

    use super::{
        CleanupAuthority, ProvisionalMarker, ProvisionalPhase, ProvisionalSlot, SlotError,
        SlotGeneration,
    };
    use crate::{
        domain::{Revision, RuntimeId},
        runtime::{
            NativeLaunch, PrivateRuntime, ProcessProbe, RuntimeError, RuntimePaths, TmuxClient,
            TmuxInvocation, TmuxResponse,
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
    }
}
