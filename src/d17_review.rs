//! Exact ownership for the disposable cwd used by native Codex hook review.
//!
//! The review directory lives beneath one already-owned D17 presentation.  A
//! bounded sibling marker records the presentation, owner process, and final
//! directory inode.  Cleanup never recurses: it quarantines and revalidates
//! the exact empty directory before removing it, then does the same for the
//! marker.  A later review never adopts an interrupted owner: only the D17
//! presentation lifecycle may finish cleanup, after it has stopped every pane
//! and provisional Runtime that could still be using the cwd.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::Revision,
    runtime::{LinuxProcessProbe, ProcessProbe},
};

const REVIEW_MARKER_FILE: &str = "d17-codex-review.json";
const REVIEW_DIRECTORY_PREFIX: &str = "d17-codex-review-";
const RETIRING_DIRECTORY_PREFIX: &str = ".d17-codex-review-retiring-";
const RETIRING_MARKER_PREFIX: &str = ".d17-codex-review-retiring-";
const RETIRING_MARKER_SUFFIX: &str = ".json";
const REVIEW_RECORD_VERSION: u8 = 1;
const MAX_REVIEW_RECORD_BYTES: usize = 4 * 1024;
const MAX_OWNER_BIRTH_BYTES: usize = 256;
const MAX_REVIEW_ARTIFACTS: usize = 8;

#[derive(Debug, Error)]
pub(crate) enum D17ReviewError {
    #[error("D17 review ownership is unavailable")]
    Unavailable,
    #[error("D17 review ownership is ambiguous")]
    Ambiguous,
    #[error("D17 review is already active")]
    Active,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewRecord {
    version: u8,
    owner_id: Uuid,
    owner_pid: u32,
    owner_birth: String,
    presentation_id: Uuid,
    presentation_revision: Revision,
    presentation_device: u64,
    presentation_inode: u64,
    directory_device: u64,
    directory_inode: u64,
}

impl ReviewRecord {
    fn validate(
        &self,
        presentation_id: Uuid,
        presentation_revision: Revision,
    ) -> Result<(), D17ReviewError> {
        if self.version != REVIEW_RECORD_VERSION
            || self.owner_id.is_nil()
            || self.owner_pid == 0
            || self.owner_birth.is_empty()
            || self.owner_birth.len() > MAX_OWNER_BIRTH_BYTES
            || self.presentation_id != presentation_id
            || self.presentation_revision != presentation_revision
            || self.presentation_inode == 0
            || self.directory_inode == 0
        {
            return Err(D17ReviewError::Ambiguous);
        }
        Ok(())
    }

    fn active_directory_name(&self) -> String {
        format!("{REVIEW_DIRECTORY_PREFIX}{}", self.owner_id)
    }

    fn retiring_directory_name(&self) -> String {
        format!("{RETIRING_DIRECTORY_PREFIX}{}", self.owner_id)
    }

    fn retiring_marker_name(&self) -> String {
        format!(
            "{RETIRING_MARKER_PREFIX}{}{RETIRING_MARKER_SUFFIX}",
            self.owner_id
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Self {
                device: 0,
                inode: 0,
            }
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        self == Self::from_metadata(metadata)
    }
}

struct MarkerProof {
    path: PathBuf,
    record: ReviewRecord,
    identity: FileIdentity,
}

/// Process-local owner of one exact native-review cwd.
pub(crate) struct D17ReviewDirectory {
    presentation_directory: PathBuf,
    record: ReviewRecord,
    marker_identity: FileIdentity,
    cleaned: bool,
}

impl D17ReviewDirectory {
    /// Creates one fresh marker-bound empty directory for the current process.
    /// Any prior review artifact remains presentation-recovery work; a new
    /// review never assumes that an absent parent also proves its native child
    /// is gone.
    pub(crate) fn create(
        presentation_directory: &Path,
        presentation_id: Uuid,
        presentation_revision: Revision,
    ) -> Result<Self, D17ReviewError> {
        recover_for_creation_with_probe(
            presentation_directory,
            presentation_id,
            presentation_revision,
            &LinuxProcessProbe,
        )?;
        let owner_pid = process::id();
        let owner_birth = LinuxProcessProbe
            .process_birth_checked(owner_pid)
            .map_err(|_| D17ReviewError::Unavailable)?
            .ok_or(D17ReviewError::Unavailable)?;
        if owner_birth.is_empty() || owner_birth.len() > MAX_OWNER_BIRTH_BYTES {
            return Err(D17ReviewError::Unavailable);
        }
        let presentation_metadata = fs::symlink_metadata(presentation_directory)
            .map_err(|_| D17ReviewError::Unavailable)?;
        if !presentation_directory.is_absolute() || !private_directory(&presentation_metadata) {
            return Err(D17ReviewError::Ambiguous);
        }
        let presentation_identity = FileIdentity::from_metadata(&presentation_metadata);
        if presentation_identity.inode == 0 {
            return Err(D17ReviewError::Unavailable);
        }

        let owner_token = Uuid::new_v4();
        let directory =
            presentation_directory.join(format!("{REVIEW_DIRECTORY_PREFIX}{owner_token}"));
        create_private_directory(&directory)?;
        let directory_metadata =
            fs::symlink_metadata(&directory).map_err(|_| D17ReviewError::Unavailable)?;
        if !private_directory(&directory_metadata) || !directory_is_empty(&directory)? {
            let _ = fs::remove_dir(&directory);
            return Err(D17ReviewError::Ambiguous);
        }
        let directory_identity = FileIdentity::from_metadata(&directory_metadata);
        if directory_identity.inode == 0 {
            let _ = fs::remove_dir(&directory);
            return Err(D17ReviewError::Unavailable);
        }
        let record = ReviewRecord {
            version: REVIEW_RECORD_VERSION,
            owner_id: owner_token,
            owner_pid,
            owner_birth,
            presentation_id,
            presentation_revision,
            presentation_device: presentation_identity.device,
            presentation_inode: presentation_identity.inode,
            directory_device: directory_identity.device,
            directory_inode: directory_identity.inode,
        };
        let marker_path = presentation_directory.join(REVIEW_MARKER_FILE);
        let marker_identity = match write_new_marker(&marker_path, &record) {
            Ok(identity) => identity,
            Err(error) => {
                let current = fs::symlink_metadata(&directory).ok();
                if current
                    .as_ref()
                    .is_some_and(|metadata| directory_identity.matches(metadata))
                {
                    let _ = fs::remove_dir(&directory);
                }
                return Err(error);
            }
        };
        sync_directory(presentation_directory)?;
        Ok(Self {
            presentation_directory: presentation_directory.to_path_buf(),
            record,
            marker_identity,
            cleaned: false,
        })
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.presentation_directory
            .join(self.record.active_directory_name())
    }

    /// Removes only this owner's exact empty directory and marker.
    pub(crate) fn cleanup(&mut self) -> Result<(), D17ReviewError> {
        if self.cleaned {
            return Ok(());
        }
        cleanup_expected(
            &self.presentation_directory,
            &self.record,
            Some(self.marker_identity),
        )?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for D17ReviewDirectory {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Validates every review-shaped artifact while the presentation ownership
/// reader remains mutation-free. Unknown, changed, non-private, or non-empty
/// paths fail closed.
pub(crate) fn validate_artifacts(
    presentation_directory: &Path,
    presentation_id: Uuid,
    presentation_revision: Revision,
) -> Result<(), D17ReviewError> {
    let inventory = review_inventory(presentation_directory)?;
    if inventory.len() > MAX_REVIEW_ARTIFACTS {
        return Err(D17ReviewError::Ambiguous);
    }
    let markers = marker_proofs(
        presentation_directory,
        presentation_id,
        presentation_revision,
        &inventory,
    )?;
    if markers.len() > 1 {
        return Err(D17ReviewError::Ambiguous);
    }
    if let Some(marker) = markers.first() {
        validate_record_directories(presentation_directory, &marker.record, &inventory)?;
    } else {
        validate_orphan_directories(presentation_directory, &inventory)?;
    }
    Ok(())
}

/// Completes any exact review cleanup after all presentation-owned processes
/// have stopped. This is the hard-interruption recovery boundary.
pub(crate) fn recover_after_presentation_stop(
    presentation_directory: &Path,
    presentation_id: Uuid,
    presentation_revision: Revision,
) -> Result<(), D17ReviewError> {
    recover_without_liveness(
        presentation_directory,
        presentation_id,
        presentation_revision,
    )
}

fn recover_for_creation_with_probe(
    presentation_directory: &Path,
    presentation_id: Uuid,
    presentation_revision: Revision,
    process_probe: &dyn ProcessProbe,
) -> Result<(), D17ReviewError> {
    let inventory = review_inventory(presentation_directory)?;
    let markers = marker_proofs(
        presentation_directory,
        presentation_id,
        presentation_revision,
        &inventory,
    )?;
    if markers.len() > 1 {
        return Err(D17ReviewError::Ambiguous);
    }
    if let Some(marker) = markers.first() {
        match process_probe
            .process_birth_checked(marker.record.owner_pid)
            .map_err(|_| D17ReviewError::Unavailable)?
        {
            Some(birth) if birth == marker.record.owner_birth => {
                return Err(D17ReviewError::Active);
            }
            Some(_) | None => return Err(D17ReviewError::Ambiguous),
        }
    } else if !inventory.is_empty() {
        return Err(D17ReviewError::Ambiguous);
    }
    validate_no_review_artifacts(presentation_directory)
}

fn recover_without_liveness(
    presentation_directory: &Path,
    presentation_id: Uuid,
    presentation_revision: Revision,
) -> Result<(), D17ReviewError> {
    let inventory = review_inventory(presentation_directory)?;
    let markers = marker_proofs(
        presentation_directory,
        presentation_id,
        presentation_revision,
        &inventory,
    )?;
    if markers.len() > 1 {
        return Err(D17ReviewError::Ambiguous);
    }
    if let Some(marker) = markers.first() {
        cleanup_expected(
            presentation_directory,
            &marker.record,
            Some(marker.identity),
        )?;
    } else {
        cleanup_orphan_directories(presentation_directory, &inventory)?;
    }
    validate_no_review_artifacts(presentation_directory)
}

fn cleanup_expected(
    presentation_directory: &Path,
    expected: &ReviewRecord,
    expected_marker_identity: Option<FileIdentity>,
) -> Result<(), D17ReviewError> {
    validate_presentation_identity(presentation_directory, expected)?;
    let inventory = review_inventory(presentation_directory)?;
    let marker = locate_expected_marker(presentation_directory, expected, &inventory)?;
    let Some(marker) = marker else {
        return cleanup_orphan_for_record(presentation_directory, expected, &inventory);
    };
    if marker.record != *expected
        || expected_marker_identity.is_some_and(|identity| identity != marker.identity)
    {
        return Err(D17ReviewError::Ambiguous);
    }
    let marker = quarantine_marker(presentation_directory, &marker)?;
    quarantine_and_remove_directory(presentation_directory, expected)?;
    remove_quarantined_marker(&marker)?;
    sync_directory(presentation_directory)
}

fn cleanup_orphan_for_record(
    presentation_directory: &Path,
    expected: &ReviewRecord,
    inventory: &[ReviewArtifact],
) -> Result<(), D17ReviewError> {
    let names = [
        expected.active_directory_name(),
        expected.retiring_directory_name(),
    ];
    if inventory
        .iter()
        .any(|artifact| !names.iter().any(|name| name == &artifact.name))
    {
        return Err(D17ReviewError::Ambiguous);
    }
    quarantine_and_remove_directory(presentation_directory, expected)
}

fn quarantine_marker(
    presentation_directory: &Path,
    marker: &MarkerProof,
) -> Result<MarkerProof, D17ReviewError> {
    let retiring = presentation_directory.join(marker.record.retiring_marker_name());
    if marker.path != retiring {
        if fs::symlink_metadata(&retiring).is_ok() {
            return Err(D17ReviewError::Ambiguous);
        }
        fs::rename(&marker.path, &retiring).map_err(|_| D17ReviewError::Unavailable)?;
    }
    let proof = read_marker_at(
        &retiring,
        marker.record.presentation_id,
        marker.record.presentation_revision,
    )?;
    if proof.record != marker.record || proof.identity != marker.identity {
        return Err(D17ReviewError::Ambiguous);
    }
    Ok(proof)
}

fn quarantine_and_remove_directory(
    presentation_directory: &Path,
    record: &ReviewRecord,
) -> Result<(), D17ReviewError> {
    let active = presentation_directory.join(record.active_directory_name());
    let retiring = presentation_directory.join(record.retiring_directory_name());
    let active_metadata = path_metadata(&active)?;
    let retiring_metadata = path_metadata(&retiring)?;
    if active_metadata.is_some() && retiring_metadata.is_some() {
        return Err(D17ReviewError::Ambiguous);
    }
    let expected = FileIdentity {
        device: record.directory_device,
        inode: record.directory_inode,
    };
    let source = if let Some(metadata) = active_metadata {
        validate_review_directory(&active, &metadata, expected)?;
        fs::rename(&active, &retiring).map_err(|_| D17ReviewError::Unavailable)?;
        retiring.as_path()
    } else if let Some(metadata) = retiring_metadata {
        validate_review_directory(&retiring, &metadata, expected)?;
        retiring.as_path()
    } else {
        return Ok(());
    };
    let after = fs::symlink_metadata(source).map_err(|_| D17ReviewError::Ambiguous)?;
    validate_review_directory(source, &after, expected)?;
    fs::remove_dir(source).map_err(|_| D17ReviewError::Unavailable)
}

fn remove_quarantined_marker(marker: &MarkerProof) -> Result<(), D17ReviewError> {
    let current = read_marker_at(
        &marker.path,
        marker.record.presentation_id,
        marker.record.presentation_revision,
    )?;
    if current.record != marker.record || current.identity != marker.identity {
        return Err(D17ReviewError::Ambiguous);
    }
    fs::remove_file(&marker.path).map_err(|_| D17ReviewError::Unavailable)
}

fn cleanup_orphan_directories(
    presentation_directory: &Path,
    inventory: &[ReviewArtifact],
) -> Result<(), D17ReviewError> {
    validate_orphan_directories(presentation_directory, inventory)?;
    for artifact in inventory {
        if artifact.kind != ReviewArtifactKind::Directory {
            return Err(D17ReviewError::Ambiguous);
        }
        let source = presentation_directory.join(&artifact.name);
        let owner_id = review_directory_owner(&artifact.name).ok_or(D17ReviewError::Ambiguous)?;
        let retiring =
            presentation_directory.join(format!("{RETIRING_DIRECTORY_PREFIX}{owner_id}"));
        let target = if artifact.name.starts_with(RETIRING_DIRECTORY_PREFIX) {
            source.clone()
        } else {
            if fs::symlink_metadata(&retiring).is_ok() {
                return Err(D17ReviewError::Ambiguous);
            }
            fs::rename(&source, &retiring).map_err(|_| D17ReviewError::Unavailable)?;
            retiring
        };
        let after = fs::symlink_metadata(&target).map_err(|_| D17ReviewError::Ambiguous)?;
        if !artifact.identity.matches(&after)
            || !private_directory(&after)
            || !directory_is_empty(&target)?
        {
            return Err(D17ReviewError::Ambiguous);
        }
        fs::remove_dir(&target).map_err(|_| D17ReviewError::Unavailable)?;
    }
    sync_directory(presentation_directory)
}

fn validate_record_directories(
    presentation_directory: &Path,
    record: &ReviewRecord,
    inventory: &[ReviewArtifact],
) -> Result<(), D17ReviewError> {
    let allowed = [
        REVIEW_MARKER_FILE.to_owned(),
        record.retiring_marker_name(),
        record.active_directory_name(),
        record.retiring_directory_name(),
    ];
    if inventory
        .iter()
        .any(|artifact| !allowed.iter().any(|name| name == &artifact.name))
    {
        return Err(D17ReviewError::Ambiguous);
    }
    let active = inventory
        .iter()
        .find(|artifact| artifact.name == record.active_directory_name());
    let retiring = inventory
        .iter()
        .find(|artifact| artifact.name == record.retiring_directory_name());
    if active.is_some() && retiring.is_some() {
        return Err(D17ReviewError::Ambiguous);
    }
    if let Some(directory) = active.or(retiring) {
        let path = presentation_directory.join(&directory.name);
        validate_review_directory(
            &path,
            &directory.metadata,
            FileIdentity {
                device: record.directory_device,
                inode: record.directory_inode,
            },
        )?;
    }
    Ok(())
}

fn validate_orphan_directories(
    presentation_directory: &Path,
    inventory: &[ReviewArtifact],
) -> Result<(), D17ReviewError> {
    if inventory.len() > 1 {
        return Err(D17ReviewError::Ambiguous);
    }
    for artifact in inventory {
        if artifact.kind != ReviewArtifactKind::Directory
            || review_directory_owner(&artifact.name).is_none()
            || !private_directory(&artifact.metadata)
            || !directory_is_empty(&presentation_directory.join(&artifact.name))?
        {
            return Err(D17ReviewError::Ambiguous);
        }
    }
    Ok(())
}

fn validate_review_directory(
    path: &Path,
    metadata: &fs::Metadata,
    expected: FileIdentity,
) -> Result<(), D17ReviewError> {
    if !private_directory(metadata) || !expected.matches(metadata) || !directory_is_empty(path)? {
        return Err(D17ReviewError::Ambiguous);
    }
    Ok(())
}

fn locate_expected_marker(
    presentation_directory: &Path,
    expected: &ReviewRecord,
    inventory: &[ReviewArtifact],
) -> Result<Option<MarkerProof>, D17ReviewError> {
    let candidates = [
        REVIEW_MARKER_FILE.to_owned(),
        expected.retiring_marker_name(),
    ];
    let present = inventory
        .iter()
        .filter(|artifact| candidates.iter().any(|name| name == &artifact.name))
        .collect::<Vec<_>>();
    match present.as_slice() {
        [] => Ok(None),
        [artifact] if artifact.kind == ReviewArtifactKind::Marker => read_marker_at(
            &presentation_directory.join(&artifact.name),
            expected.presentation_id,
            expected.presentation_revision,
        )
        .map(Some),
        _ => Err(D17ReviewError::Ambiguous),
    }
}

fn marker_proofs(
    presentation_directory: &Path,
    presentation_id: Uuid,
    presentation_revision: Revision,
    inventory: &[ReviewArtifact],
) -> Result<Vec<MarkerProof>, D17ReviewError> {
    inventory
        .iter()
        .filter(|artifact| artifact.kind == ReviewArtifactKind::Marker)
        .map(|artifact| {
            read_marker_at(
                &presentation_directory.join(&artifact.name),
                presentation_id,
                presentation_revision,
            )
        })
        .collect()
}

fn read_marker_at(
    path: &Path,
    presentation_id: Uuid,
    presentation_revision: Revision,
) -> Result<MarkerProof, D17ReviewError> {
    let before = fs::symlink_metadata(path).map_err(|_| D17ReviewError::Unavailable)?;
    if !private_file(&before)
        || usize::try_from(before.len()).unwrap_or(usize::MAX) > MAX_REVIEW_RECORD_BYTES
    {
        return Err(D17ReviewError::Ambiguous);
    }
    let mut file = File::open(path).map_err(|_| D17ReviewError::Unavailable)?;
    let opened = file.metadata().map_err(|_| D17ReviewError::Unavailable)?;
    let identity = FileIdentity::from_metadata(&opened);
    if !private_file(&opened) || !identity.matches(&before) {
        return Err(D17ReviewError::Ambiguous);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(u64::try_from(MAX_REVIEW_RECORD_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| D17ReviewError::Unavailable)?;
    if bytes.len() > MAX_REVIEW_RECORD_BYTES {
        return Err(D17ReviewError::Ambiguous);
    }
    let after = fs::symlink_metadata(path).map_err(|_| D17ReviewError::Unavailable)?;
    if !identity.matches(&after) || !private_file(&after) {
        return Err(D17ReviewError::Ambiguous);
    }
    let record: ReviewRecord =
        serde_json::from_slice(&bytes).map_err(|_| D17ReviewError::Ambiguous)?;
    record.validate(presentation_id, presentation_revision)?;
    let presentation_directory = path.parent().ok_or(D17ReviewError::Ambiguous)?;
    validate_presentation_identity(presentation_directory, &record)?;
    Ok(MarkerProof {
        path: path.to_path_buf(),
        record,
        identity,
    })
}

fn write_new_marker(path: &Path, record: &ReviewRecord) -> Result<FileIdentity, D17ReviewError> {
    let bytes = serde_json::to_vec(record).map_err(|_| D17ReviewError::Unavailable)?;
    if bytes.len() > MAX_REVIEW_RECORD_BYTES {
        return Err(D17ReviewError::Unavailable);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| D17ReviewError::Unavailable)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| D17ReviewError::Unavailable)?;
    let metadata = file.metadata().map_err(|_| D17ReviewError::Unavailable)?;
    if !private_file(&metadata) {
        return Err(D17ReviewError::Ambiguous);
    }
    Ok(FileIdentity::from_metadata(&metadata))
}

fn create_private_directory(path: &Path) -> Result<(), D17ReviewError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| D17ReviewError::Unavailable)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReviewArtifactKind {
    Marker,
    Directory,
}

struct ReviewArtifact {
    name: String,
    kind: ReviewArtifactKind,
    metadata: fs::Metadata,
    identity: FileIdentity,
}

fn review_inventory(presentation_directory: &Path) -> Result<Vec<ReviewArtifact>, D17ReviewError> {
    let entries = fs::read_dir(presentation_directory).map_err(|_| D17ReviewError::Unavailable)?;
    let mut inventory = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| D17ReviewError::Unavailable)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| D17ReviewError::Ambiguous)?;
        let Some(kind) = review_artifact_kind(&name) else {
            if review_shaped_name(&name) {
                return Err(D17ReviewError::Ambiguous);
            }
            continue;
        };
        if inventory.len() >= MAX_REVIEW_ARTIFACTS {
            return Err(D17ReviewError::Ambiguous);
        }
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| D17ReviewError::Unavailable)?;
        if metadata.file_type().is_symlink()
            || (kind == ReviewArtifactKind::Marker && !private_file(&metadata))
            || (kind == ReviewArtifactKind::Directory && !private_directory(&metadata))
        {
            return Err(D17ReviewError::Ambiguous);
        }
        inventory.push(ReviewArtifact {
            name,
            kind,
            identity: FileIdentity::from_metadata(&metadata),
            metadata,
        });
    }
    Ok(inventory)
}

fn review_artifact_kind(name: &str) -> Option<ReviewArtifactKind> {
    if name == REVIEW_MARKER_FILE || retiring_marker_owner(name).is_some() {
        Some(ReviewArtifactKind::Marker)
    } else if review_directory_owner(name).is_some() {
        Some(ReviewArtifactKind::Directory)
    } else {
        None
    }
}

fn review_shaped_name(name: &str) -> bool {
    name == REVIEW_MARKER_FILE
        || name.starts_with(REVIEW_DIRECTORY_PREFIX)
        || name.starts_with(RETIRING_DIRECTORY_PREFIX)
        || name.starts_with(RETIRING_MARKER_PREFIX)
}

pub(crate) fn is_review_artifact_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().and_then(review_artifact_kind).is_some()
}

fn review_directory_owner(name: &str) -> Option<Uuid> {
    name.strip_prefix(REVIEW_DIRECTORY_PREFIX)
        .or_else(|| name.strip_prefix(RETIRING_DIRECTORY_PREFIX))
        .and_then(|owner| owner.parse().ok())
}

fn retiring_marker_owner(name: &str) -> Option<Uuid> {
    name.strip_prefix(RETIRING_MARKER_PREFIX)
        .and_then(|owner| owner.strip_suffix(RETIRING_MARKER_SUFFIX))
        .and_then(|owner| owner.parse().ok())
}

fn validate_no_review_artifacts(presentation_directory: &Path) -> Result<(), D17ReviewError> {
    if review_inventory(presentation_directory)?.is_empty() {
        Ok(())
    } else {
        Err(D17ReviewError::Ambiguous)
    }
}

fn path_metadata(path: &Path) -> Result<Option<fs::Metadata>, D17ReviewError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(D17ReviewError::Unavailable),
    }
}

fn validate_presentation_identity(
    presentation_directory: &Path,
    record: &ReviewRecord,
) -> Result<(), D17ReviewError> {
    let metadata =
        fs::symlink_metadata(presentation_directory).map_err(|_| D17ReviewError::Unavailable)?;
    let expected = FileIdentity {
        device: record.presentation_device,
        inode: record.presentation_inode,
    };
    if !presentation_directory.is_absolute()
        || !private_directory(&metadata)
        || !expected.matches(&metadata)
    {
        return Err(D17ReviewError::Ambiguous);
    }
    Ok(())
}

fn directory_is_empty(path: &Path) -> Result<bool, D17ReviewError> {
    let mut entries = fs::read_dir(path).map_err(|_| D17ReviewError::Unavailable)?;
    entries
        .next()
        .transpose()
        .map(|entry| entry.is_none())
        .map_err(|_| D17ReviewError::Unavailable)
}

fn private_directory(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        metadata.uid() == nix::unistd::Uid::effective().as_raw()
            && metadata.permissions().mode() & 0o777 == 0o700
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn private_file(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        metadata.uid() == nix::unistd::Uid::effective().as_raw()
            && metadata.permissions().mode() & 0o777 == 0o600
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sync_directory(path: &Path) -> Result<(), D17ReviewError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| D17ReviewError::Unavailable)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, mem::ManuallyDrop};

    use super::*;
    use crate::runtime::ProcessProbeError;

    struct MissingProcessProbe;

    impl ProcessProbe for MissingProcessProbe {
        fn process_birth(&self, _pid: u32) -> Option<String> {
            None
        }

        fn process_birth_checked(&self, _pid: u32) -> Result<Option<String>, ProcessProbeError> {
            Ok(None)
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, Uuid, Revision) {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = temporary.path().join("presentation");
        create_private_directory(&presentation).unwrap();
        (temporary, presentation, Uuid::new_v4(), Revision::INITIAL)
    }

    #[test]
    fn exact_owner_removes_only_its_empty_directory_and_marker() {
        let (_temporary, presentation, id, revision) = fixture();
        let mut owner = D17ReviewDirectory::create(&presentation, id, revision).unwrap();
        let directory = owner.path();
        assert!(directory.is_dir());
        validate_artifacts(&presentation, id, revision).unwrap();

        owner.cleanup().unwrap();

        assert!(!directory.exists());
        assert!(review_inventory(&presentation).unwrap().is_empty());
    }

    #[test]
    fn changed_directory_is_preserved_and_cleanup_fails_closed() {
        let (_temporary, presentation, id, revision) = fixture();
        let owner = D17ReviewDirectory::create(&presentation, id, revision).unwrap();
        let directory = owner.path();
        fs::remove_dir(&directory).unwrap();
        create_private_directory(&directory).unwrap();
        fs::write(directory.join("foreign"), b"preserve").unwrap();

        let result = ManuallyDrop::new(owner);
        let cleanup = cleanup_expected(
            &result.presentation_directory,
            &result.record,
            Some(result.marker_identity),
        );

        assert!(matches!(cleanup, Err(D17ReviewError::Ambiguous)));
        assert_eq!(fs::read(directory.join("foreign")).unwrap(), b"preserve");
    }

    #[test]
    fn replaced_presentation_parent_preserves_the_original_review() {
        let (_temporary, presentation, id, revision) = fixture();
        let owner =
            ManuallyDrop::new(D17ReviewDirectory::create(&presentation, id, revision).unwrap());
        let original = presentation.with_extension("original");
        fs::rename(&presentation, &original).unwrap();
        create_private_directory(&presentation).unwrap();

        let cleanup = cleanup_expected(
            &owner.presentation_directory,
            &owner.record,
            Some(owner.marker_identity),
        );

        assert!(matches!(cleanup, Err(D17ReviewError::Ambiguous)));
        assert!(original.join(owner.record.active_directory_name()).exists());
        assert!(original.join(REVIEW_MARKER_FILE).exists());
    }

    #[test]
    fn malformed_review_shaped_artifact_blocks_a_new_owner() {
        let (_temporary, presentation, id, revision) = fixture();
        create_private_directory(&presentation.join("d17-codex-review-not-a-uuid")).unwrap();

        assert!(matches!(
            D17ReviewDirectory::create(&presentation, id, revision),
            Err(D17ReviewError::Ambiguous)
        ));
    }

    #[test]
    fn dead_owner_cannot_be_adopted_by_a_new_review() {
        let (_temporary, presentation, id, revision) = fixture();
        let owner =
            ManuallyDrop::new(D17ReviewDirectory::create(&presentation, id, revision).unwrap());
        let directory = owner.path();

        assert!(matches!(
            recover_for_creation_with_probe(&presentation, id, revision, &MissingProcessProbe,),
            Err(D17ReviewError::Ambiguous)
        ));
        assert!(directory.exists());

        recover_after_presentation_stop(&presentation, id, revision).unwrap();
        assert!(!directory.exists());
    }

    #[test]
    fn live_owner_blocks_recovery() {
        struct CurrentProcessProbe {
            birth: RefCell<Option<String>>,
        }
        impl ProcessProbe for CurrentProcessProbe {
            fn process_birth(&self, _pid: u32) -> Option<String> {
                self.birth.borrow().clone()
            }
        }

        let (_temporary, presentation, id, revision) = fixture();
        let owner =
            ManuallyDrop::new(D17ReviewDirectory::create(&presentation, id, revision).unwrap());
        let probe = CurrentProcessProbe {
            birth: RefCell::new(Some(owner.record.owner_birth.clone())),
        };

        assert!(matches!(
            recover_for_creation_with_probe(&presentation, id, revision, &probe),
            Err(D17ReviewError::Active)
        ));
        assert!(owner.path().exists());
    }

    #[test]
    fn presentation_stop_recovers_a_quarantined_crash_gap() {
        let (_temporary, presentation, id, revision) = fixture();
        let owner =
            ManuallyDrop::new(D17ReviewDirectory::create(&presentation, id, revision).unwrap());
        let marker = presentation.join(REVIEW_MARKER_FILE);
        let retiring_marker = presentation.join(owner.record.retiring_marker_name());
        let directory = owner.path();
        let retiring_directory = presentation.join(owner.record.retiring_directory_name());
        fs::rename(marker, &retiring_marker).unwrap();
        fs::rename(directory, &retiring_directory).unwrap();

        recover_after_presentation_stop(&presentation, id, revision).unwrap();

        assert!(!retiring_marker.exists());
        assert!(!retiring_directory.exists());
    }
}
