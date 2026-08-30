//! Host-local state boundary for the current schema-15 epoch.
//!
//! The retained current lifecycle is served directly from the schema-15 registry.
//! Historical database and client artifacts are refusal evidence only; no
//! compatibility opener, migration, or client catalog is retained.

#![allow(
    clippy::missing_errors_doc,
    reason = "The current boundary exposes many small typed test seams; their shared StateError contract is documented at the module boundary."
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;

use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::{
    Clock, CompoundOperation, HostId, IdGenerator, LocationId, OnboardingPhase, OperationId,
    OperationKind, ProjectId, ProviderKind, ProviderSessionId, Revision, RuntimeId, SystemClock,
    WorkstreamId,
};
use crate::onboarding::{CapabilityError, verify_launch_capability};
use crate::provider::{
    codex::profile::OBSERVER_PROFILE_NAME,
    lifecycle::{LifecycleEvent, LifecycleObservation},
};
use crate::repository::RepositoryDiscovery;
use crate::runtime::RuntimePaths;

use super::{
    StateError, StateRoot,
    compound::bind_opencode_session_in_transaction,
    lifecycle::{LifecycleEventContext, apply_lifecycle_event},
    models::{HostRegistry, RuntimeRecord},
    runtime::{load_binding, load_opencode_handle, row_to_runtime},
    schema::{
        HOST_APPLICATION_ID, HOST_SCHEMA_15_SQL, HOST_SCHEMA_VERSION, MAX_NAVIGATOR_WORKSTREAMS,
        table_exists, table_has_column_readonly, validate_foreign_keys, validate_host_identity,
        validate_table_columns,
    },
    utils::{
        operation_phase_from_text, operation_phase_text, provider_kind_from_text,
        runtime_status_from_text, validate_project_display_name, validate_provider_metadata,
        validate_registry_text, validate_remote_identity_display, validate_repository_fingerprint,
        workstream_lifecycle_from_text,
    },
    workstream::{next_activity_sequence, touch_workstream},
};

mod bootstrap;
mod observer;
pub(crate) mod onboarding;
mod projection;
mod registry;
mod schema;

use bootstrap::{open_bootstrap_relative_file, open_bootstrap_root_directory};
use onboarding::{
    OnboardingMarkerRow, OnboardingOperationInventoryPage, PersistedOnboardingIntent,
    RuntimePathsPage, page_parameters,
};
use projection::{
    bump_project_revision, create_project, find_project_by_fingerprint, load_project_projections,
    to_sql_error, validate_project_catalog, validate_project_membership_transaction,
    validate_safe_origin_display,
};
use schema::{schema_version, validate_schema15};

#[cfg(test)]
pub(crate) use bootstrap::create_current_with_checkpoint_hook;
pub(crate) use bootstrap::{CurrentRootClassification, classify_current_root};
pub use bootstrap::{create_current, open_current};
pub(crate) use onboarding::{
    OnboardingOperationInventory, OnboardingOwnership, OnboardingPreparation,
    OnboardingPrepareRequest, OnboardingProviderExecEvidence, OnboardingProviderExecTarget,
    OnboardingProviderExecutableIdentity, OnboardingVisibility, OnboardingWorkstreamProjection,
};
pub use projection::{ProjectLocationProjection, ProjectProjection, ProjectRecord};

pub use observer::{
    ObserverDatabaseDeadline, ObserverDatabaseError, ObserverDegradedReason,
    clear_observer_degraded_marker, observer_degraded_marker_path, read_observer_degraded_marker,
    run_observer_write_with_degraded_marker, write_observer_degraded_marker,
};

pub const PROVISIONAL_LOCK_FILE: &str = "provisional.lock";
const PROVISIONAL_LOCK_FORMAT: &str = "wsnav-provisional-lock-v1";
const MAX_PROVISIONAL_LOCK_BYTES: usize = 512;
const OBSERVER_DEGRADED_MARKER_TEMP_SUFFIX: &str = ".tmp";
const MAX_PROJECT_PROJECTION_PROJECTS: usize = 512;
const MAX_PROJECT_PROJECTION_LOCATIONS: usize = 4096;
/// Exact terminal journal evidence written only after the user has explicitly
/// parked an onboarding recovery. It records recovery resolution, never
/// provider-exec proof; the retained Runtime may later be resumed normally.
const PARKED_RECOVERY_RESOLVED_OUTCOME: &str = r#"{"code":"parked_recovery_resolved_v1"}"#;
/// Exact terminal journal evidence written when presentation authority cancels
/// an unconsumed, pre-effect onboarding capability.  The operation remains
/// as a bounded audit record while its attempt-only graph is removed.
const ONBOARDING_CANCELLED_OUTCOME: &str = r#"{"code":"onboarding_cancelled_v1"}"#;
/// The final outer margin reserved for generation-scoped observer degraded
/// marker recording after a bounded observer write has failed.
pub const OBSERVER_DEGRADED_MARKER_BUDGET: Duration = Duration::from_millis(250);

/// The stable lock which owns creation and recovery of the current state
/// epoch. It intentionally remains present after bootstrap; unlike the
/// retired transition lock it is the current root's cross-process authority.
pub const BOOTSTRAP_LOCK_FILE: &str = "bootstrap.lock";
const BOOTSTRAP_LOCK_FORMAT_VERSION: u32 = 1;
const MAX_BOOTSTRAP_LOCK_BYTES: usize = 4096;
const BOOTSTRAP_STAGE_PREFIX: &str = "host.sqlite.bootstrap-";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapPhase {
    RootReserved,
    DatabaseCreateReserved,
    DatabaseOwned,
    DatabaseReady,
    ProvisionalPending,
    Ready,
}

/// Deterministic checkpoints around the current bootstrap effects.  The
/// production path installs a no-op hook; the crate-local test seam can stop
/// at any durable phase or immediately after an external filesystem/SQLite
/// effect to exercise restart classification without killing a test process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BootstrapCheckpoint {
    RootReserved,
    DatabaseCreateReserved,
    DatabaseCreated,
    DatabaseOwned,
    DatabaseInitialized,
    DatabaseReady,
    DatabasePublished,
    ProvisionalPending,
    ProvisionalCreated,
    ProvisionalReady,
    Ready,
}

type BootstrapCheckpointHook<'hook> =
    &'hook mut dyn FnMut(BootstrapCheckpoint) -> Result<(), StateError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRecordBody {
    format_version: u32,
    schema_version: i64,
    application_id: u32,
    host_id: String,
    bootstrap_generation: String,
    root_device: u64,
    root_inode: u64,
    phase: BootstrapPhase,
    database_name: Option<String>,
    database_device: Option<u64>,
    database_inode: Option<u64>,
    provisional_device: Option<u64>,
    provisional_inode: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRecordWire {
    body: BootstrapRecordBody,
    checksum: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BootstrapFileIdentity {
    device: u64,
    inode: u64,
}

type BootstrapOperationalMetadata = (String, String, i64, String, Option<i64>, Option<i64>);
type BootstrapDatabaseMetadata = (
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
);

/// A held current-format bootstrap authority. The descriptor is private and
/// never inherited by a provider process. It is retained while a bootstrap
/// effect and its durable identity are committed, then dropped once ready.
struct BootstrapLease {
    root: PathBuf,
    lock_path: PathBuf,
    root_directory: File,
    root_identity: BootstrapFileIdentity,
    lock_identity: BootstrapFileIdentity,
    record: BootstrapRecordBody,
    file: nix::fcntl::Flock<File>,
}

/// A current database opened through an exact descriptor relative to the held
/// bootstrap root. The descriptor remains live for the lifetime of the
/// `SQLite` connection, so a replacement of the visible database path cannot
/// redirect the connection after its no-follow open.
struct HeldBootstrapDatabase {
    connection: Connection,
    file: File,
}

impl std::fmt::Debug for BootstrapLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BootstrapLease")
            .field("root", &"<private>")
            .field("lock_path", &"<private>")
            .field("phase", &self.record.phase)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateMode {
    /// The current schema-15 host state.
    Current,
}

/// Recovery outcomes intentionally carry only closed categories.  In
/// particular, they do not include paths, provider payloads, or process data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRecoveryReason {
    UnknownFreshRootArtifact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshRootClassification {
    Absent,
    Empty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshRootRejection {
    NotDirectory,
    NonCanonicalDirectory,
    NonPrivateDirectory,
    UnknownArtifact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProvisionalLockPhase {
    Pending,
    Ready { expected_identity: FileIdentity },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProvisionalLockMetadata {
    host_id: String,
    generation: i64,
    phase: ProvisionalLockPhase,
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        FileIdentity {
            device: 0,
            inode: 0,
        }
    }
}

/// The retained, nonblocking exclusive lease for the host-wide
/// provisional shell slot. The descriptor is opened `CLOEXEC` and remains
/// bound to the exact root, inode, generation, and bounded lock contents for
/// its lifetime.
pub struct ProvisionalLease {
    root: PathBuf,
    lock_path: PathBuf,
    root_identity: FileIdentity,
    lock_identity: FileIdentity,
    lease_generation: i64,
    expected_contents: Vec<u8>,
    file: nix::fcntl::Flock<File>,
}

impl std::fmt::Debug for ProvisionalLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProvisionalLease")
            .field("root", &"<private>")
            .field("lock_path", &"<private>")
            .field("lease_generation", &self.lease_generation)
            .finish_non_exhaustive()
    }
}

impl ProvisionalLease {
    fn new(
        root: PathBuf,
        root_identity: FileIdentity,
        lock_path: PathBuf,
        lock_identity: FileIdentity,
        lease_generation: i64,
        expected_contents: Vec<u8>,
        file: nix::fcntl::Flock<File>,
    ) -> Self {
        Self {
            root,
            lock_path,
            root_identity,
            lock_identity,
            lease_generation,
            expected_contents,
            file,
        }
    }

    #[must_use]
    pub const fn lease_generation(&self) -> i64 {
        self.lease_generation
    }

    /// Revalidates the held root, inode, and lock contents before a
    /// actor changes a marker, capability, or onboarding journal.
    pub(crate) fn revalidate_for_mutation(&self, requested_root: &Path) -> Result<(), StateError> {
        let requested_root = fs::canonicalize(requested_root)
            .map_err(|error| StateError::io(requested_root, error))?;
        if requested_root != self.root {
            return Err(StateError::InvalidProvisionalLease);
        }
        let root_metadata =
            fs::symlink_metadata(&self.root).map_err(|_| StateError::InvalidProvisionalLease)?;
        let lock_metadata = fs::symlink_metadata(&self.lock_path)
            .map_err(|_| StateError::InvalidProvisionalLease)?;
        let opened_metadata = self
            .file
            .metadata()
            .map_err(|error| StateError::io(&self.lock_path, error))?;
        if !root_metadata.is_dir()
            || !is_private_owner_directory(&root_metadata)
            || file_identity(&root_metadata) != self.root_identity
            || !lock_metadata.is_file()
            || !is_private_owner_file(&lock_metadata)
            || file_identity(&lock_metadata) != self.lock_identity
            || !opened_metadata.is_file()
            || !is_private_owner_file(&opened_metadata)
            || file_identity(&opened_metadata) != self.lock_identity
        {
            return Err(StateError::InvalidProvisionalLease);
        }
        let mut duplicate = self
            .file
            .try_clone()
            .map_err(|error| StateError::io(&self.lock_path, error))?;
        duplicate
            .seek(SeekFrom::Start(0))
            .map_err(|error| StateError::io(&self.lock_path, error))?;
        let mut contents = Vec::with_capacity(self.expected_contents.len());
        duplicate
            .take(u64::try_from(MAX_PROVISIONAL_LOCK_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut contents)
            .map_err(|error| StateError::io(&self.lock_path, error))?;
        if contents != self.expected_contents {
            return Err(StateError::InvalidProvisionalLease);
        }
        Ok(())
    }
}

/// A state connection opened through the current schema-15 mode. Conversion
/// back to the existing `HostRegistry` is explicit and repeats the current
/// schema and artifact checks at that boundary.
pub struct CurrentState {
    connection: Connection,
    root: PathBuf,
    mode: StateMode,
}

impl std::fmt::Debug for CurrentState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CurrentState")
            .field("root", &"<private>")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

fn ensure_current_mode(mode: StateMode) -> Result<(), StateError> {
    if mode == StateMode::Current {
        Ok(())
    } else {
        Err(StateError::MalformedHostSchema)
    }
}

fn configure_current_connection(connection: &Connection) -> Result<(), StateError> {
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(StateError::Sqlite)
}

fn exact_artifact_metadata(path: &Path) -> Result<Option<fs::Metadata>, StateError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StateError::io(path, error)),
    }
}

fn is_retired_client_artifact(name: &str) -> bool {
    matches!(
        name,
        "client.sqlite" | "client.sqlite-wal" | "client.sqlite-shm"
    )
}

fn load_provisional_lock_metadata(
    connection: &Connection,
) -> Result<ProvisionalLockMetadata, StateError> {
    let (host_id, generation, phase, device, inode): (
        String,
        i64,
        String,
        Option<i64>,
        Option<i64>,
    ) = connection
        .query_row(
            "SELECT host_identity.host_id,
                        host_operational_metadata.provisional_lease_generation,
                        host_operational_metadata.provisional_lock_phase,
                        host_operational_metadata.provisional_lock_device,
                        host_operational_metadata.provisional_lock_inode
                 FROM host_identity
                 JOIN host_operational_metadata
                   ON host_operational_metadata.singleton = host_identity.singleton
                 WHERE host_identity.singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(StateError::Sqlite)?;
    if Uuid::parse_str(&host_id).is_err() || generation <= 0 {
        return Err(StateError::MalformedHostSchema);
    }
    let phase = match (phase.as_str(), device, inode) {
        ("pending", None, None) => ProvisionalLockPhase::Pending,
        ("ready", Some(device), Some(inode)) if device >= 0 && inode > 0 => {
            ProvisionalLockPhase::Ready {
                expected_identity: FileIdentity {
                    device: u64::try_from(device).map_err(|_| StateError::MalformedHostSchema)?,
                    inode: u64::try_from(inode).map_err(|_| StateError::MalformedHostSchema)?,
                },
            }
        }
        _ => return Err(StateError::MalformedHostSchema),
    };
    Ok(ProvisionalLockMetadata {
        host_id,
        generation,
        phase,
    })
}

fn provisional_lock_contents(host_id: &str, generation: i64) -> Result<Vec<u8>, StateError> {
    if Uuid::parse_str(host_id).is_err() || generation <= 0 {
        return Err(StateError::MalformedHostSchema);
    }
    let contents = format!("{PROVISIONAL_LOCK_FORMAT} {host_id} {generation}\\n").into_bytes();
    if contents.len() > MAX_PROVISIONAL_LOCK_BYTES {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(contents)
}

fn is_private_owner_directory(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = metadata.mode() & 0o777;
        mode == 0o700 && metadata.uid() == nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn is_private_owner_file(metadata: &fs::Metadata) -> bool {
    has_private_file_mode(metadata) && is_current_owner(metadata)
}

fn has_private_file_mode(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = metadata.mode() & 0o777;
        mode == 0o600
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn is_current_owner(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.uid() == nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn open_private_provisional_file_at(
    root_directory: &File,
    name: &str,
    path: &Path,
    create_new: bool,
) -> Result<File, StateError> {
    let before = exact_artifact_metadata(path)?;
    if let Some(metadata) = &before
        && (!metadata.is_file() || !is_private_owner_file(metadata))
    {
        return Err(StateError::InvalidProvisionalLease);
    }
    let file =
        open_bootstrap_relative_file(root_directory, name, path, create_new).map_err(|error| {
            match error {
                StateError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                    StateError::InvalidProvisionalLease
                }
                other => other,
            }
        })?;
    let opened = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !opened.is_file()
        || !is_current_owner(&opened)
        || before.is_some_and(|metadata| file_identity(&metadata) != file_identity(&opened))
    {
        return Err(StateError::InvalidProvisionalLease);
    }
    if create_new {
        set_private_file_permissions_handle(&file, path)?;
    }
    let private = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !private.is_file() || !is_private_owner_file(&private) {
        return Err(StateError::InvalidProvisionalLease);
    }
    Ok(file)
}

fn validate_provisional_lock_file(
    file: &File,
    path: &Path,
    expected_contents: &[u8],
) -> Result<FileIdentity, StateError> {
    let opened = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    let path_metadata = fs::symlink_metadata(path).map_err(|error| StateError::io(path, error))?;
    if !opened.is_file()
        || !is_private_owner_file(&opened)
        || !path_metadata.is_file()
        || !is_private_owner_file(&path_metadata)
        || file_identity(&opened) != file_identity(&path_metadata)
    {
        return Err(StateError::InvalidProvisionalLease);
    }
    let mut duplicate = file
        .try_clone()
        .map_err(|error| StateError::io(path, error))?;
    duplicate
        .seek(SeekFrom::Start(0))
        .map_err(|error| StateError::io(path, error))?;
    let mut contents = Vec::with_capacity(expected_contents.len());
    duplicate
        .take(u64::try_from(MAX_PROVISIONAL_LOCK_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut contents)
        .map_err(|error| StateError::io(path, error))?;
    if contents != expected_contents {
        return Err(StateError::InvalidProvisionalLease);
    }
    Ok(file_identity(&opened))
}

fn create_private_database_file_at(
    root_directory: &File,
    name: &str,
    path: &Path,
) -> Result<File, StateError> {
    if exact_artifact_metadata(path)?.is_some() {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    let file = open_bootstrap_relative_file(root_directory, name, path, true)?;
    let metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !metadata.is_file() || !is_current_owner(&metadata) {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    set_private_file_permissions_handle(&file, path)?;
    let private_metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !private_metadata.is_file() || !is_private_owner_file(&private_metadata) {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    Ok(file)
}

fn rename_bootstrap_database(
    root_directory: &File,
    stage_name: &str,
    destination: &Path,
) -> Result<(), StateError> {
    #[cfg(target_os = "linux")]
    {
        // Linux exposes renameat2 on both glibc and musl. Keep the
        // no-replace primitive explicit instead of relying on a libc-specific
        // wrapper, because an overwrite-capable rename would turn a torn
        // publication into an unbounded adoption effect.
        rustix::fs::renameat_with(
            root_directory,
            stage_name,
            root_directory,
            "host.sqlite",
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| StateError::io(destination, error.into()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root_directory, stage_name, destination);
        // There is no portable no-replace rename primitive in this slice.
        // Refuse publication rather than silently falling back to overwrite.
        Err(StateError::InvalidBootstrapLock)
    }
}

fn set_private_file_permissions_handle(file: &File, path: &Path) -> Result<(), StateError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| StateError::io(path, error))
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        Err(StateError::InvalidBootstrapLock)
    }
}

fn open_private_observer_marker_file(path: &Path) -> Result<File, StateError> {
    if exact_artifact_metadata(path)?.is_some() {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !metadata.is_file() || !is_private_owner_file(&metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    set_private_file_permissions_handle(&file, path)?;
    let private_metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !private_metadata.is_file() || !is_private_owner_file(&private_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(file)
}

fn sync_directory(path: &Path) -> Result<(), StateError> {
    let directory = File::open(path).map_err(|error| StateError::io(path, error))?;
    directory
        .sync_all()
        .map_err(|error| StateError::io(path, error))
}
