//! D17 presentation-private provisional-slot authority.
//!
//! This module owns only the bounded marker contract for an unregistered
//! candidate Runtime.  It deliberately cannot create, attach, signal, or
//! adopt a tmux server; the atomic Navigator cutover will compose it with the
//! stable host lease, presentation proof, and broker/helper boundaries.

#![allow(
    dead_code,
    reason = "the D17 provisional marker remains unreachable until the atomic Navigator cutover"
)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
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
pub(crate) struct SlotGeneration(Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProvisionalPhase {
    Materialized,
    HandoffIssued,
    RuntimeOwnedLaunching,
    ProviderExecProven,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
}

const PROVISIONAL_MARKER_VERSION: u8 = 1;
const MAX_PROVISIONAL_MARKER_BYTES: usize = 8 * 1024;
pub(crate) const PROVISIONAL_MARKER_FILE: &str = "d17-provisional.json";

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
    fn materialized(
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
        CleanupAuthority, PROVISIONAL_MARKER_FILE, ProvisionalMarker, ProvisionalPhase,
        ProvisionalSlot, SlotError, SlotGeneration, read_marker, write_new_marker,
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
            1,
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
