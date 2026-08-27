//! D16 state boundary.
//!
//! This module owns the D16 state boundary. Current-only opens, observer
//! transitions, fresh creation, and the explicit schema-12 cutover are the
//! only supported production modes. The exact three retired client database
//! filenames remain transition evidence only; no client catalog is opened or
//! imported.

#![allow(
    clippy::missing_errors_doc,
    reason = "The D16 boundary exposes many small typed test seams; their shared StateError contract is documented at the module boundary."
)]

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::{
    Clock, CompoundOperation, HostId, IdGenerator, LocationId, OnboardingPhase, OperationId,
    OperationKind, ProjectId, ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeStatus,
    SystemClock, WorkstreamId,
};
use crate::onboarding::{
    CapabilityError, LaunchCapability, LaunchCapabilityClaims, LaunchCapabilityMetadata,
    verify_launch_capability,
};
use crate::provider::lifecycle::{LifecycleEvent, LifecycleHint, LifecycleObservation};
use crate::repository::RepositoryRegistration;
use crate::runtime::RuntimePaths;

use super::{
    StateError, StateRoot,
    lifecycle::{
        LifecycleEventContext, apply_lifecycle_event, apply_opencode_lifecycle_transition,
        validate_opencode_observation,
    },
    models::{
        ExternalWorkstream, HostRegistry, OpenCodeObserverStatus, OpenCodeRuntimeHandle,
        ProviderBinding, RuntimeRecord,
    },
    runtime::{load_binding, load_current_binding, load_opencode_handle, row_to_runtime},
    schema::HOST_SCHEMA_SQL,
    utils::{
        operation_phase_from_text, operation_phase_text, resolve_project_browser_root,
        runtime_status_from_text, validate_project_display_name, validate_provider_metadata,
        validate_registry_text, validate_remote_identity_display, validate_repository_fingerprint,
        workstream_lifecycle_from_text,
    },
    workstream::{next_activity_sequence, touch_workstream},
};

/// The D16 host schema version and sole fresh/current production boundary.
pub const D16_HOST_SCHEMA_VERSION: i64 = 13;
/// The D17 host schema version. It is understood only by the explicit,
/// currently dormant cutover migration; ordinary D16 opens continue to reject
/// it as a future schema until the replacement application boundary exists.
pub const D17_HOST_SCHEMA_VERSION: i64 = 14;
/// The only legacy host schema accepted by the confirmed-cutover migration and
/// its exact fixture.
pub const D16_SCHEMA_12_VERSION: i64 = 12;

pub const TRANSITION_LOCK_FILE: &str = "transition.lock";
pub const PROVISIONAL_LOCK_FILE: &str = "provisional.lock";
const PROVISIONAL_LOCK_FORMAT: &str = "wsnav-provisional-lock-v1";
const MAX_PROVISIONAL_LOCK_BYTES: usize = 512;
pub const LEGACY_CLIENT_DATABASE_FILE: &str = "client.sqlite";
pub const LEGACY_CLIENT_DATABASE_WAL_FILE: &str = "client.sqlite-wal";
pub const LEGACY_CLIENT_DATABASE_SHM_FILE: &str = "client.sqlite-shm";
pub const OBSERVER_HANDOVER_JOURNAL_FILE: &str = "d16-observer-handover.json";
pub const OBSERVER_HANDOVER_JOURNAL_TEMP_FILE: &str = "d16-observer-handover.json.tmp";
pub const OBSERVER_HANDOVER_ACTIVATION_ACK_FILE: &str = "d16-observer-handover.ack";
pub const OBSERVER_HANDOVER_ACTIVATION_ACK_TEMP_FILE: &str = "d16-observer-handover.ack.tmp";
const MAX_OBSERVER_HANDOVER_ACTIVATION_ACK_BYTES: usize = 4096;
const OBSERVER_DEGRADED_MARKER_TEMP_SUFFIX: &str = ".tmp";
const MIGRATION_BUDGET: Duration = Duration::from_millis(500);
const MAX_PROJECT_REFRESH_MEMBERS: usize = 256;
const MAX_PROJECT_PROJECTION_PROJECTS: usize = 512;
const MAX_PROJECT_PROJECTION_LOCATIONS: usize = 4096;
/// The final outer margin reserved for generation-scoped observer degraded
/// marker recording after a bounded observer write has failed.
pub const OBSERVER_DEGRADED_MARKER_BUDGET: Duration = Duration::from_millis(250);

/// D16's schema-13 additions. The base schema is the exact schema-12 SQL
/// retained in `schema.rs`; this fragment is the only new durable state,
/// including generation/session/message identities needed for `OpenCode`
/// settled-event deduplication.
pub const HOST_SCHEMA_13_PROJECT_SQL: &str = "
    CREATE TABLE projects (
        project_id TEXT PRIMARY KEY,
        label_location_id TEXT NOT NULL REFERENCES project_locations(location_id),
        display_name TEXT NOT NULL,
        repository_fingerprint TEXT,
        revision INTEGER NOT NULL CHECK (revision > 0)
    );
    CREATE UNIQUE INDEX project_repository_fingerprint_idx
        ON projects(repository_fingerprint)
        WHERE repository_fingerprint IS NOT NULL AND repository_fingerprint != '';
    ALTER TABLE project_locations ADD COLUMN project_id TEXT REFERENCES projects(project_id);
    CREATE TABLE opencode_settled_messages (
        settled_message_id INTEGER PRIMARY KEY AUTOINCREMENT,
        runtime_id TEXT NOT NULL REFERENCES runtimes(runtime_id),
        runtime_generation TEXT NOT NULL,
        native_session_id TEXT NOT NULL,
        message_id TEXT NOT NULL,
        UNIQUE(runtime_id, runtime_generation, native_session_id, message_id)
    );
    CREATE INDEX opencode_settled_messages_runtime_idx
        ON opencode_settled_messages(runtime_id, runtime_generation,
                                      native_session_id, settled_message_id);
";

/// D17's schema-14 cutover fragment. The later atomic Navigator replacement
/// consumes the pending lock metadata before any actor creates or recognizes
/// `provisional.lock`; the dormant capability-journal columns retain only
/// bounded verifier/digest references, never provider command or shell data.
pub const HOST_SCHEMA_14_ONBOARDING_SQL: &str = "
    DROP TABLE project_browser_settings;
    CREATE TABLE host_operational_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        provisional_lease_generation INTEGER NOT NULL CHECK (provisional_lease_generation > 0),
        provisional_lock_phase TEXT NOT NULL
            CHECK (provisional_lock_phase IN ('pending', 'ready')),
        provisional_lock_device INTEGER,
        provisional_lock_inode INTEGER,
        CHECK (
            (provisional_lock_phase = 'pending'
                AND provisional_lock_device IS NULL
                AND provisional_lock_inode IS NULL)
            OR
            (provisional_lock_phase = 'ready'
                AND provisional_lock_device IS NOT NULL
                AND provisional_lock_inode IS NOT NULL)
        )
    );
    ALTER TABLE compound_operations ADD COLUMN launch_token_id TEXT;
    ALTER TABLE compound_operations ADD COLUMN launch_token_verifier TEXT;
    ALTER TABLE compound_operations ADD COLUMN launch_token_expiry_monotonic INTEGER;
    ALTER TABLE compound_operations ADD COLUMN launch_claims_digest TEXT;
    CREATE UNIQUE INDEX compound_operations_launch_token_id_idx
        ON compound_operations(launch_token_id)
        WHERE launch_token_id IS NOT NULL;
";

/// Returns the exact schema-12 fixture used by D16 migration tests.  It is a
/// borrowed view of the authoritative pre-D16 schema and performs no I/O.
#[must_use]
pub const fn exact_schema_12_fixture_sql() -> &'static str {
    HOST_SCHEMA_SQL
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum D16OpenMode {
    CurrentOnly,
    /// Schema-14 is open for D17-specific, lease-bound onboarding only. It
    /// deliberately cannot convert into the schema-13 `HostRegistry` surface.
    D17Current,
    ObserverTransition,
    CutoverTransition,
    FreshCreate,
    ConfirmedCutover,
}

/// Recovery outcomes intentionally carry only closed categories.  In
/// particular, they do not include paths, provider payloads, or process data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRecoveryReason {
    MissingHostDatabase,
    MalformedSchema,
    UnsupportedLegacySchema,
    UnknownFreshRootArtifact,
    NonPrivateFreshRoot,
    NonPrivateTransitionLease,
    LockedTransitionLease,
    TransitionLeasePresent,
    ForeignTransitionLease,
    ProvisionalLockPresent,
    InvalidObserverJournal,
    ObserverJournalPresent,
    LegacyClientArtifact,
    ContendedObserverDatabase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshRootClassification {
    Absent,
    Empty,
    TransitionLeaseOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshRootRejection {
    NotDirectory,
    NonCanonicalDirectory,
    NonPrivateDirectory,
    UnknownArtifact,
    NonPrivateTransitionLease,
    ForeignTransitionLease,
    NonRegularTransitionLease,
    LockedTransitionLease,
    IoError,
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

/// An exclusive, nonblocking lease for one exact state root.  The held file
/// descriptor is retained for the lifetime of this value; callers cannot
/// accidentally pass a lease acquired for another root into cutover or
/// handover mutation.
pub struct TransitionLease {
    root: PathBuf,
    lock_path: PathBuf,
    root_identity: FileIdentity,
    lock_identity: FileIdentity,
    file: nix::fcntl::Flock<File>,
}

impl std::fmt::Debug for TransitionLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransitionLease")
            .field("root", &"<private>")
            .field("lock_path", &"<private>")
            .finish_non_exhaustive()
    }
}

impl TransitionLease {
    /// Acquires an existing private transition lock for `root`.
    pub fn acquire(root: &Path) -> Result<Self, StateError> {
        let (root, root_identity) = validate_transition_root(root)?;
        let lock_path = root.join(TRANSITION_LOCK_FILE);
        let lock_metadata = fs::symlink_metadata(&lock_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StateError::TransitionLeaseRequired
            } else {
                StateError::io(&lock_path, error)
            }
        })?;
        if !lock_metadata.is_file() || !is_private_owner_file(&lock_metadata) {
            return Err(StateError::InvalidTransitionLease);
        }
        let file = open_private_transition_file(&lock_path, false)?;
        let lock_identity = file_identity(
            &file
                .metadata()
                .map_err(|error| StateError::io(&lock_path, error))?,
        );
        if lock_identity != file_identity(&lock_metadata) {
            return Err(StateError::InvalidTransitionLease);
        }
        let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
            .map_err(|(_file, _error)| {
                StateError::StateRecoveryRequired(StateRecoveryReason::LockedTransitionLease)
            })?;
        let lease = Self {
            root,
            lock_path,
            root_identity,
            lock_identity,
            file,
        };
        lease.revalidate(lease.root.as_path())?;
        Ok(lease)
    }

    fn create_for_fresh_root(root: &Path) -> Result<Self, StateError> {
        let (root, root_identity) = validate_transition_root(root)?;
        let lock_path = root.join(TRANSITION_LOCK_FILE);
        let file = open_private_transition_file(&lock_path, true)?;
        let lock_identity = file_identity(
            &file
                .metadata()
                .map_err(|error| StateError::io(&lock_path, error))?,
        );
        let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
            .map_err(|(_file, _error)| {
                StateError::StateRecoveryRequired(StateRecoveryReason::LockedTransitionLease)
            })?;
        let lease = Self {
            root,
            lock_path,
            root_identity,
            lock_identity,
            file,
        };
        lease.revalidate(lease.root.as_path())?;
        Ok(lease)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Revalidates the held root and lock identity before a caller performs a
    /// cross-module mutation.  The lease is intentionally still immutable;
    /// this only proves that the originally acquired root and lock have not
    /// been replaced.
    pub(crate) fn revalidate_for_mutation(&self, requested_root: &Path) -> Result<(), StateError> {
        self.revalidate(requested_root)
    }

    fn revalidate(&self, requested_root: &Path) -> Result<(), StateError> {
        let requested_root = fs::canonicalize(requested_root)
            .map_err(|error| StateError::io(requested_root, error))?;
        if requested_root != self.root {
            return Err(StateError::TransitionLeaseRootMismatch);
        }
        let root_metadata =
            fs::symlink_metadata(&self.root).map_err(|error| StateError::io(&self.root, error))?;
        let lock_metadata = fs::symlink_metadata(&self.lock_path)
            .map_err(|error| StateError::io(&self.lock_path, error))?;
        if !root_metadata.is_dir()
            || !is_private_owner_directory(&root_metadata)
            || file_identity(&root_metadata) != self.root_identity
            || !lock_metadata.is_file()
            || !is_private_owner_file(&lock_metadata)
            || file_identity(&lock_metadata) != self.lock_identity
        {
            return Err(StateError::InvalidTransitionLease);
        }
        let opened_metadata = self
            .file
            .metadata()
            .map_err(|error| StateError::io(&self.lock_path, error))?;
        if !opened_metadata.is_file()
            || !is_private_owner_file(&opened_metadata)
            || file_identity(&opened_metadata) != self.lock_identity
        {
            return Err(StateError::InvalidTransitionLease);
        }
        Ok(())
    }

    fn require_root(&self, root: &Path) -> Result<(), StateError> {
        self.revalidate(root)
    }
}

/// The retained, nonblocking exclusive lease for D17's one host-wide
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

    /// Revalidates the held root, inode, and lock contents before a D17
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

/// A state connection opened through one explicit D16 mode. Conversion back
/// to the existing `HostRegistry` is explicit and repeats the mode, schema,
/// artifact, and (for transition handles) lease checks at that boundary.
pub struct D16State {
    connection: Connection,
    root: PathBuf,
    mode: D16OpenMode,
}

/// The bounded state-side proof needed before a cutover process adapter
/// corroborates an `OpenCode` observer. The Runtime row, exact
/// generation-bound handle, and current provider binding are returned
/// together so a later caller cannot accidentally pair an observer with a
/// different Runtime's cwd, pane PID, process birth, or native session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeObserverProjection {
    pub runtime: RuntimeRecord,
    pub handle: OpenCodeRuntimeHandle,
    pub binding: ProviderBinding,
}

/// A private, bounded capture used by the later application refresh adapter.
/// Repository paths are intentionally crate-visible only: they are needed to
/// run one explicit repository inspection, but must never enter a public
/// Project or Location snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "The application adapter consumes this private bounded capture."
)]
pub(crate) struct ProjectRefreshCaptureMember {
    pub(crate) location_id: LocationId,
    pub(crate) repository_path: PathBuf,
    pub(crate) expected_revision: Revision,
}

/// The exact Project revision and complete Location membership captured before
/// bounded repository inspection. The transaction seam rejects any stale
/// member or changed membership before applying observations.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "The application adapter consumes this private bounded capture."
)]
pub(crate) struct ProjectRefreshCapture {
    pub(crate) project_id: ProjectId,
    pub(crate) project_revision: Revision,
    pub(crate) members: Vec<ProjectRefreshCaptureMember>,
}

/// Result of atomically registering one host-local Location and creating or
/// joining its schema-13 Project. The repository path is deliberately absent
/// from this result so callers cannot accidentally expose it in snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLocationRegistration {
    pub location_id: LocationId,
    pub revision: Revision,
    pub project: ProjectRecord,
}

/// Result of registering one canonical Location together with its initial
/// external Workstream. The two rows are committed by one schema-13
/// transaction so a successful registration always has a retained source
/// anchor for later independent Workstream creation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLocationWorkstreamRegistration {
    pub location_id: LocationId,
    pub revision: Revision,
    pub project: ProjectRecord,
    pub workstream: ExternalWorkstream,
}

/// Complete private input for one dormant D17 broker preparation.  It is
/// deliberately crate-visible only: all paths and shell/process evidence stay
/// within the broker/helper boundary and never enter a public snapshot.
#[derive(Clone)]
#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
pub(crate) struct OnboardingPrepareRequest {
    pub(crate) request_key: String,
    pub(crate) presentation_id: Uuid,
    pub(crate) presentation_revision: Revision,
    pub(crate) slot_generation: Uuid,
    pub(crate) candidate_runtime_id: RuntimeId,
    pub(crate) runtime_paths: RuntimePaths,
    pub(crate) provider: ProviderKind,
    pub(crate) repository: RepositoryRegistration,
    pub(crate) shell_cwd: PathBuf,
    pub(crate) shell_pid: u32,
    pub(crate) shell_birth: String,
    pub(crate) shell_process_group: u32,
    pub(crate) shell_session: u32,
    pub(crate) argv_digest: String,
    pub(crate) boot_provenance: String,
    pub(crate) now_monotonic_millis: i64,
    pub(crate) expiry_monotonic_millis: i64,
}

impl std::fmt::Debug for OnboardingPrepareRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OnboardingPrepareRequest")
            .field("request_key", &"<private>")
            .field("presentation", &"<opaque>")
            .field("slot_generation", &"<opaque>")
            .field("candidate_runtime_id", &"<opaque>")
            .field("provider", &self.provider)
            .field("repository", &"<private>")
            .field("shell", &"<private>")
            .finish_non_exhaustive()
    }
}

/// A newly issued broker handoff.  The live capability remains in memory and
/// is deliberately not copied into the operation, runtime, or snapshot.
#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
pub(crate) struct OnboardingReservation {
    operation_id: OperationId,
    location_id: LocationId,
    workstream_id: WorkstreamId,
    runtime: RuntimeRecord,
    capability: LaunchCapability,
}

impl std::fmt::Debug for OnboardingReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OnboardingReservation")
            .field("operation_id", &"<opaque>")
            .field("location_id", &"<opaque>")
            .field("workstream_id", &"<opaque>")
            .field("runtime", &self.runtime)
            .field("capability", &self.capability)
            .finish()
    }
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
impl OnboardingReservation {
    #[must_use]
    pub(crate) const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    #[must_use]
    pub(crate) const fn location_id(&self) -> LocationId {
        self.location_id
    }

    #[must_use]
    pub(crate) const fn workstream_id(&self) -> WorkstreamId {
        self.workstream_id
    }

    #[must_use]
    pub(crate) fn runtime(&self) -> &RuntimeRecord {
        &self.runtime
    }

    #[must_use]
    pub(crate) fn capability(&self) -> &LaunchCapability {
        &self.capability
    }

    /// Transfers the live capability only to the crate-private D17 broker
    /// channel. Durable state never receives this value.
    pub(crate) fn into_capability(self) -> LaunchCapability {
        self.capability
    }
}

/// A request-key replay that found the one existing unresolved onboarding
/// journal.  It never reissues the lost live token or creates another graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    clippy::struct_field_names,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
pub(crate) struct ExistingOnboardingReservation {
    pub(crate) operation_id: OperationId,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_id: RuntimeId,
}

#[allow(
    dead_code,
    clippy::large_enum_variant,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
pub(crate) enum OnboardingPreparation {
    Issued(OnboardingReservation),
    Existing(ExistingOnboardingReservation),
}

/// The only state-side result of an exact helper capability consumption. It
/// establishes durable Runtime ownership but deliberately does not grant
/// attach/action authority or imply provider exec proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    clippy::struct_field_names,
    reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
)]
pub(crate) struct OnboardingOwnership {
    pub(crate) operation_id: OperationId,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_id: RuntimeId,
    pub(crate) operation_revision: Revision,
}

impl std::fmt::Debug for OnboardingPreparation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Issued(reservation) => formatter
                .debug_tuple("OnboardingPreparation::Issued")
                .field(reservation)
                .finish(),
            Self::Existing(existing) => formatter
                .debug_tuple("OnboardingPreparation::Existing")
                .field(existing)
                .finish(),
        }
    }
}

/// The bounded, non-secret part of an onboarding request retained in the
/// operation journal.  Paths, shell identities, and the live token stay out
/// of this structure; their exact commitment is the capability claim digest.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
struct PersistedOnboardingIntent {
    version: u8,
    presentation_id: Uuid,
    presentation_revision: Revision,
    slot_generation: Uuid,
    lease_generation: i64,
    candidate_runtime_id: RuntimeId,
    provider: ProviderKind,
    location_id: LocationId,
    workstream_id: WorkstreamId,
    runtime_generation: String,
    registry_generation: String,
    argv_digest: String,
    boot_provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProjectLocationRegistrationInner {
    registration: ProjectLocationRegistration,
    workstream: Option<ExternalWorkstream>,
}

/// The host-private browser-root revision returned by a successful CAS update.
/// The configured path remains in `SQLite` and is never returned by this API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectBrowserRootRevision {
    pub revision: Revision,
}

impl std::fmt::Debug for D16State {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("D16State")
            .field("root", &"<private>")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl D16State {
    #[must_use]
    pub const fn mode(&self) -> D16OpenMode {
        self.mode
    }

    fn ensure_current_only_artifacts(&self) -> Result<(), StateError> {
        if self.mode == D16OpenMode::CurrentOnly {
            reject_current_only_artifacts(&self.root, true)?;
        }
        Ok(())
    }

    /// Returns the exact state-root spelling retained by this handle.  The
    /// path is used only for revalidation; it is never included
    /// in public projections or provider-facing state.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads `PRAGMA user_version` without changing the database.
    pub fn schema_version(&self) -> Result<i64, StateError> {
        schema_version(&self.connection)
    }

    /// Converts a validated schema-13 D16 handle into the existing host
    /// registry without opening, creating, or migrating another connection.
    /// Current-only conversion additionally repeats the clean-artifact check;
    /// transition-bound handles use [`Self::into_host_registry_under_lease`]
    /// when their exact lease is still present.
    pub fn into_host_registry(self) -> Result<HostRegistry, StateError> {
        if self.mode == D16OpenMode::ObserverTransition
            || self.mode == D16OpenMode::CutoverTransition
            || self.mode == D16OpenMode::D17Current
        {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        if !validate_d16_host_database_path(&self.root.join("host.sqlite"))? {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::MissingHostDatabase,
            ));
        }
        validate_schema13(&self.connection)?;
        if self.mode == D16OpenMode::CurrentOnly {
            reject_current_only_artifacts(&self.root, true)?;
        } else {
            reject_schema13_conversion_artifacts(&self.root, false)?;
        }
        let Self { connection, .. } = self;
        Ok(HostRegistry { connection })
    }

    /// Lease-bound conversion for a schema-13 handle opened during the
    /// confirmed transition.  The transition lock is allowed only when this
    /// exact held lease is revalidated immediately before conversion.
    pub fn into_host_registry_under_lease(
        self,
        lease: &TransitionLease,
    ) -> Result<HostRegistry, StateError> {
        if !matches!(
            self.mode,
            D16OpenMode::FreshCreate
                | D16OpenMode::ConfirmedCutover
                | D16OpenMode::CutoverTransition
        ) {
            return self.into_host_registry();
        }
        lease.revalidate_for_mutation(&self.root)?;
        if !validate_d16_host_database_path(&self.root.join("host.sqlite"))? {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::MissingHostDatabase,
            ));
        }
        validate_schema13(&self.connection)?;
        reject_schema13_conversion_artifacts(&self.root, true)?;
        lease.revalidate_for_mutation(&self.root)?;
        let Self { connection, .. } = self;
        Ok(HostRegistry { connection })
    }

    /// Returns deterministic host-local Project rows with their complete
    /// bounded Location membership.  No repository or Git inspection occurs.
    pub fn project_projections(&self) -> Result<Vec<ProjectProjection>, StateError> {
        ensure_project_projection_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        load_project_projections(&self.connection)
    }

    /// Alias for callers that want to name the returned rows explicitly as a
    /// Project/Location projection rather than a Project inventory.
    pub fn project_location_projections(&self) -> Result<Vec<ProjectProjection>, StateError> {
        self.project_projections()
    }

    /// Reads all schema-13 Projects in deterministic opaque-ID order and
    /// validates every persisted label source before returning any row.
    pub fn projects(&self) -> Result<Vec<ProjectRecord>, StateError> {
        ensure_project_projection_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        query_projects(&self.connection)
    }

    /// Reads a single exact Project by opaque ID.
    pub fn project(&self, project_id: ProjectId) -> Result<Option<ProjectRecord>, StateError> {
        ensure_project_projection_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        let project = self
            .connection
            .query_row(
                "SELECT project_id, label_location_id, display_name,
                        repository_fingerprint, revision
                 FROM projects WHERE project_id = ?1",
                [project_id.to_string()],
                row_to_project,
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        Ok(project)
    }

    /// Captures the selected Project's complete membership and private
    /// repository paths in one bounded read transaction. The returned paths
    /// are crate-visible only and are intended solely for the explicit Git
    /// inspection that precedes [`Self::refresh_project`].
    #[allow(
        dead_code,
        reason = "The application adapter consumes this private bounded capture."
    )]
    pub(crate) fn capture_project_refresh(
        &self,
        project_id: ProjectId,
    ) -> Result<ProjectRefreshCapture, StateError> {
        ensure_project_mutation_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let project = transaction
            .query_row(
                "SELECT project_id, label_location_id, display_name,
                        repository_fingerprint, revision
                 FROM projects WHERE project_id = ?1",
                [project_id.to_string()],
                row_to_project,
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        validate_project_source_transaction(&transaction, &project)?;
        let members = load_project_members(&transaction, project_id)?;
        if members.is_empty() || members.len() > MAX_PROJECT_REFRESH_MEMBERS {
            return Err(StateError::MalformedHostSchema);
        }
        let members = members
            .into_iter()
            .map(|member| ProjectRefreshCaptureMember {
                location_id: member.location_id,
                repository_path: PathBuf::from(member.repository_path),
                expected_revision: member.revision,
            })
            .collect();
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(ProjectRefreshCapture {
            project_id,
            project_revision: project.revision,
            members,
        })
    }

    /// Returns the currently captured browser-root revision. The path itself
    /// remains host-private and is never included in a D16 projection.
    pub fn project_browser_root_revision(&self) -> Result<Revision, StateError> {
        ensure_project_mutation_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        load_project_browser_root_revision(&self.connection)
    }

    /// Updates the host-private browser root using an exact revision CAS. The
    /// path is resolved and validated before the transaction; only the new
    /// opaque revision crosses this state boundary.
    pub fn set_project_browser_root(
        &mut self,
        expected_revision: Revision,
        root_path: &str,
    ) -> Result<ProjectBrowserRootRevision, StateError> {
        ensure_project_mutation_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        let root = resolve_project_browser_root(root_path)?;
        let root = fs::canonicalize(root).map_err(|_| StateError::ProjectBrowserRootUnavailable)?;
        if !root.is_dir() {
            return Err(StateError::ProjectBrowserRootUnavailable);
        }
        let root = root.to_str().ok_or(StateError::InvalidProjectBrowserRoot)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let current = load_project_browser_root_revision(&transaction)?;
        if current != expected_revision {
            return Err(StateError::ConcurrentWrite);
        }
        let changed = transaction
            .execute(
                "INSERT INTO project_browser_settings (singleton, root_path, revision)
                 VALUES (1, ?1, 1)
                 ON CONFLICT(singleton) DO UPDATE SET
                   root_path = excluded.root_path,
                   revision = project_browser_settings.revision + 1
                 WHERE project_browser_settings.revision = ?2",
                params![root, expected_revision.value()],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        let revision = load_project_browser_root_revision(&transaction)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(ProjectBrowserRootRevision { revision })
    }

    /// Alias retained for callers that use update-oriented naming at the
    /// application boundary.
    pub fn update_project_browser_root(
        &mut self,
        expected_revision: Revision,
        root_path: &str,
    ) -> Result<ProjectBrowserRootRevision, StateError> {
        self.set_project_browser_root(expected_revision, root_path)
    }

    /// Registers one canonical host-local repository root and atomically
    /// creates or joins its schema-13 Project. No Workstream or provider
    /// Runtime is created by this presentation registration seam.
    pub fn register_project_location(
        &mut self,
        repository_path: &Path,
        display_name: &str,
        repository_fingerprint: Option<&str>,
        remote_identity_display: Option<&str>,
        id_generator: &dyn IdGenerator,
    ) -> Result<ProjectLocationRegistration, StateError> {
        Ok(self
            .register_project_location_inner(
                repository_path,
                display_name,
                repository_fingerprint,
                remote_identity_display,
                None,
                id_generator,
            )?
            .registration)
    }

    /// Registers one canonical repository root, its schema-13 Project, and
    /// the initial external Workstream in one transaction. The Workstream is
    /// deliberately unstarted; it is the retained source anchor for a later
    /// explicit provider action.
    #[allow(
        clippy::too_many_arguments,
        reason = "The registration boundary keeps the complete repository and provider evidence explicit."
    )]
    pub fn register_project_location_with_initial_workstream(
        &mut self,
        repository_path: &Path,
        display_name: &str,
        repository_fingerprint: Option<&str>,
        remote_identity_display: Option<&str>,
        provider: ProviderKind,
        id_generator: &dyn IdGenerator,
    ) -> Result<ProjectLocationWorkstreamRegistration, StateError> {
        let inner = self.register_project_location_inner(
            repository_path,
            display_name,
            repository_fingerprint,
            remote_identity_display,
            Some(provider),
            id_generator,
        )?;
        let workstream = inner.workstream.ok_or(StateError::MalformedHostSchema)?;
        Ok(ProjectLocationWorkstreamRegistration {
            location_id: inner.registration.location_id,
            revision: inner.registration.revision,
            project: inner.registration.project,
            workstream,
        })
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "The state-owned transaction receives every captured registration field explicitly."
    )]
    fn register_project_location_inner(
        &mut self,
        repository_path: &Path,
        display_name: &str,
        repository_fingerprint: Option<&str>,
        remote_identity_display: Option<&str>,
        provider: Option<ProviderKind>,
        id_generator: &dyn IdGenerator,
    ) -> Result<ProjectLocationRegistrationInner, StateError> {
        ensure_project_mutation_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        validate_project_display_name(display_name)?;
        let fingerprint = repository_fingerprint.filter(|value| !value.is_empty());
        validate_repository_fingerprint(fingerprint)?;
        validate_safe_origin_display(remote_identity_display)?;
        let repository_path = repository_path
            .to_str()
            .ok_or(StateError::InvalidPersistedValue(
                "repository path is not UTF-8".to_owned(),
            ))?;
        validate_registry_text("repository path", repository_path)?;
        if !Path::new(repository_path).is_absolute() {
            return Err(StateError::InvalidPersistedValue(
                "repository path is not absolute".to_owned(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let duplicate: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM project_locations WHERE repository_path = ?1
                 )",
                [repository_path],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if duplicate {
            return Err(StateError::ConcurrentWrite);
        }
        let location_id = LocationId::from(id_generator.uuid());
        let remote_identity_display = remote_identity_display.unwrap_or_default();
        transaction
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display,
                    revision, project_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, NULL)",
                params![
                    location_id.to_string(),
                    repository_path,
                    display_name,
                    fingerprint,
                    remote_identity_display,
                ],
            )
            .map_err(StateError::Sqlite)?;

        let project = if let Some(fingerprint) = fingerprint {
            if let Some(existing) = find_project_by_fingerprint(&transaction, fingerprint)? {
                bump_project_revision(&transaction, existing.project_id)?;
                transaction
                    .execute(
                        "UPDATE project_locations SET project_id = ?1
                         WHERE location_id = ?2",
                        params![existing.project_id.to_string(), location_id.to_string()],
                    )
                    .map_err(StateError::Sqlite)?;
                existing
            } else {
                let created = create_project(
                    &transaction,
                    location_id,
                    display_name,
                    Some(fingerprint),
                    id_generator,
                )?;
                transaction
                    .execute(
                        "UPDATE project_locations SET project_id = ?1
                         WHERE location_id = ?2",
                        params![created.project_id.to_string(), location_id.to_string()],
                    )
                    .map_err(StateError::Sqlite)?;
                created
            }
        } else {
            let created =
                create_project(&transaction, location_id, display_name, None, id_generator)?;
            transaction
                .execute(
                    "UPDATE project_locations SET project_id = ?1
                     WHERE location_id = ?2",
                    params![created.project_id.to_string(), location_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            created
        };
        let project = transaction
            .query_row(
                "SELECT project_id, label_location_id, display_name,
                        repository_fingerprint, revision
                 FROM projects WHERE project_id = ?1",
                [project.project_id.to_string()],
                row_to_project,
            )
            .map_err(StateError::Sqlite)?;
        let workstream = provider.map(|provider| {
            (
                ExternalWorkstream {
                    location_id,
                    workstream_id: WorkstreamId::from(id_generator.uuid()),
                },
                provider,
            )
        });
        if let Some((workstream, provider)) = &workstream {
            let activity_sequence = next_activity_sequence(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO workstreams (
                        workstream_id, location_id, provider, origin,
                        source_workstream_id, lifecycle, archived_at_millis,
                        last_activity_sequence, last_activity_at_millis, revision
                     ) VALUES (?1, ?2, ?3, 'external', NULL, 'open', NULL, ?4, 0, 1)",
                    params![
                        workstream.workstream_id.to_string(),
                        workstream.location_id.to_string(),
                        provider.as_str(),
                        activity_sequence,
                    ],
                )
                .map_err(StateError::Sqlite)?;
        }
        validate_project_membership_transaction(&transaction)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(ProjectLocationRegistrationInner {
            registration: ProjectLocationRegistration {
                location_id,
                revision: Revision::INITIAL,
                project,
            },
            workstream: workstream.map(|(workstream, _)| workstream),
        })
    }

    /// Applies a complete, captured Project refresh atomically.  The caller
    /// supplies the selected Project revision, every member in opaque-ID
    /// order, each member's expected Location revision, and bounded observer
    /// evidence.  No repository or client files are read by this seam.
    pub fn refresh_project(
        &mut self,
        input: &ProjectRefreshInput,
        id_generator: &dyn IdGenerator,
    ) -> Result<ProjectRefreshOutcome, StateError> {
        ensure_project_mutation_mode(self.mode)?;
        self.ensure_current_only_artifacts()?;
        validate_schema13(&self.connection)?;
        input.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let selected = transaction
            .query_row(
                "SELECT project_id, label_location_id, display_name,
                        repository_fingerprint, revision
                 FROM projects WHERE project_id = ?1",
                [input.selected_project_id.to_string()],
                row_to_project,
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        validate_project_source_transaction(&transaction, &selected)?;
        if selected.revision != input.selected_project_revision {
            return Err(StateError::ConcurrentWrite);
        }
        let database_members = load_project_members(&transaction, input.selected_project_id)?;
        if database_members.len() != input.members.len()
            || database_members
                .iter()
                .map(|member| member.location_id)
                .ne(input.members.iter().map(|member| member.location_id))
        {
            return Err(StateError::ConcurrentWrite);
        }
        for (database_member, captured_member) in database_members.iter().zip(&input.members) {
            if database_member.revision != captured_member.expected_revision {
                return Err(StateError::ConcurrentWrite);
            }
        }
        for member in &input.members {
            reconcile_location_in_transaction(&transaction, member, id_generator)?;
        }
        validate_project_membership_transaction(&transaction)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        let projects = query_projects(&self.connection)?;
        let selected_project = projects
            .iter()
            .find(|project| project.project_id == input.selected_project_id)
            .cloned();
        Ok(ProjectRefreshOutcome {
            selected_project,
            projects,
        })
    }

    /// Runs one bounded observer write.  This method is intentionally limited
    /// to the transition bridge and does not expose Project or presentation
    /// operations through observer mode.
    pub fn observer_write<T, F>(
        &mut self,
        deadline: ObserverDatabaseDeadline,
        mut operation: F,
    ) -> Result<T, StateError>
    where
        F: FnMut(&Connection) -> Result<T, rusqlite::Error>,
    {
        if self.mode != D16OpenMode::ObserverTransition {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        deadline
            .run(|| operation(&self.connection))
            .map_err(|error| match error {
                ObserverDatabaseError::DeadlineExceeded => {
                    StateError::ObserverDatabaseDeadlineExceeded
                }
                ObserverDatabaseError::Sqlite(error) => StateError::Sqlite(error),
            })
    }

    /// Runs one observer-transition operation against this handle's actual
    /// `SQLite` connection and records the exact generation-scoped degraded
    /// marker on bounded contention or a non-retryable write failure.  The
    /// operation remains a narrow state-owned closure until the provider
    /// adapter supplies typed lifecycle/binding/attention calls;
    /// it cannot accidentally open a second registry connection.
    pub fn observer_write_with_degraded_marker<T, F>(
        &mut self,
        runtime_id: RuntimeId,
        runtime_generation: &str,
        deadline: ObserverDatabaseDeadline,
        mut operation: F,
    ) -> Result<T, StateError>
    where
        F: FnMut(&Connection) -> Result<T, rusqlite::Error>,
    {
        if self.mode != D16OpenMode::ObserverTransition {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        let root = self.root.clone();
        run_observer_write_with_degraded_marker(
            &root,
            runtime_id,
            runtime_generation,
            deadline,
            || operation(&self.connection),
        )
    }

    /// Runs one narrow observer-transition write and leaves a generation
    /// scoped degraded marker when bounded `SQLite` work cannot complete. The
    /// marker is written only after this handle has already proved the
    /// observer-transition mode and exact runtime generation.
    fn observer_transition_write<T, F>(
        &mut self,
        runtime_id: RuntimeId,
        runtime_generation: &str,
        deadline: ObserverDatabaseDeadline,
        operation: F,
    ) -> Result<T, StateError>
    where
        F: FnMut(&Connection) -> Result<T, rusqlite::Error>,
    {
        let root = self.root.clone();
        match self.observer_write(deadline, operation) {
            Ok(value) => {
                clear_observer_degraded_marker(&root, runtime_id, runtime_generation)?;
                Ok(value)
            }
            Err(StateError::ObserverDatabaseDeadlineExceeded) => {
                write_observer_degraded_marker(
                    &root,
                    runtime_id,
                    runtime_generation,
                    ObserverDegradedReason::BusyDeadline,
                )?;
                Err(StateError::ObserverDatabaseDeadlineExceeded)
            }
            // A compare-and-swap miss is an expected stale-observer outcome,
            // not a database failure.  In particular, do not write a
            // degraded marker for a replacement generation that won the
            // race while this observer was validating its evidence.
            Err(StateError::Sqlite(rusqlite::Error::QueryReturnedNoRows)) => {
                Err(StateError::ConcurrentWrite)
            }
            // State-owned semantic validation can happen inside the same
            // bounded closure as its transaction. Preserve that typed error
            // across the existing rusqlite-only retry seam instead of
            // treating malformed authority as a stale compare-and-swap.
            Err(StateError::Sqlite(rusqlite::Error::ToSqlConversionFailure(error))) => {
                match error.downcast::<StateError>() {
                    Ok(error) => Err(*error),
                    Err(error) => {
                        write_observer_degraded_marker(
                            &root,
                            runtime_id,
                            runtime_generation,
                            ObserverDegradedReason::CommitFailed,
                        )?;
                        Err(StateError::Sqlite(rusqlite::Error::ToSqlConversionFailure(
                            error,
                        )))
                    }
                }
            }
            Err(StateError::Sqlite(error)) => {
                write_observer_degraded_marker(
                    &root,
                    runtime_id,
                    runtime_generation,
                    ObserverDegradedReason::CommitFailed,
                )?;
                Err(StateError::Sqlite(error))
            }
            Err(error) => Err(error),
        }
    }

    /// Promotes one exact D16 observer helper from `starting` to `ready`.
    /// This operation is available only through the schema-12/13 observer
    /// transition mode and never opens the ordinary host registry.
    pub fn observer_mark_opencode_ready(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: Revision,
        observer_pid: u32,
        observer_birth: &str,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<OpenCodeRuntimeHandle, StateError> {
        if observer_pid == 0 {
            return Err(StateError::InvalidRegistryField("observer PID"));
        }
        validate_registry_text("observer birth", observer_birth)?;
        let handle =
            self.observer_transition_write(runtime_id, generation, deadline, |connection| {
                let transaction = connection.unchecked_transaction()?;
                let changed = transaction.execute(
                    "UPDATE opencode_runtime_handles SET observer_status = 'ready',
                        revision = revision + 1
                     WHERE runtime_id = ?1 AND runtime_generation = ?2
                       AND observer_status = 'starting' AND observer_pid = ?3
                       AND observer_birth = ?4 AND revision = ?5",
                    params![
                        runtime_id.to_string(),
                        generation,
                        i64::from(observer_pid),
                        observer_birth,
                        expected_revision.value(),
                    ],
                )?;
                if changed != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                let handle = load_opencode_handle(&transaction, runtime_id)
                    .map_err(Self::state_error_as_sqlite)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                transaction.commit()?;
                Ok(handle)
            })?;
        Ok(handle)
    }

    /// Marks one exact D16 observer generation unknown after corroboration
    /// fails. A stale PID/birth/revision never marks a replacement helper.
    pub fn observer_mark_opencode_unknown(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: Revision,
        observer_pid: u32,
        observer_birth: &str,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<(), StateError> {
        if observer_pid == 0 {
            return Err(StateError::InvalidRegistryField("observer PID"));
        }
        validate_registry_text("observer birth", observer_birth)?;
        self.observer_transition_write(runtime_id, generation, deadline, |connection| {
            let transaction = connection.unchecked_transaction()?;
            let changed = transaction.execute(
                "UPDATE opencode_runtime_handles SET observer_status = 'unknown',
                        revision = revision + 1
                     WHERE runtime_id = ?1 AND runtime_generation = ?2
                       AND observer_pid = ?3 AND observer_birth = ?4 AND revision = ?5",
                params![
                    runtime_id.to_string(),
                    generation,
                    i64::from(observer_pid),
                    observer_birth,
                    expected_revision.value(),
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// Applies one already-correlated `OpenCode` lifecycle observation through
    /// the narrow observer-transition authority. The first read validates
    /// provider/session/PID/birth/revision evidence before the bounded writer
    /// is entered; a racing revision fails closed without assigning a marker.
    pub fn observer_apply_opencode_lifecycle_observation(
        &mut self,
        runtime_id: RuntimeId,
        observation: &super::models::OpenCodeLifecycleObservation,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<Revision, StateError> {
        if observation.session.provider() != ProviderKind::OpenCode || observation.observer_pid == 0
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("observer birth", &observation.observer_birth)?;
        let activity_at_millis = match &observation.hint {
            LifecycleHint::Working | LifecycleHint::Settled { .. } => {
                Some(SystemClock.now_millis()?)
            }
            LifecycleHint::Started | LifecycleHint::Ended => None,
        };
        let accepted = self.observer_transition_write(
            runtime_id,
            &observation.generation,
            deadline,
            |connection| {
                let transaction = connection.unchecked_transaction()?;
                let (lifecycle, workstream_id) =
                    validate_opencode_observation(&transaction, runtime_id, observation)
                        .map_err(Self::state_error_as_sqlite)?;
                let accepted = apply_opencode_lifecycle_transition(
                    &transaction,
                    runtime_id,
                    observation.runtime_revision,
                    &lifecycle,
                    workstream_id,
                    observation,
                )
                .map_err(Self::state_error_as_sqlite)?;
                if accepted {
                    let workstream_id = workstream_id.to_string();
                    touch_workstream(&transaction, &workstream_id, activity_at_millis)
                        .map_err(Self::state_error_as_sqlite)?;
                }
                transaction.commit()?;
                Ok(accepted)
            },
        )?;
        // A duplicate settled message is a successful no-op at the same
        // Runtime revision; a new hint advances the revision as before.
        Ok(if accepted {
            observation.runtime_revision.next()
        } else {
            observation.runtime_revision
        })
    }

    /// Reads one exact Runtime through the observer-transition handle. This
    /// surface deliberately joins the Workstream provider and rejects a
    /// cross-provider row before returning evidence to a provider adapter.
    pub fn observer_runtime_by_id(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        if self.mode != D16OpenMode::ObserverTransition {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let runtime = transaction
            .query_row(
                "SELECT runtimes.runtime_id, runtimes.provider,
                        runtimes.tmux_generation, runtimes.tmux_session,
                        runtimes.cwd, runtimes.provider_pid,
                        runtimes.process_birth, runtimes.lifecycle,
                        runtimes.revision, runtimes.workstream_id
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1
                   AND runtimes.provider = workstreams.provider",
                [runtime_id.to_string()],
                |row| {
                    let workstream_id: String = row.get(9)?;
                    let workstream_id = Uuid::parse_str(&workstream_id).map_err(to_sql_error)?;
                    row_to_runtime(row, WorkstreamId::from(workstream_id))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(runtime)
    }

    /// Returns only exact process-fingerprinted Runtime rows eligible for a
    /// Codex hook. The process and private-tmux corroboration remains outside
    /// this state read; this method supplies no mutation authority.
    pub fn observer_hook_runtime_candidates(&self) -> Result<Vec<RuntimeRecord>, StateError> {
        if self.mode != D16OpenMode::ObserverTransition {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT runtimes.runtime_id, runtimes.provider,
                        runtimes.tmux_generation, runtimes.tmux_session,
                        runtimes.cwd, runtimes.provider_pid,
                        runtimes.process_birth, runtimes.lifecycle,
                        runtimes.revision, runtimes.workstream_id
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.lifecycle IN ('starting', 'idle', 'working', 'attention')
                   AND runtimes.provider_pid IS NOT NULL
                   AND runtimes.process_birth IS NOT NULL
                   AND runtimes.provider = workstreams.provider",
            )
            .map_err(StateError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                let workstream_id: String = row.get(9)?;
                let workstream_id = Uuid::parse_str(&workstream_id).map_err(to_sql_error)?;
                row_to_runtime(row, WorkstreamId::from(workstream_id))
            })
            .map_err(StateError::Sqlite)?;
        rows.map(|row| row.map_err(StateError::Sqlite)).collect()
    }

    /// Reads one exact `OpenCode` observer handle through the transition
    /// connection. No ordinary `HostRegistry` connection is opened.
    pub fn observer_opencode_runtime_handle(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<OpenCodeRuntimeHandle>, StateError> {
        if self.mode != D16OpenMode::ObserverTransition {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let handle = load_opencode_handle(&transaction, runtime_id)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(handle)
    }

    /// Reads only the exact current settled-message identity needed to
    /// reconcile a standby observer's ordered buffer with mutations already
    /// committed by the frozen predecessor. No event payload is read or
    /// retained.
    pub(crate) fn observer_last_settled_turn_id(
        &self,
        runtime_id: RuntimeId,
        generation: &str,
        native_session_id: &ProviderSessionId,
    ) -> Result<Option<String>, StateError> {
        if self.mode != D16OpenMode::ObserverTransition {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema,
            ));
        }
        if native_session_id.provider() != ProviderKind::OpenCode {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("runtime generation", generation)?;
        let settled = self
            .connection
            .query_row(
                "SELECT last_settled_turn_id FROM provider_bindings
                 WHERE runtime_id = ?1 AND provider = 'opencode'
                   AND native_session_id = ?2 AND runtime_generation = ?3",
                params![
                    runtime_id.to_string(),
                    native_session_id.native_id(),
                    generation,
                ],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::HookEvidenceMismatch)?;
        if let Some(message_id) = settled.as_deref() {
            validate_provider_metadata(message_id)?;
        }
        Ok(settled)
    }

    /// Records bounded Codex thread-name metadata through the observer
    /// transition authority. The exact Runtime generation and native session
    /// are part of the compare target; no prompt, response, or turn payload is
    /// persisted.
    pub fn observer_record_thread_metadata(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        native_session_id: &ProviderSessionId,
        name: Option<&str>,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<(), StateError> {
        if native_session_id.provider() != ProviderKind::Codex {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("runtime generation", generation)?;
        let (name, name_state) = match name.filter(|value| !value.trim().is_empty()) {
            Some(name) => {
                validate_registry_text("thread name", name)?;
                (Some(name), "named")
            }
            None => (None, "known_empty"),
        };
        self.observer_transition_write(runtime_id, generation, deadline, |connection| {
            let transaction = connection.unchecked_transaction()?;
            let changed = transaction.execute(
                "UPDATE provider_bindings SET observed_thread_name = ?1,
                         name_state = ?2, revision = revision + 1
                     WHERE runtime_id = ?3 AND provider = 'codex'
                       AND native_session_id = ?4 AND runtime_generation = ?5
                       AND EXISTS (
                           SELECT 1 FROM runtimes
                           WHERE runtime_id = ?3 AND tmux_generation = ?5
                       )",
                params![
                    name,
                    name_state,
                    runtime_id.to_string(),
                    native_session_id.native_id(),
                    generation,
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// Applies one exact Codex hook observation through the narrow D16
    /// observer-transition authority. All `SQLite` work, including validation,
    /// is inside the bounded generation-scoped write so a busy/locked
    /// database records the closed degraded marker before returning.
    pub fn observer_apply_codex_lifecycle_observation(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        observation: &LifecycleObservation,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<(), StateError> {
        validate_registry_text("runtime generation", generation)?;
        let activity_at_millis = match observation.event {
            LifecycleEvent::UserPromptSubmit | LifecycleEvent::Stop => {
                Some(SystemClock.now_millis()?)
            }
            LifecycleEvent::SessionStart | LifecycleEvent::SessionEnd => None,
        };
        self.observer_transition_write(runtime_id, generation, deadline, |connection| {
            let transaction = connection.unchecked_transaction()?;
            let runtime = transaction
                .query_row(
                    "SELECT runtimes.workstream_id, runtimes.provider,
                                runtimes.tmux_generation, runtimes.cwd,
                                runtimes.lifecycle, runtimes.revision,
                                workstreams.provider, workstreams.lifecycle
                         FROM runtimes
                         JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                         WHERE runtimes.runtime_id = ?1",
                    [runtime_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .optional()
                .map_err(StateError::Sqlite)
                .map_err(Self::state_error_as_sqlite)?
                .ok_or_else(|| {
                    Self::state_error_as_sqlite(StateError::UnknownRuntime(runtime_id))
                })?;
            let workstream_id = Uuid::parse_str(&runtime.0)
                .map(WorkstreamId::from)
                .map_err(StateError::InvalidPersistedUuid)
                .map_err(Self::state_error_as_sqlite)?;
            let provider = super::utils::provider_kind_from_text(&runtime.1)
                .map_err(Self::state_error_as_sqlite)?;
            let workstream_provider = super::utils::provider_kind_from_text(&runtime.6)
                .map_err(Self::state_error_as_sqlite)?;
            if provider != ProviderKind::Codex
                || workstream_provider != ProviderKind::Codex
                || provider != workstream_provider
                || runtime.2 != generation
                || runtime.3 != observation.cwd
            {
                return Err(Self::state_error_as_sqlite(
                    StateError::HookEvidenceMismatch,
                ));
            }
            let runtime_revision = Revision::try_from(runtime.5)
                .map_err(|error| Self::state_error_as_sqlite(StateError::Domain(error)))?;
            let workstream_lifecycle =
                workstream_lifecycle_from_text(&runtime.7).map_err(Self::state_error_as_sqlite)?;
            let existing =
                load_binding(&transaction, runtime_id).map_err(Self::state_error_as_sqlite)?;
            let observed_session =
                ProviderSessionId::new(provider, observation.native_session_id.clone())
                    .map_err(|error| Self::state_error_as_sqlite(StateError::Domain(error)))?;
            apply_lifecycle_event(
                LifecycleEventContext {
                    transaction: &transaction,
                    runtime_id,
                    provider,
                    runtime_status: &runtime.4,
                    runtime_revision,
                    generation,
                    workstream_id,
                    workstream_lifecycle,
                    existing,
                    observed_session,
                },
                observation,
            )
            .map_err(Self::state_error_as_sqlite)?;
            touch_workstream(&transaction, &runtime.0, activity_at_millis)
                .map_err(Self::state_error_as_sqlite)?;
            transaction.commit()?;
            Ok(())
        })
    }

    fn state_error_as_sqlite(error: StateError) -> rusqlite::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }

    /// Performs the explicit, lease-bound schema-12 to schema-13 step after
    /// any required observer handover.  A schema-13 connection is validated
    /// idempotently and is not rewritten.
    pub fn migrate_schema12_to13(
        &mut self,
        lease: &TransitionLease,
        id_generator: &dyn IdGenerator,
    ) -> Result<(), StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        lease.revalidate_for_mutation(&self.root)?;
        if !validate_d16_host_database_path(&self.root.join("host.sqlite"))? {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::MissingHostDatabase,
            ));
        }
        match schema_version(&self.connection)? {
            D16_SCHEMA_12_VERSION => {
                migrate_schema12_to13(&mut self.connection, id_generator, lease)?;
            }
            D16_HOST_SCHEMA_VERSION => validate_schema13(&self.connection)?,
            0..=11 => {
                return Err(StateError::HostStateResetRequired(schema_version(
                    &self.connection,
                )?));
            }
            value if value > D16_HOST_SCHEMA_VERSION => {
                return Err(StateError::UnsupportedFutureHostSchema(value));
            }
            _ => return Err(StateError::MalformedHostSchema),
        }
        validate_schema13(&self.connection)?;
        lease.revalidate_for_mutation(&self.root)
    }

    /// Performs the explicit, lease-bound schema-13 to schema-14 groundwork
    /// step for D17's later atomic Navigator cutover.
    ///
    /// This method is intentionally not reachable from an ordinary command:
    /// D16 continues to own the active schema-13 UI and state path until its
    /// shell-first replacement is complete. The migration only removes the
    /// obsolete browser settings table and records pending provisional-lock
    /// installation metadata; it never creates or adopts `provisional.lock`.
    ///
    /// # Errors
    ///
    /// Returns an error when the caller lacks the exact transition lease, the
    /// database is not an intact schema-13 root, or a pre-schema-14
    /// provisional-lock artifact is present.
    pub fn migrate_schema13_to14(&mut self, lease: &TransitionLease) -> Result<(), StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        lease.revalidate_for_mutation(&self.root)?;
        if !validate_d16_host_database_path(&self.root.join("host.sqlite"))? {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::MissingHostDatabase,
            ));
        }
        match schema_version(&self.connection)? {
            D16_HOST_SCHEMA_VERSION => {
                reject_pre_schema14_provisional_lock(&self.root)?;
                migrate_schema13_to14(&mut self.connection, lease)?;
            }
            D17_HOST_SCHEMA_VERSION => validate_schema14(&self.connection)?,
            0..=12 => {
                return Err(StateError::HostStateResetRequired(schema_version(
                    &self.connection,
                )?));
            }
            value if value > D17_HOST_SCHEMA_VERSION => {
                return Err(StateError::UnsupportedFutureHostSchema(value));
            }
            _ => return Err(StateError::MalformedHostSchema),
        }
        validate_schema14(&self.connection)?;
        lease.revalidate_for_mutation(&self.root)
    }

    /// Installs or acquires the exact schema-14 stable provisional lease.
    ///
    /// The pending metadata written by [`Self::migrate_schema13_to14`] is
    /// finalized only after an owner-only, no-follow, `CLOEXEC` file has been
    /// written and synced. A ready lock is never recreated: missing,
    /// replaced, malformed, or busy evidence fails closed.
    ///
    /// This remains a dormant cutover seam. No current D16 command invokes
    /// it, and it never materializes a shell or launches a provider.
    ///
    /// # Errors
    ///
    /// Returns an error when schema-14 metadata, the exact lock artifact, or
    /// the held transition lease cannot prove one current owner.
    pub fn install_or_acquire_provisional_lease(
        &mut self,
        transition_lease: &TransitionLease,
    ) -> Result<ProvisionalLease, StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        transition_lease.revalidate_for_mutation(&self.root)?;
        validate_schema14(&self.connection)?;
        let metadata = load_provisional_lock_metadata(&self.connection)?;
        let (root, root_identity) = validate_transition_root(&self.root)
            .map_err(|_| StateError::InvalidProvisionalLease)?;
        let lock_path = root.join(PROVISIONAL_LOCK_FILE);
        let expected_contents = provisional_lock_contents(&metadata.host_id, metadata.generation)?;
        let (file, lock_identity) = match metadata.phase {
            ProvisionalLockPhase::Pending => {
                let file = match exact_artifact_metadata(&lock_path)? {
                    None => {
                        let mut file = open_private_provisional_file(&lock_path, true)?;
                        file.write_all(&expected_contents)
                            .map_err(|error| StateError::io(&lock_path, error))?;
                        file.sync_all()
                            .map_err(|error| StateError::io(&lock_path, error))?;
                        sync_directory(&root)?;
                        file
                    }
                    Some(_) => open_private_provisional_file(&lock_path, false)?,
                };
                let identity =
                    validate_provisional_lock_file(&file, &lock_path, &expected_contents)?;
                (file, identity)
            }
            ProvisionalLockPhase::Ready { expected_identity } => {
                let file = open_private_provisional_file(&lock_path, false)?;
                let identity =
                    validate_provisional_lock_file(&file, &lock_path, &expected_contents)?;
                if identity != expected_identity {
                    return Err(StateError::InvalidProvisionalLease);
                }
                (file, identity)
            }
        };
        let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
            .map_err(|(_file, _error)| StateError::ProvisionalLeaseBusy)?;
        let provisional = ProvisionalLease::new(
            root,
            root_identity,
            lock_path,
            lock_identity,
            metadata.generation,
            expected_contents,
            file,
        );
        provisional.revalidate_for_mutation(&self.root)?;
        if matches!(metadata.phase, ProvisionalLockPhase::Pending) {
            transition_lease.revalidate_for_mutation(&self.root)?;
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(StateError::Sqlite)?;
            let changed = transaction
                .execute(
                    "UPDATE host_operational_metadata
                     SET provisional_lock_phase = 'ready',
                         provisional_lock_device = ?1,
                         provisional_lock_inode = ?2
                     WHERE singleton = 1
                       AND provisional_lease_generation = ?3
                       AND provisional_lock_phase = 'pending'",
                    params![
                        i64::try_from(lock_identity.device)
                            .map_err(|_| StateError::InvalidProvisionalLease)?,
                        i64::try_from(lock_identity.inode)
                            .map_err(|_| StateError::InvalidProvisionalLease)?,
                        metadata.generation,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            if changed != 1 {
                return Err(StateError::ConcurrentWrite);
            }
            validate_schema14(&transaction)?;
            transaction.commit().map_err(StateError::Sqlite)?;
            transition_lease.revalidate_for_mutation(&self.root)?;
            provisional.revalidate_for_mutation(&self.root)?;
        }
        Ok(provisional)
    }

    /// Installs or acquires D17's stable provisional lease from a normal
    /// schema-14 opening. The same pending-to-ready protocol is used as the
    /// cutover seam, but no schema-13 transition lease can authorize this
    /// post-migration D17 operation.
    ///
    /// The returned descriptor remains `CLOEXEC`, locked, and bound to the
    /// exact root/inode/generation. No marker, tmux server, Runtime, or
    /// provider process is created here.
    pub fn acquire_d17_provisional_lease(&mut self) -> Result<ProvisionalLease, StateError> {
        ensure_d17_current_mode(self.mode)?;
        validate_schema14(&self.connection)?;
        let metadata = load_provisional_lock_metadata(&self.connection)?;
        let (root, root_identity) = validate_transition_root(&self.root)
            .map_err(|_| StateError::InvalidProvisionalLease)?;
        let lock_path = root.join(PROVISIONAL_LOCK_FILE);
        let expected_contents = provisional_lock_contents(&metadata.host_id, metadata.generation)?;
        let (file, lock_identity) = match metadata.phase {
            ProvisionalLockPhase::Pending => {
                let file = match exact_artifact_metadata(&lock_path)? {
                    None => {
                        let mut file = open_private_provisional_file(&lock_path, true)?;
                        file.write_all(&expected_contents)
                            .map_err(|error| StateError::io(&lock_path, error))?;
                        file.sync_all()
                            .map_err(|error| StateError::io(&lock_path, error))?;
                        sync_directory(&root)?;
                        file
                    }
                    Some(_) => open_private_provisional_file(&lock_path, false)?,
                };
                let identity =
                    validate_provisional_lock_file(&file, &lock_path, &expected_contents)?;
                (file, identity)
            }
            ProvisionalLockPhase::Ready { expected_identity } => {
                let file = open_private_provisional_file(&lock_path, false)?;
                let identity =
                    validate_provisional_lock_file(&file, &lock_path, &expected_contents)?;
                if identity != expected_identity {
                    return Err(StateError::InvalidProvisionalLease);
                }
                (file, identity)
            }
        };
        let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
            .map_err(|(_file, _error)| StateError::ProvisionalLeaseBusy)?;
        let provisional = ProvisionalLease::new(
            root,
            root_identity,
            lock_path,
            lock_identity,
            metadata.generation,
            expected_contents,
            file,
        );
        provisional.revalidate_for_mutation(&self.root)?;
        if matches!(metadata.phase, ProvisionalLockPhase::Pending) {
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(StateError::Sqlite)?;
            let changed = transaction
                .execute(
                    "UPDATE host_operational_metadata
                     SET provisional_lock_phase = 'ready',
                         provisional_lock_device = ?1,
                         provisional_lock_inode = ?2
                     WHERE singleton = 1
                       AND provisional_lease_generation = ?3
                       AND provisional_lock_phase = 'pending'",
                    params![
                        i64::try_from(lock_identity.device)
                            .map_err(|_| StateError::InvalidProvisionalLease)?,
                        i64::try_from(lock_identity.inode)
                            .map_err(|_| StateError::InvalidProvisionalLease)?,
                        metadata.generation,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            if changed != 1 {
                return Err(StateError::ConcurrentWrite);
            }
            validate_schema14(&transaction)?;
            transaction.commit().map_err(StateError::Sqlite)?;
            provisional.revalidate_for_mutation(&self.root)?;
        }
        Ok(provisional)
    }

    /// Transactionally reserves the D17 Project/Location/Workstream/Runtime
    /// graph for one marker-owned candidate and records a verifier-backed
    /// handoff.  This is intentionally a dormant cutover seam: it requires
    /// both the migration lease and the stable provisional lease, creates no
    /// marker or tmux artifact, and never launches a provider.
    #[allow(
        dead_code,
        reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn prepare_d17_onboarding(
        &mut self,
        transition_lease: &TransitionLease,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        id_generator: &dyn IdGenerator,
    ) -> Result<OnboardingPreparation, StateError> {
        self.prepare_d17_onboarding_authorized(
            D17OnboardingAuthority::Cutover(transition_lease),
            provisional_lease,
            request,
            id_generator,
        )
    }

    /// Transactionally reserves the D17 graph from a normal schema-14
    /// opening. The caller must retain the exact D17 provisional lease; a
    /// pre-schema-14 migration lease cannot authorize this path.
    ///
    /// This remains an unreachable broker seam. It creates no marker, tmux
    /// artifact, Runtime process, or provider process.
    #[allow(
        dead_code,
        reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn prepare_d17_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        id_generator: &dyn IdGenerator,
    ) -> Result<OnboardingPreparation, StateError> {
        self.prepare_d17_onboarding_authorized(
            D17OnboardingAuthority::Current,
            provisional_lease,
            request,
            id_generator,
        )
    }

    fn prepare_d17_onboarding_authorized(
        &mut self,
        authority: D17OnboardingAuthority<'_>,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        id_generator: &dyn IdGenerator,
    ) -> Result<OnboardingPreparation, StateError> {
        let previous_busy_timeout = self
            .connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .map_err(StateError::Sqlite)?;
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(StateError::Sqlite)?;
        let preparation = self.prepare_d17_onboarding_with_zero_timeout(
            authority,
            provisional_lease,
            request,
            id_generator,
        );
        let restore = self.connection.busy_timeout(Duration::from_millis(
            u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
        ));
        match (preparation, restore) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(StateError::Sqlite(error)),
            (Ok(preparation), Ok(())) => Ok(preparation),
        }
    }

    #[allow(
        dead_code,
        clippy::too_many_lines,
        reason = "the single transaction keeps every onboarding authority transition auditable"
    )]
    fn prepare_d17_onboarding_with_zero_timeout(
        &mut self,
        authority: D17OnboardingAuthority<'_>,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        id_generator: &dyn IdGenerator,
    ) -> Result<OnboardingPreparation, StateError> {
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema14(&self.connection)?;
        validate_onboarding_prepare_request(request, &self.root)?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let registry_generation = load_registry_generation(&transaction)?;

        if let Some(existing) = load_existing_onboarding_preparation(
            &transaction,
            request,
            provisional_lease.lease_generation(),
            &registry_generation,
            &self.root,
        )? {
            transaction.commit().map_err(StateError::Sqlite)?;
            authority.revalidate(self.mode, &self.root)?;
            provisional_lease.revalidate_for_mutation(&self.root)?;
            return Ok(OnboardingPreparation::Existing(existing));
        }

        let candidate_exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runtimes WHERE runtime_id = ?1)",
                [request.candidate_runtime_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if candidate_exists {
            return Err(StateError::InvalidOnboardingPreparation);
        }

        let existing_location = load_location_for_repository_path(
            &transaction,
            request.repository.project_root.as_path(),
        )?;
        let location_id = existing_location.map_or_else(
            || LocationId::from(id_generator.uuid()),
            |location| location.location_id,
        );
        let operation_id = OperationId::from(id_generator.uuid());
        let workstream_id = WorkstreamId::from(id_generator.uuid());
        let runtime_generation = id_generator.uuid().to_string();
        validate_registry_text("runtime generation", &runtime_generation)?;
        let intent = PersistedOnboardingIntent {
            version: 1,
            presentation_id: request.presentation_id,
            presentation_revision: request.presentation_revision,
            slot_generation: request.slot_generation,
            lease_generation: provisional_lease.lease_generation(),
            candidate_runtime_id: request.candidate_runtime_id,
            provider: request.provider,
            location_id,
            workstream_id,
            runtime_generation: runtime_generation.clone(),
            registry_generation: registry_generation.clone(),
            argv_digest: request.argv_digest.clone(),
            boot_provenance: request.boot_provenance.clone(),
        };
        let expected_revisions_json =
            serde_json::to_string(&intent).map_err(|_| StateError::InvalidOnboardingPreparation)?;
        let claims = onboarding_claims(
            operation_id,
            location_id,
            &runtime_generation,
            &registry_generation,
            provisional_lease.lease_generation(),
            request,
        )?;
        let capability = LaunchCapability::issue(
            &claims,
            request.now_monotonic_millis,
            request.expiry_monotonic_millis,
            id_generator,
        )
        .map_err(|_| StateError::InvalidOnboardingPreparation)?;
        let mut operation = CompoundOperation::with_id(
            operation_id,
            request.request_key.clone(),
            OperationKind::Onboard,
            expected_revisions_json,
        )?;
        operation.transition_onboarding(OnboardingPhase::CapabilityIssued, None, None)?;
        operation.launch_token_id = Some(capability.metadata().token_id().to_owned());
        operation.launch_token_verifier = Some(capability.metadata().verifier().to_owned());
        operation.launch_token_expiry_monotonic =
            Some(capability.metadata().expiry_monotonic_millis());
        operation.launch_claims_digest = Some(capability.metadata().claims_digest().to_owned());

        if existing_location.is_none() {
            insert_onboarding_location(&transaction, request, location_id, id_generator)?;
        }
        let activity_sequence = next_activity_sequence(&transaction)?;
        transaction
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin, source_workstream_id,
                    lifecycle, archived_at_millis, last_activity_sequence,
                    last_activity_at_millis, revision
                 ) VALUES (?1, ?2, ?3, 'independent', NULL, 'open', NULL, ?4, 0, 1)",
                params![
                    workstream_id.to_string(),
                    location_id.to_string(),
                    request.provider.as_str(),
                    activity_sequence,
                ],
            )
            .map_err(StateError::Sqlite)?;
        let runtime = RuntimeRecord {
            runtime_id: request.candidate_runtime_id,
            workstream_id,
            provider: request.provider,
            tmux_generation: runtime_generation,
            tmux_session: request.runtime_paths.session_name.clone(),
            cwd: request.repository.project_root.clone(),
            provider_pid: None,
            process_birth: None,
            status: RuntimeStatus::Starting,
            revision: Revision::INITIAL,
        };
        transaction
            .execute(
                "INSERT INTO runtimes (
                    runtime_id, workstream_id, provider, tmux_generation, tmux_session,
                    cwd, provider_pid, process_birth, lifecycle, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, 'starting', 1)",
                params![
                    runtime.runtime_id.to_string(),
                    runtime.workstream_id.to_string(),
                    runtime.provider.as_str(),
                    runtime.tmux_generation,
                    runtime.tmux_session,
                    runtime.cwd.to_string_lossy(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO compound_operations (
                    operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision,
                    launch_token_id, launch_token_verifier,
                    launch_token_expiry_monotonic, launch_claims_digest
                 ) VALUES (?1, ?2, 'onboard', 'capability_issued', ?3,
                    NULL, NULL, ?4, ?5, ?6, ?7, ?8)",
                params![
                    operation.id.to_string(),
                    operation.request_key,
                    operation.expected_revisions_json,
                    operation.revision.value(),
                    operation.launch_token_id,
                    operation.launch_token_verifier,
                    operation.launch_token_expiry_monotonic,
                    operation.launch_claims_digest,
                ],
            )
            .map_err(StateError::Sqlite)?;
        validate_project_membership_transaction(&transaction)?;
        validate_schema14(&transaction)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(OnboardingPreparation::Issued(OnboardingReservation {
            operation_id,
            location_id,
            workstream_id,
            runtime,
            capability,
        }))
    }

    /// Atomically consumes one revalidated D17 launch capability and records
    /// the durable Runtime-owned launch fence.  The caller is responsible for
    /// marker/process/cwd proof before this seam; this state transition does
    /// not launch, attach, signal, or otherwise contact a provider.
    #[allow(
        dead_code,
        reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn consume_d17_onboarding(
        &mut self,
        transition_lease: &TransitionLease,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        token: &str,
        now_monotonic_millis: i64,
    ) -> Result<OnboardingOwnership, StateError> {
        self.consume_d17_onboarding_authorized(
            D17OnboardingAuthority::Cutover(transition_lease),
            provisional_lease,
            request,
            token,
            now_monotonic_millis,
        )
    }

    /// Atomically consumes one D17 launch capability from a normal
    /// schema-14 opening. The provisional lease remains the only mutable
    /// shell-slot authority; provider execution remains outside this seam.
    #[allow(
        dead_code,
        reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn consume_d17_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        token: &str,
        now_monotonic_millis: i64,
    ) -> Result<OnboardingOwnership, StateError> {
        self.consume_d17_onboarding_authorized(
            D17OnboardingAuthority::Current,
            provisional_lease,
            request,
            token,
            now_monotonic_millis,
        )
    }

    fn consume_d17_onboarding_authorized(
        &mut self,
        authority: D17OnboardingAuthority<'_>,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        token: &str,
        now_monotonic_millis: i64,
    ) -> Result<OnboardingOwnership, StateError> {
        let previous_busy_timeout = self
            .connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .map_err(StateError::Sqlite)?;
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(StateError::Sqlite)?;
        let ownership = self.consume_d17_onboarding_with_zero_timeout(
            authority,
            provisional_lease,
            request,
            token,
            now_monotonic_millis,
        );
        let restore = self.connection.busy_timeout(Duration::from_millis(
            u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
        ));
        match (ownership, restore) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(StateError::Sqlite(error)),
            (Ok(ownership), Ok(())) => Ok(ownership),
        }
    }

    #[allow(
        dead_code,
        clippy::too_many_lines,
        reason = "the single transaction keeps the one-shot ownership boundary auditable"
    )]
    fn consume_d17_onboarding_with_zero_timeout(
        &mut self,
        authority: D17OnboardingAuthority<'_>,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        token: &str,
        now_monotonic_millis: i64,
    ) -> Result<OnboardingOwnership, StateError> {
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema14(&self.connection)?;
        validate_onboarding_prepare_request(request, &self.root)?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let registry_generation = load_registry_generation(&transaction)?;
        let existing = load_existing_onboarding_preparation(
            &transaction,
            request,
            provisional_lease.lease_generation(),
            &registry_generation,
            &self.root,
        )?
        .ok_or_else(|| StateError::MissingOperation(request.request_key.clone()))?;
        let persisted: (String, String, i64, String, String, i64) = transaction
            .query_row(
                "SELECT launch_token_id, launch_token_verifier,
                        launch_token_expiry_monotonic, launch_claims_digest,
                        expected_revisions_json, revision
                 FROM compound_operations
                 WHERE operation_id = ?1 AND kind = 'onboard'
                   AND phase = 'capability_issued'",
                [existing.operation_id.to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .map_err(StateError::Sqlite)?;
        let intent: PersistedOnboardingIntent =
            serde_json::from_str(&persisted.4).map_err(|_| StateError::MalformedHostSchema)?;
        let metadata = LaunchCapabilityMetadata::from_persisted(
            persisted.0,
            persisted.1,
            persisted.2,
            persisted.3,
        )
        .map_err(|_| StateError::MalformedHostSchema)?;
        let claims = onboarding_claims(
            existing.operation_id,
            existing.location_id,
            &intent.runtime_generation,
            &registry_generation,
            provisional_lease.lease_generation(),
            request,
        )?;
        verify_launch_capability(token, &metadata, &claims, now_monotonic_millis)
            .map_err(map_onboarding_capability_error)?;
        OnboardingPhase::CapabilityIssued.transition(OnboardingPhase::RuntimeOwnedLaunching)?;
        let operation_revision = Revision::try_from(persisted.5)?;
        let next_revision = next_revision(operation_revision)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = 'runtime_owned_launching', revision = ?1
                 WHERE operation_id = ?2 AND kind = 'onboard'
                   AND phase = 'capability_issued' AND revision = ?3",
                params![
                    next_revision.value(),
                    existing.operation_id.to_string(),
                    operation_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        validate_schema14(&transaction)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(OnboardingOwnership {
            operation_id: existing.operation_id,
            location_id: existing.location_id,
            workstream_id: existing.workstream_id,
            runtime_id: existing.runtime_id,
            operation_revision: next_revision,
        })
    }

    /// Records the helper's durable provider-preparation fence after exact
    /// Runtime ownership has committed. This changes only the bounded D17
    /// journal; it neither invokes nor proves a provider.
    #[allow(
        dead_code,
        reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn record_d17_provider_preparation_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_d17_onboarding_current(
            provisional_lease,
            ownership,
            OnboardingPhase::ProviderPreparation,
        )
    }

    /// Records the point at which provider-specific preparation may have an
    /// external effect. The caller must record this before making that effect;
    /// this state seam itself does not contact a provider.
    #[allow(
        dead_code,
        reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn record_d17_provider_external_effect_started_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_d17_onboarding_current(
            provisional_lease,
            ownership,
            OnboardingPhase::ProviderExternalEffectStarted,
        )
    }

    /// Records the final durable boundary immediately before the helper would
    /// execute the native provider. It intentionally does not expose an
    /// unproven Runtime to ordinary attachment or action authority.
    #[allow(
        dead_code,
        reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn record_d17_provider_exec_started_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_d17_onboarding_current(
            provisional_lease,
            ownership,
            OnboardingPhase::ProviderExecStarted,
        )
    }

    /// Advances one exact Runtime-owned D17 journal through a pre-exec fence.
    /// Provider-exec proof, known absence, rollback, and recovery each need
    /// their own evidence-bearing reconciler APIs and cannot use this generic
    /// mutation seam.
    fn advance_d17_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
        next: OnboardingPhase,
    ) -> Result<OnboardingOwnership, StateError> {
        let previous_busy_timeout = self
            .connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .map_err(StateError::Sqlite)?;
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(StateError::Sqlite)?;
        let advanced =
            self.advance_d17_onboarding_with_zero_timeout(provisional_lease, ownership, next);
        let restore = self.connection.busy_timeout(Duration::from_millis(
            u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
        ));
        match (advanced, restore) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(StateError::Sqlite(error)),
            (Ok(ownership), Ok(())) => Ok(ownership),
        }
    }

    fn advance_d17_onboarding_with_zero_timeout(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
        next: OnboardingPhase,
    ) -> Result<OnboardingOwnership, StateError> {
        ensure_d17_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema14(&self.connection)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        ensure_d17_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let persisted: Option<(String, String, String, i64)> = transaction
            .query_row(
                "SELECT kind, phase, expected_revisions_json, revision
                 FROM compound_operations WHERE operation_id = ?1",
                [ownership.operation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((kind, phase, encoded_intent, revision)) = persisted else {
            return Err(StateError::UnknownOperation(ownership.operation_id));
        };
        if kind != "onboard" {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let intent: PersistedOnboardingIntent =
            serde_json::from_str(&encoded_intent).map_err(|_| StateError::MalformedHostSchema)?;
        if intent.version != 1
            || intent.location_id != ownership.location_id
            || intent.workstream_id != ownership.workstream_id
            || intent.candidate_runtime_id != ownership.runtime_id
        {
            return Err(StateError::MalformedHostSchema);
        }
        let persisted_revision = Revision::try_from(revision)?;
        if persisted_revision != ownership.operation_revision {
            return Err(StateError::ConcurrentWrite);
        }
        let runtime: Option<(String, String, String)> = transaction
            .query_row(
                "SELECT runtimes.workstream_id, workstreams.location_id, runtimes.tmux_generation
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1",
                [ownership.runtime_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((workstream_id, location_id, runtime_generation)) = runtime else {
            return Err(StateError::MalformedHostSchema);
        };
        if workstream_id != ownership.workstream_id.to_string()
            || location_id != ownership.location_id.to_string()
            || runtime_generation != intent.runtime_generation
        {
            return Err(StateError::MalformedHostSchema);
        }
        let current = OnboardingPhase::from_operation_phase(
            operation_phase_from_text(&phase).map_err(|_| StateError::MalformedHostSchema)?,
        )
        .ok_or(StateError::MalformedHostSchema)?;
        current.transition(next)?;
        let next_revision = next_revision(persisted_revision)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = ?1, revision = ?2
                 WHERE operation_id = ?3 AND kind = 'onboard'
                   AND phase = ?4 AND revision = ?5",
                params![
                    operation_phase_text(next.operation_phase()),
                    next_revision.value(),
                    ownership.operation_id.to_string(),
                    phase,
                    persisted_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        validate_schema14(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(OnboardingOwnership {
            operation_revision: next_revision,
            ..ownership
        })
    }

    /// Lists only deterministic, current `OpenCode` observer handles whose
    /// Runtime lifecycle is itself non-stopped. Runtime IDs are ordered by
    /// their opaque persisted spelling and handles are provider/generation/
    /// binding validated before exposure.
    pub fn live_opencode_observers(&self) -> Result<Vec<OpenCodeRuntimeHandle>, StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let mut statement = transaction
            .prepare(
                "SELECT runtimes.runtime_id, runtimes.lifecycle,
                        runtimes.provider, workstreams.provider
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.provider = 'opencode' ORDER BY runtimes.runtime_id",
            )
            .map_err(StateError::Sqlite)?;
        let runtime_rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        drop(statement);
        let mut handles = Vec::new();
        for (runtime_id, lifecycle, runtime_provider, workstream_provider) in runtime_rows {
            if runtime_provider != "opencode" || workstream_provider != "opencode" {
                return Err(StateError::ProviderIdentityMismatch);
            }
            if runtime_status_from_text(&lifecycle)? == RuntimeStatus::Stopped {
                continue;
            }
            let runtime_id = runtime_id
                .parse::<RuntimeId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            let Some(handle) = load_opencode_handle(&transaction, runtime_id)? else {
                continue;
            };
            if handle.observer_status != OpenCodeObserverStatus::Stopped
                && handle.observer_pid.is_some()
                && handle.observer_birth.is_some()
            {
                handles.push(handle);
            }
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(handles)
    }

    /// Returns the same live `OpenCode` observer set with its exact Runtime row
    /// attached.  This is a cutover-only proof surface; it is not a public
    /// snapshot and does not read provider payloads or process state.
    pub fn live_opencode_observer_projections(
        &self,
    ) -> Result<Vec<OpenCodeObserverProjection>, StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let mut statement = transaction
            .prepare(
                "SELECT runtimes.runtime_id, runtimes.provider,
                        runtimes.tmux_generation, runtimes.tmux_session,
                        runtimes.cwd, runtimes.provider_pid,
                        runtimes.process_birth, runtimes.lifecycle,
                        runtimes.revision, runtimes.workstream_id,
                        workstreams.provider
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.provider = 'opencode'
                 ORDER BY runtimes.runtime_id",
            )
            .map_err(StateError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                let workstream_id: String = row.get(9)?;
                let workstream_id = workstream_id
                    .parse::<WorkstreamId>()
                    .map_err(to_sql_error)?;
                let runtime = row_to_runtime(row, workstream_id)?;
                Ok((runtime, row.get::<_, String>(10)?))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        drop(statement);
        let mut projections = Vec::new();
        for (runtime, workstream_provider) in rows {
            if workstream_provider != "opencode" {
                return Err(StateError::ProviderIdentityMismatch);
            }
            if runtime.status == RuntimeStatus::Stopped {
                continue;
            }
            let Some(handle) = load_opencode_handle(&transaction, runtime.runtime_id)? else {
                continue;
            };
            if handle.observer_status == OpenCodeObserverStatus::Stopped
                || handle.observer_pid.is_none()
                || handle.observer_birth.is_none()
            {
                continue;
            }
            let binding =
                load_current_binding(&transaction, runtime.runtime_id)?.ok_or_else(|| {
                    StateError::InvalidPersistedValue(
                        "live OpenCode Runtime is missing its provider binding".to_owned(),
                    )
                })?;
            if binding.provider != crate::domain::ProviderKind::OpenCode
                || binding.native_session_id != handle.native_session_id
                || binding.runtime_generation != handle.runtime_generation
            {
                return Err(StateError::ProviderIdentityMismatch);
            }
            projections.push(OpenCodeObserverProjection {
                runtime,
                handle,
                binding,
            });
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(projections)
    }

    /// Reads the exact current observer PID/birth and handle revision for a
    /// Runtime.  This is evidence only; process corroboration remains outside
    /// the state boundary.
    pub fn current_observer(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<CurrentObserverHandleProof, StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let result = load_current_observer_proof(&transaction, runtime_id);
        transaction.commit().map_err(StateError::Sqlite)?;
        result
    }

    /// Performs one exact revision-guarded observer-handle assignment under
    /// the held transition lease.  No PID is accepted without a bounded birth
    /// identity, and one stale revision leaves the row unchanged.
    pub fn compare_and_swap_observer(
        &mut self,
        lease: &TransitionLease,
        runtime_id: RuntimeId,
        expected_revision: Revision,
        standby: &ObserverProcessIdentity,
    ) -> Result<CurrentObserverHandleProof, StateError> {
        ensure_cutover_transition_mode(self.mode)?;
        lease.revalidate_for_mutation(&self.root)?;
        standby.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let current = load_current_observer_proof(&transaction, runtime_id)?;
        if current.revision != expected_revision
            || (current.pid == standby.pid && current.birth == standby.birth)
        {
            return Err(StateError::ConcurrentWrite);
        }
        let next_revision = next_revision(expected_revision)?;
        let changed = transaction
            .execute(
                "UPDATE opencode_runtime_handles
                 SET observer_pid = ?1, observer_birth = ?2,
                     observer_status = 'ready', revision = ?3
                 WHERE runtime_id = ?4 AND runtime_generation = ?5
                   AND revision = ?6 AND observer_pid = ?7
                   AND observer_birth = ?8",
                params![
                    i64::from(standby.pid),
                    standby.birth,
                    next_revision.value(),
                    runtime_id.to_string(),
                    current.runtime_generation,
                    expected_revision.value(),
                    i64::from(current.pid),
                    current.birth,
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(CurrentObserverHandleProof {
            runtime_id,
            runtime_generation: current.runtime_generation,
            pid: standby.pid,
            birth: standby.birth.clone(),
            revision: next_revision,
        })
    }
}

/// A bounded repository observation captured outside the state transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRefreshObservation {
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub remote_identity_display: Option<String>,
}

/// One complete expected member of a Project refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRefreshMember {
    pub location_id: LocationId,
    pub expected_revision: Revision,
    pub observation: ProjectRefreshObservation,
}

/// Complete Project refresh input.  `members` must be the exact captured
/// membership set in `LocationId` order; partial batches are rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRefreshInput {
    pub selected_project_id: ProjectId,
    pub selected_project_revision: Revision,
    pub members: Vec<ProjectRefreshMember>,
}

impl ProjectRefreshInput {
    fn validate(&self) -> Result<(), StateError> {
        if self.members.is_empty() || self.members.len() > MAX_PROJECT_REFRESH_MEMBERS {
            return Err(StateError::InvalidPersistedValue(
                "invalid Project refresh membership size".to_owned(),
            ));
        }
        for pair in self.members.windows(2) {
            if pair[0].location_id >= pair[1].location_id {
                return Err(StateError::InvalidPersistedValue(
                    "Project refresh members are not in LocationId order".to_owned(),
                ));
            }
        }
        for member in &self.members {
            validate_project_display_name(&member.observation.display_name)?;
            validate_repository_fingerprint(member.observation.repository_fingerprint.as_deref())?;
            validate_safe_origin_display(member.observation.remote_identity_display.as_deref())?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRefreshOutcome {
    pub selected_project: Option<ProjectRecord>,
    pub projects: Vec<ProjectRecord>,
}

/// Persisted Project row introduced by schema 13.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecord {
    pub project_id: ProjectId,
    pub label_location_id: LocationId,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub revision: Revision,
}

/// One bounded host-local Location row used by D16 snapshots.
/// Repository paths remain private; only the safe display, validated
/// fingerprint evidence, and separate origin display are projected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLocationProjection {
    pub project_id: ProjectId,
    pub location_id: LocationId,
    pub revision: Revision,
    pub is_label_source: bool,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub origin_display: Option<String>,
}

/// One deterministic Project row and its Location membership.  Both vectors
/// are bounded and sorted by their opaque IDs before being returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectProjection {
    pub project_id: ProjectId,
    pub revision: Revision,
    pub label_location_id: LocationId,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub locations: Vec<ProjectLocationProjection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocationForProject {
    location_id: LocationId,
    repository_path: String,
    display_name: String,
    fingerprint: Option<String>,
    remote_display: String,
    revision: Revision,
    project_id: Option<ProjectId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlannedProject {
    project_id: ProjectId,
    label_location_id: LocationId,
    display_name: String,
    repository_fingerprint: Option<String>,
    locations: Vec<LocationId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProjectPlan {
    locations: Vec<LocationForProject>,
    projects: Vec<PlannedProject>,
}

/// Opens schema 13 only, read-write, and refuses exact legacy client/journal
/// artifacts.  It never creates, migrates, removes, or adopts anything.
pub fn open_current_only(root: &StateRoot) -> Result<D16State, StateError> {
    validate_state_root_directory(root.base())?;
    let path = root.host_database_path();
    let database_exists = validate_d16_host_database_path(&path)?;
    if !database_exists {
        reject_current_only_artifacts(root.base(), false)?;
        return Err(StateError::FreshStateRequired);
    }
    reject_transition_lease_artifact(root.base())?;
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    configure_d16_connection(&connection)?;
    match schema_version(&connection)? {
        D16_HOST_SCHEMA_VERSION => {
            validate_schema13(&connection)?;
            reject_current_only_artifacts(root.base(), true)?;
        }
        D16_SCHEMA_12_VERSION => return Err(StateError::CutoverRequired),
        0..=11 => {
            return Err(StateError::HostStateResetRequired(schema_version(
                &connection,
            )?));
        }
        value if value > D16_HOST_SCHEMA_VERSION => {
            return Err(StateError::UnsupportedFutureHostSchema(value));
        }
        _ => return Err(StateError::MalformedHostSchema),
    }
    Ok(D16State {
        connection,
        root: root.base().to_path_buf(),
        mode: D16OpenMode::CurrentOnly,
    })
}

/// Opens a schema-14 root for the dormant D17-specific state boundary.
///
/// This is intentionally not a compatibility path for the active D16
/// `HostRegistry`: the removed browser table and D17 onboarding columns mean
/// the regular D16 navigator must not be pointed at it. The future D17
/// application boundary acquires and revalidates `provisional.lock` before
/// every marker or onboarding mutation.
pub fn open_d17_current_only(root: &StateRoot) -> Result<D16State, StateError> {
    validate_state_root_directory(root.base())?;
    let path = root.host_database_path();
    if !validate_d16_host_database_path(&path)? {
        if exact_artifact_metadata(&root.base().join(PROVISIONAL_LOCK_FILE))?.is_some() {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::ProvisionalLockPresent,
            ));
        }
        reject_d17_current_only_artifacts(root.base())?;
        return Err(StateError::FreshStateRequired);
    }
    reject_d17_current_only_artifacts(root.base())?;
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    configure_d16_connection(&connection)?;
    match schema_version(&connection)? {
        D17_HOST_SCHEMA_VERSION => validate_schema14(&connection)?,
        D16_HOST_SCHEMA_VERSION | D16_SCHEMA_12_VERSION => return Err(StateError::CutoverRequired),
        0..=11 => {
            return Err(StateError::HostStateResetRequired(schema_version(
                &connection,
            )?));
        }
        value if value > D17_HOST_SCHEMA_VERSION => {
            return Err(StateError::UnsupportedFutureHostSchema(value));
        }
        _ => return Err(StateError::MalformedHostSchema),
    }
    Ok(D16State {
        connection,
        root: root.base().to_path_buf(),
        mode: D16OpenMode::D17Current,
    })
}

/// Opens exactly schema 12 or 13 for the provider observer bridge.  No client
/// path is inspected and no migration or host identity creation occurs.
pub fn open_observer_transition(root: &StateRoot) -> Result<D16State, StateError> {
    validate_state_root_directory(root.base())?;
    let path = root.host_database_path();
    if !validate_d16_host_database_path(&path)? {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::MissingHostDatabase,
        ));
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    configure_d16_connection(&connection)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(StateError::Sqlite)?;
    let version = schema_version(&connection)?;
    match version {
        D16_SCHEMA_12_VERSION => validate_schema12(&connection)?,
        D16_HOST_SCHEMA_VERSION => validate_schema13(&connection)?,
        0..=11 => return Err(StateError::HostStateResetRequired(version)),
        value if value > D16_HOST_SCHEMA_VERSION => {
            return Err(StateError::UnsupportedFutureHostSchema(value));
        }
        _ => return Err(StateError::MalformedHostSchema),
    }
    Ok(D16State {
        connection,
        root: root.base().to_path_buf(),
        mode: D16OpenMode::ObserverTransition,
    })
}

/// Opens a schema-12 root only through the explicit cutover entrypoint and
/// performs the transactional 12-to-13 migration.  Client files
/// are deliberately neither read nor removed in this slice.
pub fn open_cutover_transition(
    root: &StateRoot,
    lease: &TransitionLease,
) -> Result<D16State, StateError> {
    lease.revalidate_for_mutation(root.base())?;
    let path = root.host_database_path();
    if !validate_d16_host_database_path(&path)? {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::MissingHostDatabase,
        ));
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    configure_d16_connection(&connection)?;
    match schema_version(&connection)? {
        D16_SCHEMA_12_VERSION => validate_schema12(&connection)?,
        D16_HOST_SCHEMA_VERSION => validate_schema13(&connection)?,
        0..=11 => {
            return Err(StateError::HostStateResetRequired(schema_version(
                &connection,
            )?));
        }
        value if value > D16_HOST_SCHEMA_VERSION => {
            return Err(StateError::UnsupportedFutureHostSchema(value));
        }
        _ => return Err(StateError::MalformedHostSchema),
    }
    lease.revalidate_for_mutation(root.base())?;
    Ok(D16State {
        connection,
        root: root.base().to_path_buf(),
        mode: D16OpenMode::CutoverTransition,
    })
}

/// Opens a schema-12 root through the explicit cutover transition and then
/// runs its separate lease-bound migration step. Schema 13 is validated
/// idempotently and is never rewritten.
pub fn open_confirmed_cutover(
    root: &StateRoot,
    id_generator: &dyn IdGenerator,
    lease: &TransitionLease,
) -> Result<D16State, StateError> {
    let mut state = open_cutover_transition(root, lease)?;
    state.migrate_schema12_to13(lease, id_generator)?;
    state.mode = D16OpenMode::ConfirmedCutover;
    Ok(state)
}

/// Classifies a root for fresh creation without changing it.  Only an absent
/// root, an empty private directory, or one private unlocked transition lease
/// is adoptable.
pub fn classify_fresh_root(path: &Path) -> Result<FreshRootClassification, StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FreshRootClassification::Absent);
        }
        Err(error) => return Err(StateError::io(path, error)),
    };
    if !metadata.is_dir() {
        return Err(StateError::FreshRootRejected(
            FreshRootRejection::NotDirectory,
        ));
    }
    if !is_private_owner_directory(&metadata) {
        return Err(StateError::FreshRootRejected(
            FreshRootRejection::NonPrivateDirectory,
        ));
    }
    let entries = fs::read_dir(path).map_err(|error| StateError::io(path, error))?;
    let mut count = 0_u8;
    for entry in entries {
        let entry = entry.map_err(|error| StateError::io(path, error))?;
        count = count.saturating_add(1);
        let name = entry.file_name();
        if name == TRANSITION_LOCK_FILE {
            let lock_path = entry.path();
            let metadata = fs::symlink_metadata(&lock_path)
                .map_err(|error| StateError::io(&lock_path, error))?;
            if !metadata.is_file() {
                return Err(StateError::FreshRootRejected(
                    FreshRootRejection::NonRegularTransitionLease,
                ));
            }
            if !has_private_file_mode(&metadata) {
                return Err(StateError::FreshRootRejected(
                    FreshRootRejection::NonPrivateTransitionLease,
                ));
            }
            if !is_current_owner(&metadata) {
                return Err(StateError::FreshRootRejected(
                    FreshRootRejection::ForeignTransitionLease,
                ));
            }
            probe_transition_lock(&lock_path)?;
        } else {
            return Err(StateError::FreshRootRejected(
                FreshRootRejection::UnknownArtifact,
            ));
        }
    }
    match count {
        0 => Ok(FreshRootClassification::Empty),
        1 => Ok(FreshRootClassification::TransitionLeaseOnly),
        _ => Err(StateError::FreshRootRejected(
            FreshRootRejection::UnknownArtifact,
        )),
    }
}

fn classify_fresh_root_while_lease_held(
    path: &Path,
) -> Result<FreshRootClassification, StateError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| StateError::io(path, error))?;
    if !metadata.is_dir() || !is_private_owner_directory(&metadata) {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::NonPrivateFreshRoot,
        ));
    }
    let entries = fs::read_dir(path).map_err(|error| StateError::io(path, error))?;
    let mut count = 0_u8;
    for entry in entries {
        let entry = entry.map_err(|error| StateError::io(path, error))?;
        count = count.saturating_add(1);
        if entry.file_name() != TRANSITION_LOCK_FILE {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnknownFreshRootArtifact,
            ));
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| StateError::io(&entry.path(), error))?;
        if !metadata.is_file() || !is_private_owner_file(&metadata) {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::NonPrivateTransitionLease,
            ));
        }
    }
    if count == 1 {
        Ok(FreshRootClassification::TransitionLeaseOnly)
    } else {
        Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ))
    }
}

/// Creates a fresh schema-13 root while holding a private transition lease
/// across the complete allowlist recheck and database creation.
pub fn fresh_create(path: &Path, id_generator: &dyn IdGenerator) -> Result<D16State, StateError> {
    let initial = classify_fresh_root(path)?;
    if matches!(initial, FreshRootClassification::Absent) {
        create_private_fresh_root(path)?;
    }
    let classified = classify_fresh_root(path)?;
    let lease = match classified {
        FreshRootClassification::Empty => TransitionLease::create_for_fresh_root(path)?,
        FreshRootClassification::TransitionLeaseOnly => TransitionLease::acquire(path)?,
        FreshRootClassification::Absent => {
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::NonPrivateFreshRoot,
            ));
        }
    };
    let lease_path = path.join(TRANSITION_LOCK_FILE);
    // The allowlist is repeated while the lease is held, closing the TOCTOU
    // window between classification and database creation.
    let rechecked = classify_fresh_root_while_lease_held(path)?;
    if !matches!(rechecked, FreshRootClassification::TransitionLeaseOnly) {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    lease.require_root(path)?;
    let final_recheck = classify_fresh_root_while_lease_held(path)?;
    if !matches!(final_recheck, FreshRootClassification::TransitionLeaseOnly) {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    lease.require_root(path)?;
    let database_path = path.join("host.sqlite");
    // Publish the database inode with create-new/no-follow semantics before
    // SQLite opens it.  This keeps the private mode tied to the opened file
    // descriptor and prevents a swapped path from being chmodded or adopted.
    let database_file = create_private_database_file(&database_path)?;
    let database_identity = file_identity(
        &database_file
            .metadata()
            .map_err(|error| StateError::io(&database_path, error))?,
    );
    let mut connection = Connection::open_with_flags(
        &database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    let opened_database = fs::symlink_metadata(&database_path)
        .map_err(|error| StateError::io(&database_path, error))?;
    if !opened_database.is_file()
        || !is_private_owner_file(&opened_database)
        || file_identity(&opened_database) != database_identity
    {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    configure_d16_connection(&connection)?;
    create_schema13(&mut connection, id_generator)?;
    drop(database_file);
    drop(connection);
    drop(lease);
    fs::remove_file(&lease_path).map_err(|error| StateError::io(&lease_path, error))?;
    sync_directory(path)?;
    let connection = Connection::open_with_flags(
        &database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    configure_d16_connection(&connection)?;
    validate_schema13(&connection)?;
    Ok(D16State {
        connection,
        root: path.to_path_buf(),
        mode: D16OpenMode::FreshCreate,
    })
}

/// Creates the absent fresh root without following a path component that may
/// have been swapped in after classification.  The directory is opened with
/// `O_NOFOLLOW`, permissions are applied through that opened handle, and the
/// final identity is checked before the caller proceeds to lease acquisition.
fn create_private_fresh_root(path: &Path) -> Result<(), StateError> {
    fs::create_dir(path).map_err(|error| StateError::io(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut options = OpenOptions::new();
        options.read(true);
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
        let directory = options
            .open(path)
            .map_err(|error| StateError::io(path, error))?;
        let before = directory
            .metadata()
            .map_err(|error| StateError::io(path, error))?;
        if !before.is_dir() || !is_current_owner(&before) {
            return Err(StateError::FreshRootRejected(
                FreshRootRejection::NonPrivateDirectory,
            ));
        }
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| StateError::io(path, error))?;
        let after = directory
            .metadata()
            .map_err(|error| StateError::io(path, error))?;
        if !after.is_dir()
            || !is_private_owner_directory(&after)
            || file_identity(&before) != file_identity(&after)
        {
            return Err(StateError::FreshRootRejected(
                FreshRootRejection::NonPrivateDirectory,
            ));
        }
    }
    #[cfg(not(unix))]
    {
        super::utils::set_private_directory_permissions(path)?;
    }
    Ok(())
}

fn ensure_project_mutation_mode(mode: D16OpenMode) -> Result<(), StateError> {
    if matches!(
        mode,
        D16OpenMode::CurrentOnly | D16OpenMode::ConfirmedCutover | D16OpenMode::FreshCreate
    ) {
        Ok(())
    } else {
        Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnsupportedLegacySchema,
        ))
    }
}

fn ensure_cutover_transition_mode(mode: D16OpenMode) -> Result<(), StateError> {
    if matches!(
        mode,
        D16OpenMode::CutoverTransition | D16OpenMode::ConfirmedCutover
    ) {
        Ok(())
    } else {
        Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnsupportedLegacySchema,
        ))
    }
}

fn ensure_d17_current_mode(mode: D16OpenMode) -> Result<(), StateError> {
    if mode == D16OpenMode::D17Current {
        Ok(())
    } else {
        Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnsupportedLegacySchema,
        ))
    }
}

/// The only two authority sources permitted to mutate D17 onboarding state.
/// The cutover variant is retained solely for the one-time schema migration
/// checkpoint; normal D17 execution can use only the schema-14 opening and
/// its separately retained provisional lease.
#[derive(Clone, Copy)]
enum D17OnboardingAuthority<'lease> {
    Cutover(&'lease TransitionLease),
    Current,
}

impl D17OnboardingAuthority<'_> {
    fn revalidate(self, mode: D16OpenMode, root: &Path) -> Result<(), StateError> {
        match self {
            Self::Cutover(transition_lease) => {
                ensure_cutover_transition_mode(mode)?;
                transition_lease.revalidate_for_mutation(root)
            }
            Self::Current => ensure_d17_current_mode(mode),
        }
    }
}

#[derive(Clone, Copy)]
#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
struct ExistingOnboardingLocation {
    location_id: LocationId,
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn validate_onboarding_prepare_request(
    request: &OnboardingPrepareRequest,
    state_root: &Path,
) -> Result<(), StateError> {
    validate_registry_text("onboarding request key", &request.request_key)?;
    if request.presentation_id.is_nil()
        || request.slot_generation.is_nil()
        || request.candidate_runtime_id.as_uuid().is_nil()
    {
        return Err(StateError::InvalidOnboardingPreparation);
    }
    let state_root =
        fs::canonicalize(state_root).map_err(|_| StateError::InvalidOnboardingPreparation)?;
    if request.runtime_paths != RuntimePaths::for_runtime(&state_root, request.candidate_runtime_id)
        || !is_normalized_absolute_utf8_path(&request.repository.project_root)
        || !is_normalized_absolute_utf8_path(&request.shell_cwd)
        || !request
            .shell_cwd
            .starts_with(&request.repository.project_root)
    {
        return Err(StateError::InvalidOnboardingPreparation);
    }
    let repository_path = request
        .repository
        .project_root
        .to_str()
        .ok_or(StateError::InvalidOnboardingPreparation)?;
    validate_registry_text("repository path", repository_path)?;
    validate_project_display_name(&request.repository.display_name)?;
    validate_repository_fingerprint(request.repository.remote_identity_fingerprint.as_deref())?;
    validate_safe_origin_display(request.repository.remote_identity_display.as_deref())?;
    Ok(())
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn is_normalized_absolute_utf8_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn load_registry_generation(transaction: &rusqlite::Transaction<'_>) -> Result<String, StateError> {
    let generation: String = transaction
        .query_row(
            "SELECT registry_generation FROM host_identity WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    validate_registry_text("registry generation", &generation)?;
    Ok(generation)
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn load_location_for_repository_path(
    transaction: &rusqlite::Transaction<'_>,
    repository_path: &Path,
) -> Result<Option<ExistingOnboardingLocation>, StateError> {
    let repository_path = repository_path
        .to_str()
        .ok_or(StateError::InvalidOnboardingPreparation)?;
    let mut statement = transaction
        .prepare(
            "SELECT location_id FROM project_locations
             WHERE repository_path = ?1 ORDER BY location_id LIMIT 2",
        )
        .map_err(StateError::Sqlite)?;
    let locations = statement
        .query_map([repository_path], |row| row.get::<_, String>(0))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    match locations.as_slice() {
        [] => Ok(None),
        [location_id] => location_id
            .parse::<LocationId>()
            .map(|location_id| Some(ExistingOnboardingLocation { location_id }))
            .map_err(|_| StateError::MalformedHostSchema),
        _ => Err(StateError::MalformedHostSchema),
    }
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn onboarding_claims(
    operation_id: OperationId,
    location_id: LocationId,
    runtime_generation: &str,
    registry_generation: &str,
    lease_generation: i64,
    request: &OnboardingPrepareRequest,
) -> Result<LaunchCapabilityClaims, StateError> {
    LaunchCapabilityClaims::new(
        operation_id,
        request.presentation_id,
        request.presentation_revision,
        request.slot_generation,
        lease_generation,
        request.candidate_runtime_id,
        request.runtime_paths.clone(),
        request.provider,
        request.shell_cwd.clone(),
        request.repository.project_root.clone(),
        location_id,
        runtime_generation.to_owned(),
        registry_generation.to_owned(),
        request.shell_pid,
        request.shell_birth.clone(),
        request.shell_process_group,
        request.shell_session,
        request.argv_digest.clone(),
        request.boot_provenance.clone(),
    )
    .map_err(|_error: CapabilityError| StateError::InvalidOnboardingPreparation)
}

#[allow(
    dead_code,
    reason = "the D17 helper remains unreachable until the atomic Navigator cutover"
)]
fn map_onboarding_capability_error(error: CapabilityError) -> StateError {
    match error {
        CapabilityError::Expired => StateError::OnboardingCapabilityExpired,
        CapabilityError::InvalidClaims
        | CapabilityError::InvalidExpiry
        | CapabilityError::InvalidToken
        | CapabilityError::ClaimMismatch => StateError::OnboardingCapabilityRejected,
    }
}

#[allow(
    dead_code,
    clippy::too_many_lines,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn load_existing_onboarding_preparation(
    transaction: &rusqlite::Transaction<'_>,
    request: &OnboardingPrepareRequest,
    lease_generation: i64,
    registry_generation: &str,
    state_root: &Path,
) -> Result<Option<ExistingOnboardingReservation>, StateError> {
    let existing: Option<(String, String, String, String, Option<String>)> = transaction
        .query_row(
            "SELECT operation_id, kind, phase, expected_revisions_json, launch_claims_digest
             FROM compound_operations WHERE request_key = ?1",
            [&request.request_key],
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
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((operation_id, kind, phase, encoded_intent, claims_digest)) = existing else {
        return Ok(None);
    };
    if kind != "onboard" || phase != "capability_issued" {
        return Err(StateError::OnboardingOperationUnavailable);
    }
    let operation_id = operation_id
        .parse::<OperationId>()
        .map_err(|_| StateError::MalformedHostSchema)?;
    let intent: PersistedOnboardingIntent =
        serde_json::from_str(&encoded_intent).map_err(|_| StateError::MalformedHostSchema)?;
    if intent.version != 1
        || intent.presentation_id != request.presentation_id
        || intent.presentation_revision != request.presentation_revision
        || intent.slot_generation != request.slot_generation
        || intent.lease_generation != lease_generation
        || intent.candidate_runtime_id != request.candidate_runtime_id
        || intent.provider != request.provider
        || intent.registry_generation != registry_generation
        || intent.argv_digest != request.argv_digest
        || intent.boot_provenance != request.boot_provenance
    {
        return Err(StateError::OperationRequestMismatch);
    }
    let runtime: Option<(String, String, String, String, String, String)> = transaction
        .query_row(
            "SELECT runtimes.workstream_id, workstreams.location_id, runtimes.provider,
                    runtimes.tmux_generation, runtimes.tmux_session, runtimes.cwd
             FROM runtimes
             JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
             WHERE runtimes.runtime_id = ?1",
            [intent.candidate_runtime_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((workstream_id, location_id, provider, runtime_generation, session, cwd)) = runtime
    else {
        return Err(StateError::MalformedHostSchema);
    };
    let repository_path = request
        .repository
        .project_root
        .to_str()
        .ok_or(StateError::InvalidOnboardingPreparation)?;
    if workstream_id != intent.workstream_id.to_string()
        || location_id != intent.location_id.to_string()
        || provider != intent.provider.as_str()
        || runtime_generation != intent.runtime_generation
        || session != request.runtime_paths.session_name
        || cwd != repository_path
        || request.runtime_paths
            != RuntimePaths::for_runtime(
                &fs::canonicalize(state_root)
                    .map_err(|_| StateError::InvalidOnboardingPreparation)?,
                intent.candidate_runtime_id,
            )
    {
        return Err(StateError::MalformedHostSchema);
    }
    let claims = onboarding_claims(
        operation_id,
        intent.location_id,
        &intent.runtime_generation,
        registry_generation,
        lease_generation,
        request,
    )?;
    if claims_digest.as_deref() != Some(claims.digest().as_str()) {
        return Err(StateError::OperationRequestMismatch);
    }
    Ok(Some(ExistingOnboardingReservation {
        operation_id,
        location_id: intent.location_id,
        workstream_id: intent.workstream_id,
        runtime_id: intent.candidate_runtime_id,
    }))
}

#[allow(
    dead_code,
    reason = "the D17 broker remains unreachable until the atomic Navigator cutover"
)]
fn insert_onboarding_location(
    transaction: &rusqlite::Transaction<'_>,
    request: &OnboardingPrepareRequest,
    location_id: LocationId,
    id_generator: &dyn IdGenerator,
) -> Result<(), StateError> {
    let repository_path = request
        .repository
        .project_root
        .to_str()
        .ok_or(StateError::InvalidOnboardingPreparation)?;
    let fingerprint = request.repository.remote_identity_fingerprint.as_deref();
    let remote_display = request
        .repository
        .remote_identity_display
        .as_deref()
        .unwrap_or_default();
    transaction
        .execute(
            "INSERT INTO project_locations (
                location_id, repository_path, repository_display_name,
                remote_identity_fingerprint, remote_identity_display,
                revision, project_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, NULL)",
            params![
                location_id.to_string(),
                repository_path,
                request.repository.display_name,
                fingerprint,
                remote_display,
            ],
        )
        .map_err(StateError::Sqlite)?;
    let project = if let Some(fingerprint) = fingerprint {
        if let Some(existing) = find_project_by_fingerprint(transaction, fingerprint)? {
            bump_project_revision(transaction, existing.project_id)?;
            transaction
                .execute(
                    "UPDATE project_locations SET project_id = ?1 WHERE location_id = ?2",
                    params![existing.project_id.to_string(), location_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            existing
        } else {
            let created = create_project(
                transaction,
                location_id,
                &request.repository.display_name,
                Some(fingerprint),
                id_generator,
            )?;
            transaction
                .execute(
                    "UPDATE project_locations SET project_id = ?1 WHERE location_id = ?2",
                    params![created.project_id.to_string(), location_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            created
        }
    } else {
        let created = create_project(
            transaction,
            location_id,
            &request.repository.display_name,
            None,
            id_generator,
        )?;
        transaction
            .execute(
                "UPDATE project_locations SET project_id = ?1 WHERE location_id = ?2",
                params![created.project_id.to_string(), location_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        created
    };
    let _ = project;
    Ok(())
}

fn next_revision(revision: Revision) -> Result<Revision, StateError> {
    Revision::try_from(
        revision
            .value()
            .checked_add(1)
            .ok_or(StateError::ConcurrentWrite)?,
    )
    .map_err(|_| StateError::ConcurrentWrite)
}

fn load_current_observer_proof(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<CurrentObserverHandleProof, StateError> {
    let (lifecycle, runtime_provider, workstream_provider): (String, String, String) = transaction
        .query_row(
            "SELECT runtimes.lifecycle, runtimes.provider, workstreams.provider
             FROM runtimes
             JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
             WHERE runtimes.runtime_id = ?1",
            [runtime_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(StateError::Sqlite)?;
    if runtime_provider != "opencode" || workstream_provider != "opencode" {
        return Err(StateError::ProviderIdentityMismatch);
    }
    if runtime_status_from_text(&lifecycle)? == RuntimeStatus::Stopped {
        return Err(StateError::HookEvidenceMismatch);
    }
    let handle =
        load_opencode_handle(transaction, runtime_id)?.ok_or(StateError::HookEvidenceMismatch)?;
    let pid = handle
        .observer_pid
        .ok_or(StateError::HookEvidenceMismatch)?;
    let birth = handle
        .observer_birth
        .ok_or(StateError::HookEvidenceMismatch)?;
    if handle.observer_status == OpenCodeObserverStatus::Stopped {
        return Err(StateError::HookEvidenceMismatch);
    }
    Ok(CurrentObserverHandleProof {
        runtime_id,
        runtime_generation: handle.runtime_generation,
        pid,
        birth,
        revision: handle.revision,
    })
}

fn configure_d16_connection(connection: &Connection) -> Result<(), StateError> {
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(StateError::Sqlite)
}

fn ensure_project_projection_mode(mode: D16OpenMode) -> Result<(), StateError> {
    if matches!(
        mode,
        D16OpenMode::ObserverTransition | D16OpenMode::CutoverTransition
    ) {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnsupportedLegacySchema,
        ));
    }
    Ok(())
}

fn validate_d16_host_database_path(path: &Path) -> Result<bool, StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(StateError::io(path, error)),
    };
    if !metadata.is_file() || !is_private_owner_file(&metadata) {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(true)
}

/// Validates the exact root directory before any direct state open derives or
/// opens `host.sqlite`.  In particular, `symlink_metadata` deliberately does
/// not follow a root symlink: otherwise `SQLite` would follow the root's parent
/// component and a direct current/observer open could inspect another root's
/// database.  An absent root remains valid here so the caller can preserve its
/// mode-specific missing/fresh-state error.
fn validate_state_root_directory(path: &Path) -> Result<(), StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(StateError::io(path, error)),
    };
    if !metadata.is_dir() {
        return Err(StateError::FreshRootRejected(
            FreshRootRejection::NotDirectory,
        ));
    }
    if !is_private_owner_directory(&metadata) {
        return Err(StateError::FreshRootRejected(
            FreshRootRejection::NonPrivateDirectory,
        ));
    }
    let canonical = fs::canonicalize(path).map_err(|error| StateError::io(path, error))?;
    if canonical != path {
        return Err(StateError::FreshRootRejected(
            FreshRootRejection::NonCanonicalDirectory,
        ));
    }
    Ok(())
}

fn exact_artifact_metadata(path: &Path) -> Result<Option<fs::Metadata>, StateError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StateError::io(path, error)),
    }
}

fn is_exact_legacy_client_artifact(name: &str) -> bool {
    matches!(
        name,
        LEGACY_CLIENT_DATABASE_FILE
            | LEGACY_CLIENT_DATABASE_WAL_FILE
            | LEGACY_CLIENT_DATABASE_SHM_FILE
    )
}

fn reject_transition_lease_artifact(root: &Path) -> Result<(), StateError> {
    let path = root.join(TRANSITION_LOCK_FILE);
    if exact_artifact_metadata(&path)?.is_some() {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::TransitionLeasePresent,
        ));
    }
    Ok(())
}

/// Schema-13 knows nothing about D17's stable provisional lease. Any such
/// artifact is therefore ambiguous pre-migration evidence, not a candidate to
/// adopt or clean up. The explicit D17 migration refuses before beginning its
/// transaction and leaves the path untouched.
fn reject_pre_schema14_provisional_lock(root: &Path) -> Result<(), StateError> {
    let path = root.join("provisional.lock");
    if exact_artifact_metadata(&path)?.is_some() {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::ProvisionalLockPresent,
        ));
    }
    Ok(())
}

fn reject_current_only_artifacts(
    root: &Path,
    client_means_cutover: bool,
) -> Result<(), StateError> {
    for name in [
        TRANSITION_LOCK_FILE,
        LEGACY_CLIENT_DATABASE_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE,
        LEGACY_CLIENT_DATABASE_SHM_FILE,
        OBSERVER_HANDOVER_JOURNAL_FILE,
        OBSERVER_HANDOVER_JOURNAL_TEMP_FILE,
        OBSERVER_HANDOVER_ACTIVATION_ACK_FILE,
        OBSERVER_HANDOVER_ACTIVATION_ACK_TEMP_FILE,
    ] {
        let path = root.join(name);
        if exact_artifact_metadata(&path)?.is_some() {
            let reason = if is_exact_legacy_client_artifact(name) {
                if client_means_cutover {
                    return Err(StateError::CutoverRequired);
                }
                StateRecoveryReason::LegacyClientArtifact
            } else if name == TRANSITION_LOCK_FILE {
                StateRecoveryReason::TransitionLeasePresent
            } else {
                StateRecoveryReason::ObserverJournalPresent
            };
            return Err(StateError::StateRecoveryRequired(reason));
        }
    }
    Ok(())
}

/// D17 recognizes the stable provisional lock as schema-14 operational state,
/// but otherwise retains D16's strict refusal of unfinished cutover, legacy
/// client, and observer-handover artifacts. The lock itself is revalidated by
/// the retained `ProvisionalLease` before a D17 mutation; merely opening state
/// never adopts, creates, or repairs it.
fn reject_d17_current_only_artifacts(root: &Path) -> Result<(), StateError> {
    for name in [
        TRANSITION_LOCK_FILE,
        LEGACY_CLIENT_DATABASE_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE,
        LEGACY_CLIENT_DATABASE_SHM_FILE,
        OBSERVER_HANDOVER_JOURNAL_FILE,
        OBSERVER_HANDOVER_JOURNAL_TEMP_FILE,
        OBSERVER_HANDOVER_ACTIVATION_ACK_FILE,
        OBSERVER_HANDOVER_ACTIVATION_ACK_TEMP_FILE,
    ] {
        if exact_artifact_metadata(&root.join(name))?.is_some() {
            let reason = if is_exact_legacy_client_artifact(name) {
                return Err(StateError::CutoverRequired);
            } else if name == TRANSITION_LOCK_FILE {
                StateRecoveryReason::TransitionLeasePresent
            } else {
                StateRecoveryReason::ObserverJournalPresent
            };
            return Err(StateError::StateRecoveryRequired(reason));
        }
    }
    Ok(())
}

fn reject_schema13_conversion_artifacts(
    root: &Path,
    allow_transition_lease: bool,
) -> Result<(), StateError> {
    for name in [
        LEGACY_CLIENT_DATABASE_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE,
        LEGACY_CLIENT_DATABASE_SHM_FILE,
        OBSERVER_HANDOVER_JOURNAL_FILE,
        OBSERVER_HANDOVER_JOURNAL_TEMP_FILE,
        OBSERVER_HANDOVER_ACTIVATION_ACK_FILE,
        OBSERVER_HANDOVER_ACTIVATION_ACK_TEMP_FILE,
    ] {
        let path = root.join(name);
        if exact_artifact_metadata(&path)?.is_some() {
            if is_exact_legacy_client_artifact(name) {
                return Err(StateError::CutoverRequired);
            }
            return Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::ObserverJournalPresent,
            ));
        }
    }
    if !allow_transition_lease {
        reject_transition_lease_artifact(root)?;
    }
    Ok(())
}

fn schema_version(connection: &Connection) -> Result<i64, StateError> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| match error {
            rusqlite::Error::SqliteFailure(_, _) => StateError::MalformedHostSchema,
            error => StateError::Sqlite(error),
        })
}

fn validate_schema12(connection: &Connection) -> Result<(), StateError> {
    if schema_version(connection)? != D16_SCHEMA_12_VERSION {
        return Err(StateError::MalformedHostSchema);
    }
    validate_host_identity(connection, D16_SCHEMA_12_VERSION)?;
    for (table, columns) in required_schema12_tables() {
        validate_table_columns(connection, table, columns)?;
    }
    if table_exists(connection, "projects")?
        || table_has_column_readonly(connection, "project_locations", "project_id")?
        || table_exists(connection, "opencode_settled_messages")?
    {
        return Err(StateError::MalformedHostSchema);
    }
    // Reject malformed relationships before a 12-to-13 rewrite starts.  If
    // this check ran only after commit, schema-13 validation could discover a
    // dangling row after the exact schema-12 state had already been replaced.
    validate_foreign_keys(connection)?;
    Ok(())
}

fn validate_schema13(connection: &Connection) -> Result<(), StateError> {
    if schema_version(connection)? != D16_HOST_SCHEMA_VERSION {
        return Err(StateError::MalformedHostSchema);
    }
    validate_host_identity(connection, D16_HOST_SCHEMA_VERSION)?;
    for (table, columns) in required_schema12_tables() {
        validate_table_columns(connection, table, columns)?;
    }
    validate_schema13_extensions(connection)
}

fn validate_schema14(connection: &Connection) -> Result<(), StateError> {
    if schema_version(connection)? != D17_HOST_SCHEMA_VERSION {
        return Err(StateError::MalformedHostSchema);
    }
    validate_host_identity(connection, D17_HOST_SCHEMA_VERSION)?;
    for (table, columns) in required_schema12_tables() {
        if table != "project_browser_settings" {
            validate_table_columns(connection, table, columns)?;
        }
    }
    if table_exists(connection, "project_browser_settings")? {
        return Err(StateError::MalformedHostSchema);
    }
    validate_schema13_extensions(connection)?;
    validate_table_columns(
        connection,
        "host_operational_metadata",
        &[
            "singleton",
            "provisional_lease_generation",
            "provisional_lock_phase",
            "provisional_lock_device",
            "provisional_lock_inode",
        ],
    )?;
    let metadata: Option<(i64, String, Option<i64>, Option<i64>)> = connection
        .query_row(
            "SELECT provisional_lease_generation, provisional_lock_phase,
                    provisional_lock_device, provisional_lock_inode
             FROM host_operational_metadata WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((generation, phase, device, inode)) = metadata else {
        return Err(StateError::MalformedHostSchema);
    };
    if generation <= 0
        || !matches!(phase.as_str(), "pending" | "ready")
        || matches!(phase.as_str(), "pending") && (device.is_some() || inode.is_some())
        || matches!(phase.as_str(), "ready") && (device.is_none() || inode.is_none())
        || device.is_some_and(|value| value < 0)
        || inode.is_some_and(|value| value <= 0)
    {
        return Err(StateError::MalformedHostSchema);
    }
    validate_schema14_onboarding_operation_columns(connection)
}

fn validate_schema14_onboarding_operation_columns(
    connection: &Connection,
) -> Result<(), StateError> {
    validate_table_columns(
        connection,
        "compound_operations",
        &[
            "launch_token_id",
            "launch_token_verifier",
            "launch_token_expiry_monotonic",
            "launch_claims_digest",
        ],
    )?;
    let mut statement = connection
        .prepare(
            "SELECT kind, phase, launch_token_id, launch_token_verifier,
                    launch_token_expiry_monotonic, launch_claims_digest
             FROM compound_operations ORDER BY operation_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let kind: String = row.get(0).map_err(StateError::Sqlite)?;
        let phase: String = row.get(1).map_err(StateError::Sqlite)?;
        let token_id: Option<String> = row.get(2).map_err(StateError::Sqlite)?;
        let token_verifier: Option<String> = row.get(3).map_err(StateError::Sqlite)?;
        let token_expiry: Option<i64> = row.get(4).map_err(StateError::Sqlite)?;
        let claims_digest: Option<String> = row.get(5).map_err(StateError::Sqlite)?;
        if kind == "onboard" {
            validate_schema14_onboarding_operation(
                &phase,
                token_id.as_deref(),
                token_verifier.as_deref(),
                token_expiry,
                claims_digest.as_deref(),
            )?;
        } else if token_id.is_some()
            || token_verifier.is_some()
            || token_expiry.is_some()
            || claims_digest.is_some()
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

fn validate_schema14_onboarding_operation(
    phase: &str,
    token_id: Option<&str>,
    token_verifier: Option<&str>,
    token_expiry: Option<i64>,
    claims_digest: Option<&str>,
) -> Result<(), StateError> {
    let phase = operation_phase_from_text(phase).map_err(|_| StateError::MalformedHostSchema)?;
    let phase =
        OnboardingPhase::from_operation_phase(phase).ok_or(StateError::MalformedHostSchema)?;
    let capability_is_absent = token_id.is_none()
        && token_verifier.is_none()
        && token_expiry.is_none()
        && claims_digest.is_none();
    let capability_is_complete = matches!(
        (token_id, token_verifier, token_expiry, claims_digest),
        (Some(_), Some(_), Some(_), Some(_))
    );
    match phase {
        OnboardingPhase::Prepared if !capability_is_absent => {
            return Err(StateError::MalformedHostSchema);
        }
        OnboardingPhase::RolledBack if !(capability_is_absent || capability_is_complete) => {
            return Err(StateError::MalformedHostSchema);
        }
        OnboardingPhase::Prepared | OnboardingPhase::RolledBack => {}
        _ if !capability_is_complete => return Err(StateError::MalformedHostSchema),
        _ => {}
    }
    if capability_is_complete {
        let token_id = token_id.ok_or(StateError::MalformedHostSchema)?;
        let token_verifier = token_verifier.ok_or(StateError::MalformedHostSchema)?;
        let token_expiry = token_expiry.ok_or(StateError::MalformedHostSchema)?;
        let claims_digest = claims_digest.ok_or(StateError::MalformedHostSchema)?;
        if Uuid::parse_str(token_id).is_err()
            || token_expiry <= 0
            || !is_versioned_sha256(token_verifier, "d17-launch-verifier-v1:sha256:")
            || !is_versioned_sha256(claims_digest, "d17-launch-claims-v1:sha256:")
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

fn is_versioned_sha256(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
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
    let contents = format!("{PROVISIONAL_LOCK_FORMAT} {host_id} {generation}\n").into_bytes();
    if contents.len() > MAX_PROVISIONAL_LOCK_BYTES {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(contents)
}

fn validate_schema13_extensions(connection: &Connection) -> Result<(), StateError> {
    validate_table_columns(
        connection,
        "projects",
        &[
            "project_id",
            "label_location_id",
            "display_name",
            "repository_fingerprint",
            "revision",
        ],
    )?;
    validate_table_columns(
        connection,
        "project_locations",
        &[
            "location_id",
            "project_id",
            "repository_path",
            "repository_display_name",
            "remote_identity_fingerprint",
            "remote_identity_display",
            "revision",
        ],
    )?;
    validate_table_columns(
        connection,
        "opencode_settled_messages",
        &[
            "settled_message_id",
            "runtime_id",
            "runtime_generation",
            "native_session_id",
            "message_id",
        ],
    )?;
    let has_fingerprint_index: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'index' AND name = 'project_repository_fingerprint_idx'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !has_fingerprint_index {
        return Err(StateError::MalformedHostSchema);
    }
    let null_locations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE project_id IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if null_locations != 0 {
        return Err(StateError::MalformedHostSchema);
    }
    validate_foreign_keys(connection)?;
    validate_project_location_rows(connection)?;
    validate_project_membership(connection)?;
    Ok(())
}

fn validate_foreign_keys(connection: &Connection) -> Result<(), StateError> {
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(StateError::Sqlite)?;
    if foreign_keys
        .query([])
        .map_err(StateError::Sqlite)?
        .next()
        .map_err(StateError::Sqlite)?
        .is_some()
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

fn validate_project_location_rows(connection: &Connection) -> Result<(), StateError> {
    let mut statement = connection
        .prepare(
            "SELECT location_id, project_id, repository_display_name,
                    remote_identity_display, revision
             FROM project_locations ORDER BY location_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let location_id: String = row.get(0).map_err(StateError::Sqlite)?;
        let project_id: Option<String> = row.get(1).map_err(StateError::Sqlite)?;
        let display_name: String = row.get(2).map_err(StateError::Sqlite)?;
        let origin_display: Option<String> = row.get(3).map_err(StateError::Sqlite)?;
        let revision: i64 = row.get(4).map_err(StateError::Sqlite)?;
        location_id
            .parse::<LocationId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        project_id
            .ok_or(StateError::MalformedHostSchema)?
            .parse::<ProjectId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_project_display_name(&display_name)
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_safe_origin_display(origin_display.as_deref())
            .map_err(|_| StateError::MalformedHostSchema)?;
        Revision::try_from(revision).map_err(|_| StateError::MalformedHostSchema)?;
    }
    Ok(())
}

fn validate_project_membership(connection: &Connection) -> Result<(), StateError> {
    let mut statement = connection
        .prepare(
            "SELECT project_id, label_location_id, display_name,
                    repository_fingerprint, revision
             FROM projects ORDER BY project_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let project_id: String = row.get(0).map_err(StateError::Sqlite)?;
        let label_location_id: String = row.get(1).map_err(StateError::Sqlite)?;
        let display_name: String = row.get(2).map_err(StateError::Sqlite)?;
        let fingerprint: Option<String> = row.get(3).map_err(StateError::Sqlite)?;
        let revision: i64 = row.get(4).map_err(StateError::Sqlite)?;
        let project_id = project_id
            .parse::<ProjectId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        let label_location_id = label_location_id
            .parse::<LocationId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_project_display_name(&display_name)
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_repository_fingerprint(fingerprint.as_deref())
            .map_err(|_| StateError::MalformedHostSchema)?;
        Revision::try_from(revision).map_err(|_| StateError::MalformedHostSchema)?;
        let membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
                [project_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if membership == 0 {
            return Err(StateError::MalformedHostSchema);
        }
        let source_membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM project_locations
                 WHERE project_id = ?1 AND location_id = ?2",
                params![project_id.to_string(), label_location_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if source_membership != 1 {
            return Err(StateError::MalformedHostSchema);
        }
        let (source_display_name, source_fingerprint): (String, Option<String>) = connection
            .query_row(
                "SELECT repository_display_name, remote_identity_fingerprint
                 FROM project_locations WHERE project_id = ?1 AND location_id = ?2",
                params![project_id.to_string(), label_location_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(StateError::Sqlite)?;
        if source_display_name != display_name
            || !project_source_fingerprints_compatible(
                fingerprint.as_deref(),
                source_fingerprint.as_deref(),
            )
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

fn validate_host_identity(connection: &Connection, version: i64) -> Result<(), StateError> {
    let row: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT host_id, registry_generation, schema_version
             FROM host_identity WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((host_id, generation, recorded_version)) = row else {
        return Err(StateError::MalformedHostSchema);
    };
    if Uuid::parse_str(&host_id).is_err()
        || generation.is_empty()
        || generation.len() > 256
        || generation.contains(['\n', '\r'])
        || recorded_version != version
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn required_schema12_tables() -> [(&'static str, &'static [&'static str]); 12] {
    [
        (
            "host_identity",
            &[
                "singleton",
                "host_id",
                "registry_generation",
                "schema_version",
            ],
        ),
        (
            "codex_integrations",
            &[
                "integration_id",
                "profile_name",
                "canonical_profile_path",
                "owner_id",
                "profile_schema_version",
                "hook_executable_path",
                "generated_content_hash",
                "lifecycle",
                "revision",
            ],
        ),
        (
            "project_locations",
            &[
                "location_id",
                "repository_path",
                "repository_display_name",
                "remote_identity_fingerprint",
                "remote_identity_display",
                "revision",
            ],
        ),
        (
            "workstreams",
            &[
                "workstream_id",
                "location_id",
                "provider",
                "origin",
                "source_workstream_id",
                "lifecycle",
                "archived_at_millis",
                "last_activity_sequence",
                "last_activity_at_millis",
                "revision",
            ],
        ),
        (
            "independent_creation_requests",
            &[
                "request_key",
                "source_workstream_id",
                "source_revision",
                "workstream_id",
            ],
        ),
        (
            "project_browser_settings",
            &["singleton", "root_path", "revision"],
        ),
        (
            "runtimes",
            &[
                "runtime_id",
                "workstream_id",
                "provider",
                "tmux_generation",
                "tmux_session",
                "cwd",
                "provider_pid",
                "process_birth",
                "lifecycle",
                "revision",
            ],
        ),
        (
            "opencode_runtime_handles",
            &[
                "runtime_id",
                "runtime_generation",
                "endpoint_host",
                "endpoint_port",
                "version",
                "native_session_id",
                "observer_pid",
                "observer_birth",
                "observer_status",
                "revision",
            ],
        ),
        (
            "provider_bindings",
            &[
                "binding_id",
                "runtime_id",
                "provider",
                "native_session_id",
                "start_source",
                "last_settled_turn_id",
                "observed_thread_name",
                "name_state",
                "name_observed_at",
                "predecessor_native_session_id",
                "predecessor_effective_name",
                "runtime_generation",
                "revision",
            ],
        ),
        (
            "attention_states",
            &[
                "workstream_id",
                "result_unseen_since_revision",
                "recovery_unseen_since_revision",
                "latest_native_session_id",
                "latest_native_session_provider",
                "latest_turn_id",
                "revision",
            ],
        ),
        (
            "compound_operations",
            &[
                "operation_id",
                "request_key",
                "kind",
                "phase",
                "expected_revisions_json",
                "effect_watermark",
                "outcome_json",
                "revision",
            ],
        ),
        // The base schema contains this index; requiring it catches a
        // partially-created schema without depending on index SQL formatting.
        ("sqlite_master", &[]),
    ]
}

fn validate_table_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
) -> Result<(), StateError> {
    if table == "sqlite_master" {
        return Ok(());
    }
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    if required
        .iter()
        .any(|column| !columns.iter().any(|value| value == column))
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, StateError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

fn table_has_column_readonly(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, StateError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?
        .iter()
        .any(|value| value == column))
}

fn migrate_schema13_to14(
    connection: &mut Connection,
    lease: &TransitionLease,
) -> Result<(), StateError> {
    let previous_busy_timeout = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .map_err(StateError::Sqlite)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(StateError::Sqlite)?;
    let migration = migrate_schema13_to14_with_zero_timeout(connection, lease);
    let restore = connection.busy_timeout(Duration::from_millis(
        u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
    ));
    match (migration, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(StateError::Sqlite(error)),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn migrate_schema13_to14_with_zero_timeout(
    connection: &mut Connection,
    lease: &TransitionLease,
) -> Result<(), StateError> {
    lease.revalidate(lease.root())?;
    validate_schema13(connection)?;
    reject_pre_schema14_provisional_lock(lease.root())?;
    let deadline = Instant::now() + MIGRATION_BUDGET;
    let transaction = begin_migration_transaction(&*connection, deadline)?;
    check_migration_deadline(deadline)?;
    lease.revalidate(lease.root())?;
    reject_pre_schema14_provisional_lock(lease.root())?;
    transaction
        .execute_batch(HOST_SCHEMA_14_ONBOARDING_SQL)
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO host_operational_metadata (
                singleton, provisional_lease_generation, provisional_lock_phase,
                provisional_lock_device, provisional_lock_inode
             ) VALUES (1, 1, 'pending', NULL, NULL)",
            [],
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute("PRAGMA user_version = 14", [])
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE host_identity SET schema_version = 14 WHERE singleton = 1",
            [],
        )
        .map_err(StateError::Sqlite)?;
    check_migration_deadline(deadline)?;
    validate_schema14(&transaction)?;
    check_migration_deadline(deadline)?;
    transaction.commit().map_err(StateError::Sqlite)
}

fn migrate_schema12_to13(
    connection: &mut Connection,
    id_generator: &dyn IdGenerator,
    lease: &TransitionLease,
) -> Result<(), StateError> {
    let previous_busy_timeout = connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .map_err(StateError::Sqlite)?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(StateError::Sqlite)?;
    let migration = migrate_schema12_to13_with_zero_timeout(connection, id_generator, lease);
    let restore = connection.busy_timeout(Duration::from_millis(
        u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
    ));
    match (migration, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(StateError::Sqlite(error)),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn migrate_schema12_to13_with_zero_timeout(
    connection: &mut Connection,
    id_generator: &dyn IdGenerator,
    lease: &TransitionLease,
) -> Result<(), StateError> {
    lease.revalidate(lease.root())?;
    validate_schema12(connection)?;
    let plan = build_project_plan(connection, id_generator)?;
    let deadline = Instant::now() + MIGRATION_BUDGET;
    let transaction = begin_migration_transaction(&*connection, deadline)?;
    check_migration_deadline(deadline)?;
    let current = read_locations(&transaction)?;
    if current != plan.locations {
        return Err(StateError::ConcurrentWrite);
    }
    check_migration_deadline(deadline)?;
    transaction
        .execute_batch(HOST_SCHEMA_13_PROJECT_SQL)
        .map_err(StateError::Sqlite)?;
    check_migration_deadline(deadline)?;
    for project in &plan.projects {
        transaction
            .execute(
                "INSERT INTO projects (
                    project_id, label_location_id, display_name,
                    repository_fingerprint, revision
                 ) VALUES (?1, ?2, ?3, ?4, 1)",
                params![
                    project.project_id.to_string(),
                    project.label_location_id.to_string(),
                    project.display_name,
                    project.repository_fingerprint,
                ],
            )
            .map_err(StateError::Sqlite)?;
        check_migration_deadline(deadline)?;
        for location_id in &project.locations {
            transaction
                .execute(
                    "UPDATE project_locations SET project_id = ?1 WHERE location_id = ?2",
                    params![project.project_id.to_string(), location_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            check_migration_deadline(deadline)?;
        }
    }
    check_migration_deadline(deadline)?;
    transaction
        .execute("PRAGMA user_version = 13", [])
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "UPDATE host_identity SET schema_version = 13 WHERE singleton = 1",
            [],
        )
        .map_err(StateError::Sqlite)?;
    check_migration_deadline(deadline)?;
    // Revalidate the complete reconstructed membership while the writer
    // transaction is still uncommitted.  Any malformed relationship or
    // Project/source invariant therefore drops the transaction and leaves
    // the exact schema-12 database available for a retry.
    validate_foreign_keys(&transaction)?;
    validate_project_membership_transaction(&transaction)?;
    check_migration_deadline(deadline)?;
    transaction.commit().map_err(StateError::Sqlite)
}

fn begin_migration_transaction(
    connection: &Connection,
    deadline: Instant,
) -> Result<rusqlite::Transaction<'_>, StateError> {
    check_migration_deadline(deadline)?;
    match begin_migration_attempt(connection) {
        Ok(transaction) => Ok(transaction),
        Err(error) if is_retryable_observer_error(&error) => {
            let now = Instant::now();
            if now >= deadline {
                return Err(StateError::ObserverDatabaseDeadlineExceeded);
            }
            thread_sleep(Duration::from_millis(1).min(deadline - now));
            begin_migration_transaction(connection, deadline)
        }
        Err(error) => Err(StateError::Sqlite(error)),
    }
}

fn begin_migration_attempt(
    connection: &Connection,
) -> Result<rusqlite::Transaction<'_>, rusqlite::Error> {
    rusqlite::Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
}

fn check_migration_deadline(deadline: Instant) -> Result<(), StateError> {
    if Instant::now() >= deadline {
        Err(StateError::ObserverDatabaseDeadlineExceeded)
    } else {
        Ok(())
    }
}

fn create_schema13(
    connection: &mut Connection,
    id_generator: &dyn IdGenerator,
) -> Result<(), StateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StateError::Sqlite)?;
    transaction
        .execute_batch(HOST_SCHEMA_SQL)
        .map_err(StateError::Sqlite)?;
    transaction
        .execute_batch(HOST_SCHEMA_13_PROJECT_SQL)
        .map_err(StateError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO host_identity (
                singleton, host_id, registry_generation, schema_version
             ) VALUES (1, ?1, ?2, 13)",
            params![
                HostId::from(id_generator.uuid()).to_string(),
                id_generator.uuid().to_string(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    transaction
        .execute("PRAGMA user_version = 13", [])
        .map_err(StateError::Sqlite)?;
    transaction.commit().map_err(StateError::Sqlite)
}

fn build_project_plan(
    connection: &Connection,
    id_generator: &dyn IdGenerator,
) -> Result<ProjectPlan, StateError> {
    let locations = read_locations(connection)?;
    let mut groups = BTreeMap::<Option<String>, Vec<&LocationForProject>>::new();
    for location in &locations {
        if let Some(fingerprint) = &location.fingerprint {
            groups
                .entry(Some(fingerprint.clone()))
                .or_default()
                .push(location);
        } else {
            groups.entry(None).or_default().push(location);
        }
    }
    let mut projects = Vec::new();
    for (fingerprint, mut members) in groups {
        members.sort_by_key(|location| location.location_id);
        if fingerprint.is_none() {
            for location in members {
                projects.push(PlannedProject {
                    project_id: ProjectId::from(id_generator.uuid()),
                    label_location_id: location.location_id,
                    display_name: location.display_name.clone(),
                    repository_fingerprint: None,
                    locations: vec![location.location_id],
                });
            }
        } else {
            let first = members[0];
            projects.push(PlannedProject {
                project_id: ProjectId::from(id_generator.uuid()),
                label_location_id: first.location_id,
                display_name: first.display_name.clone(),
                repository_fingerprint: fingerprint,
                locations: members
                    .iter()
                    .map(|location| location.location_id)
                    .collect(),
            });
        }
    }
    projects.sort_by_key(|project| project.project_id);
    Ok(ProjectPlan {
        locations,
        projects,
    })
}

fn read_locations(connection: &Connection) -> Result<Vec<LocationForProject>, StateError> {
    let mut statement = connection
        .prepare(
            "SELECT location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display, revision
             FROM project_locations ORDER BY location_id",
        )
        .map_err(StateError::Sqlite)?;
    statement
        .query_map([], |row| {
            let location_id: String = row.get(0)?;
            let repository_path: String = row.get(1)?;
            let display_name: String = row.get(2)?;
            let fingerprint: Option<String> = row.get(3)?;
            let remote_display: Option<String> = row.get(4)?;
            let revision: i64 = row.get(5)?;
            Ok((
                location_id,
                repository_path,
                display_name,
                fingerprint,
                remote_display,
                revision,
            ))
        })
        .map_err(StateError::Sqlite)?
        .map(|row| {
            let (
                location_id,
                repository_path,
                display_name,
                fingerprint,
                remote_display,
                revision,
            ) =
                row.map_err(StateError::Sqlite)?;
            let location_id = location_id
                .parse::<LocationId>()
                .map_err(StateError::InvalidPersistedUuid)?;
            validate_project_display_name(&display_name)?;
            let fingerprint = normalize_persisted_fingerprint(fingerprint.as_deref());
            validate_safe_origin_display(remote_display.as_deref())?;
            let remote_display = remote_display.unwrap_or_default();
            let revision = Revision::try_from(revision)?;
            Ok(LocationForProject {
                location_id,
                repository_path,
                display_name,
                fingerprint,
                remote_display,
                revision,
                project_id: None,
            })
        })
        .collect()
}

fn normalize_persisted_fingerprint(value: Option<&str>) -> Option<String> {
    let value = value.filter(|value| !value.is_empty())?;
    if validate_repository_fingerprint(Some(value)).is_ok() {
        Some(value.to_owned())
    } else {
        // An old or ambiguous origin is deliberately ungrouped rather than
        // guessed into a Project.  The raw bounded location field remains
        // untouched by migration.
        None
    }
}

fn validate_safe_origin_display(value: Option<&str>) -> Result<(), StateError> {
    if value.is_some_and(str::is_empty) {
        return Ok(());
    }
    validate_remote_identity_display(value)
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    let project_id: String = row.get(0)?;
    let label_location_id: String = row.get(1)?;
    let revision: i64 = row.get(4)?;
    Ok(ProjectRecord {
        project_id: project_id.parse().map_err(to_sql_error)?,
        label_location_id: label_location_id.parse().map_err(to_sql_error)?,
        display_name: row.get(2)?,
        repository_fingerprint: row.get(3)?,
        revision: Revision::try_from(revision).map_err(domain_to_sql_error)?,
    })
}

fn load_project_browser_root_revision(connection: &Connection) -> Result<Revision, StateError> {
    let revision: Option<i64> = connection
        .query_row(
            "SELECT revision FROM project_browser_settings WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    revision
        .map(Revision::try_from)
        .transpose()
        .map_err(StateError::from)
        .map(|revision| revision.unwrap_or(Revision::INITIAL))
}

fn to_sql_error(error: uuid::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn domain_to_sql_error(error: crate::domain::DomainError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error))
}

fn query_projects(connection: &Connection) -> Result<Vec<ProjectRecord>, StateError> {
    let mut statement = connection
        .prepare(
            "SELECT project_id, label_location_id, display_name,
                    repository_fingerprint, revision
             FROM projects ORDER BY project_id",
        )
        .map_err(StateError::Sqlite)?;
    statement
        .query_map([], row_to_project)
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)
}

fn load_project_projections(connection: &Connection) -> Result<Vec<ProjectProjection>, StateError> {
    let projects = query_projects(connection)?;
    if projects.len() > MAX_PROJECT_PROJECTION_PROJECTS {
        return Err(StateError::InvalidPersistedValue(
            "too many Project projection rows".to_owned(),
        ));
    }
    let mut total_locations = 0_usize;
    let mut projections = Vec::with_capacity(projects.len());
    for project in projects {
        let mut statement = connection
            .prepare(
                "SELECT location_id, repository_display_name,
                        remote_identity_fingerprint, remote_identity_display, revision
                 FROM project_locations WHERE project_id = ?1 ORDER BY location_id",
            )
            .map_err(StateError::Sqlite)?;
        let locations = statement
            .query_map([project.project_id.to_string()], |row| {
                let location_id: String = row.get(0)?;
                let display_name: String = row.get(1)?;
                let fingerprint: Option<String> = row.get(2)?;
                let origin_display: Option<String> = row.get(3)?;
                let revision: i64 = row.get(4)?;
                Ok((
                    location_id,
                    display_name,
                    fingerprint,
                    origin_display,
                    revision,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        if locations.is_empty() || locations.len() > MAX_PROJECT_PROJECTION_LOCATIONS {
            return Err(StateError::MalformedHostSchema);
        }
        total_locations = total_locations
            .checked_add(locations.len())
            .ok_or_else(|| {
                StateError::InvalidPersistedValue("Project projection size".to_owned())
            })?;
        if total_locations > MAX_PROJECT_PROJECTION_LOCATIONS {
            return Err(StateError::InvalidPersistedValue(
                "too many Project Location projection rows".to_owned(),
            ));
        }
        let mut projected_locations = Vec::with_capacity(locations.len());
        for (location_id, display_name, fingerprint, origin_display, revision) in locations {
            let location_id = location_id
                .parse::<LocationId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            validate_project_display_name(&display_name)?;
            validate_safe_origin_display(origin_display.as_deref())?;
            let revision = Revision::try_from(revision)?;
            projected_locations.push(ProjectLocationProjection {
                project_id: project.project_id,
                location_id,
                revision,
                is_label_source: location_id == project.label_location_id,
                display_name,
                repository_fingerprint: normalize_persisted_fingerprint(fingerprint.as_deref()),
                origin_display: origin_display.filter(|value| !value.is_empty()),
            });
        }
        projections.push(ProjectProjection {
            project_id: project.project_id,
            revision: project.revision,
            label_location_id: project.label_location_id,
            display_name: project.display_name,
            repository_fingerprint: project.repository_fingerprint,
            locations: projected_locations,
        });
    }
    Ok(projections)
}

fn validate_project_source_transaction(
    transaction: &rusqlite::Transaction<'_>,
    project: &ProjectRecord,
) -> Result<(), StateError> {
    if project.display_name.trim().is_empty() {
        return Err(StateError::InvalidPersistedValue(
            "empty Project display name".to_owned(),
        ));
    }
    let membership: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM project_locations
             WHERE project_id = ?1 AND location_id = ?2",
            params![
                project.project_id.to_string(),
                project.label_location_id.to_string()
            ],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if membership != 1 {
        return Err(StateError::MalformedHostSchema);
    }
    let (display_name, source_fingerprint): (String, Option<String>) = transaction
        .query_row(
            "SELECT repository_display_name, remote_identity_fingerprint
             FROM project_locations WHERE project_id = ?1 AND location_id = ?2",
            params![
                project.project_id.to_string(),
                project.label_location_id.to_string()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(StateError::Sqlite)?;
    if display_name != project.display_name
        || !project_source_fingerprints_compatible(
            project.repository_fingerprint.as_deref(),
            source_fingerprint.as_deref(),
        )
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

fn project_source_fingerprints_compatible(
    project_fingerprint: Option<&str>,
    source_fingerprint: Option<&str>,
) -> bool {
    let project_fingerprint = project_fingerprint.filter(|value| !value.is_empty());
    let source_fingerprint = normalize_persisted_fingerprint(source_fingerprint);
    match (project_fingerprint, source_fingerprint) {
        // A missing later observation is allowed to retain the Project's last
        // positive fingerprint without clearing the durable association.
        (Some(_) | None, None) => true,
        (Some(project), Some(source)) => project == source,
        (None, Some(_)) => false,
    }
}

fn load_location_for_update(
    transaction: &rusqlite::Transaction<'_>,
    location_id: LocationId,
) -> Result<LocationForProject, StateError> {
    transaction
        .query_row(
            "SELECT location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display,
                    revision, project_id
             FROM project_locations WHERE location_id = ?1",
            [location_id.to_string()],
            |row| {
                let location_id: String = row.get(0)?;
                let repository_path: String = row.get(1)?;
                let display_name: String = row.get(2)?;
                let fingerprint: Option<String> = row.get(3)?;
                let remote_display: Option<String> = row.get(4)?;
                let revision: i64 = row.get(5)?;
                let project_id: Option<String> = row.get(6)?;
                Ok((
                    location_id,
                    repository_path,
                    display_name,
                    fingerprint,
                    remote_display,
                    revision,
                    project_id,
                ))
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                StateError::InvalidPersistedValue("unknown ProjectLocation".to_owned())
            }
            error => StateError::Sqlite(error),
        })
        .and_then(
            |(
                location_id,
                repository_path,
                display_name,
                fingerprint,
                remote_display,
                revision,
                project_id,
            )| {
                Ok(LocationForProject {
                    location_id: location_id
                        .parse()
                        .map_err(StateError::InvalidPersistedUuid)?,
                    repository_path,
                    display_name,
                    fingerprint: normalize_persisted_fingerprint(fingerprint.as_deref()),
                    remote_display: remote_display.unwrap_or_default(),
                    revision: Revision::try_from(revision)?,
                    project_id: project_id
                        .map(|value| value.parse().map_err(StateError::InvalidPersistedUuid))
                        .transpose()?,
                })
            },
        )
}

fn load_project_members(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
) -> Result<Vec<LocationForProject>, StateError> {
    let mut statement = transaction
        .prepare(
            "SELECT location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display,
                    revision, project_id
             FROM project_locations WHERE project_id = ?1 ORDER BY location_id",
        )
        .map_err(StateError::Sqlite)?;
    statement
        .query_map([project_id.to_string()], |row| {
            let location_id: String = row.get(0)?;
            let repository_path: String = row.get(1)?;
            let display_name: String = row.get(2)?;
            let fingerprint: Option<String> = row.get(3)?;
            let remote_display: Option<String> = row.get(4)?;
            let revision: i64 = row.get(5)?;
            let project_id: Option<String> = row.get(6)?;
            Ok((
                location_id,
                repository_path,
                display_name,
                fingerprint,
                remote_display,
                revision,
                project_id,
            ))
        })
        .map_err(StateError::Sqlite)?
        .map(|row| {
            let (
                location_id,
                repository_path,
                display_name,
                fingerprint,
                remote_display,
                revision,
                project_id,
            ) = row.map_err(StateError::Sqlite)?;
            let project_id = project_id
                .ok_or(StateError::MalformedHostSchema)?
                .parse()
                .map_err(|_| StateError::MalformedHostSchema)?;
            Ok(LocationForProject {
                location_id: location_id
                    .parse()
                    .map_err(|_| StateError::MalformedHostSchema)?,
                repository_path,
                display_name,
                fingerprint: normalize_persisted_fingerprint(fingerprint.as_deref()),
                remote_display: remote_display.unwrap_or_default(),
                revision: Revision::try_from(revision)
                    .map_err(|_| StateError::MalformedHostSchema)?,
                project_id: Some(project_id),
            })
        })
        .collect()
}

fn reconcile_location_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    member: &ProjectRefreshMember,
    id_generator: &dyn IdGenerator,
) -> Result<(), StateError> {
    let location_id = member.location_id;
    let expected_revision = member.expected_revision;
    let observation = &member.observation;
    let display_name = observation.display_name.as_str();
    let fingerprint = observation
        .repository_fingerprint
        .as_deref()
        .filter(|value| !value.is_empty());
    let location = load_location_for_update(transaction, location_id)?;
    if location.revision != expected_revision {
        return Err(StateError::ConcurrentWrite);
    }
    let existing_project = location.project_id;
    let matching_project = fingerprint
        .map(|value| find_project_by_fingerprint(transaction, value))
        .transpose()?
        .flatten();

    let mut project_revision_bumped = false;
    let target_project = match (existing_project, matching_project) {
        (None, Some(project)) => {
            bump_project_revision(transaction, project.project_id)?;
            project_revision_bumped = true;
            project.project_id
        }
        (None, None) => {
            create_project(
                transaction,
                location_id,
                display_name,
                fingerprint,
                id_generator,
            )?
            .project_id
        }
        (Some(existing), Some(matching)) if existing == matching.project_id => existing,
        (Some(existing), Some(matching)) => {
            move_location(transaction, location_id, matching.project_id)?;
            bump_project_revision(transaction, matching.project_id)?;
            repair_project_after_departure(transaction, existing)?;
            matching.project_id
        }
        (Some(existing), None) if fingerprint.is_none() => existing,
        (Some(existing), None) => {
            let members = project_member_count(transaction, existing)?;
            if members == 1 {
                transaction
                    .execute(
                        "UPDATE projects SET repository_fingerprint = ?1,
                         revision = revision + 1 WHERE project_id = ?2",
                        params![fingerprint, existing.to_string()],
                    )
                    .map_err(StateError::Sqlite)?;
                project_revision_bumped = true;
                existing
            } else {
                let fresh = create_project(
                    transaction,
                    location_id,
                    display_name,
                    fingerprint,
                    id_generator,
                )?;
                move_location(transaction, location_id, fresh.project_id)?;
                repair_project_after_departure(transaction, existing)?;
                fresh.project_id
            }
        }
    };

    transaction
        .execute(
            "UPDATE project_locations
             SET project_id = ?1, repository_display_name = ?2,
                 remote_identity_fingerprint = ?3, remote_identity_display = ?4,
                 revision = revision + 1
             WHERE location_id = ?5 AND revision = ?6",
            params![
                target_project.to_string(),
                display_name,
                fingerprint,
                observation.remote_identity_display,
                location_id.to_string(),
                expected_revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if transaction.changes() != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    refresh_project_label_if_source(
        transaction,
        target_project,
        location_id,
        display_name,
        project_revision_bumped,
    )
}

fn validate_project_membership_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StateError> {
    let mut projects = transaction
        .prepare("SELECT project_id, label_location_id, display_name, repository_fingerprint, revision FROM projects")
        .map_err(StateError::Sqlite)?;
    let rows = projects
        .query_map([], row_to_project)
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    for project in rows {
        validate_project_source_transaction(transaction, &project)?;
        let members = project_member_count(transaction, project.project_id)?;
        if members == 0 {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

fn find_project_by_fingerprint(
    transaction: &rusqlite::Transaction<'_>,
    fingerprint: &str,
) -> Result<Option<ProjectRecord>, StateError> {
    transaction
        .query_row(
            "SELECT project_id, label_location_id, display_name,
                    repository_fingerprint, revision
             FROM projects WHERE repository_fingerprint = ?1",
            [fingerprint],
            row_to_project,
        )
        .optional()
        .map_err(StateError::Sqlite)
}

fn create_project(
    transaction: &rusqlite::Transaction<'_>,
    location_id: LocationId,
    display_name: &str,
    fingerprint: Option<&str>,
    id_generator: &dyn IdGenerator,
) -> Result<ProjectRecord, StateError> {
    let project_id = ProjectId::from(id_generator.uuid());
    transaction
        .execute(
            "INSERT INTO projects (
                project_id, label_location_id, display_name,
                repository_fingerprint, revision
             ) VALUES (?1, ?2, ?3, ?4, 1)",
            params![
                project_id.to_string(),
                location_id.to_string(),
                display_name,
                fingerprint,
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(ProjectRecord {
        project_id,
        label_location_id: location_id,
        display_name: display_name.to_owned(),
        repository_fingerprint: fingerprint.map(str::to_owned),
        revision: Revision::INITIAL,
    })
}

fn move_location(
    transaction: &rusqlite::Transaction<'_>,
    location_id: LocationId,
    project_id: ProjectId,
) -> Result<(), StateError> {
    transaction
        .execute(
            "UPDATE project_locations SET project_id = ?1 WHERE location_id = ?2",
            params![project_id.to_string(), location_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    Ok(())
}

fn bump_project_revision(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
) -> Result<(), StateError> {
    transaction
        .execute(
            "UPDATE projects SET revision = revision + 1 WHERE project_id = ?1",
            [project_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    if transaction.changes() != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    Ok(())
}

fn project_member_count(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
) -> Result<i64, StateError> {
    transaction
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

fn repair_project_after_departure(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
) -> Result<(), StateError> {
    let member: Option<(String, String)> = transaction
        .query_row(
            "SELECT location_id, repository_display_name
             FROM project_locations WHERE project_id = ?1
             ORDER BY location_id LIMIT 1",
            [project_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((location_id, display_name)) = member else {
        transaction
            .execute(
                "DELETE FROM projects WHERE project_id = ?1",
                [project_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        return Ok(());
    };
    let current_source: String = transaction
        .query_row(
            "SELECT label_location_id FROM projects WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    let source_still_member: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM project_locations
             WHERE project_id = ?1 AND location_id = ?2)",
            params![project_id.to_string(), current_source],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if source_still_member {
        bump_project_revision(transaction, project_id)?;
    } else {
        transaction
            .execute(
                "UPDATE projects SET label_location_id = ?1, display_name = ?2,
                 revision = revision + 1 WHERE project_id = ?3",
                params![location_id, display_name, project_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
    }
    Ok(())
}

fn refresh_project_label_if_source(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
    location_id: LocationId,
    display_name: &str,
    project_revision_bumped: bool,
) -> Result<(), StateError> {
    let source: String = transaction
        .query_row(
            "SELECT label_location_id FROM projects WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if source == location_id.to_string() {
        // The project label is updated as part of the location observation
        // only when this exact location is the durable source.
        let query = if project_revision_bumped {
            "UPDATE projects SET display_name = ?1
             WHERE project_id = ?2 AND label_location_id = ?3
               AND display_name != ?1"
        } else {
            "UPDATE projects SET display_name = ?1, revision = revision + 1
             WHERE project_id = ?2 AND label_location_id = ?3
               AND display_name != ?1"
        };
        transaction
            .execute(
                query,
                params![
                    display_name,
                    project_id.to_string(),
                    location_id.to_string()
                ],
            )
            .map_err(StateError::Sqlite)?;
    }
    Ok(())
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

fn validate_transition_root(root: &Path) -> Result<(PathBuf, FileIdentity), StateError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| StateError::io(root, error))?;
    if !metadata.is_dir() || !is_private_owner_directory(&metadata) {
        return Err(StateError::InvalidTransitionLease);
    }
    let canonical = fs::canonicalize(root).map_err(|error| StateError::io(root, error))?;
    Ok((canonical, file_identity(&metadata)))
}

fn open_private_transition_file(path: &Path, create_new: bool) -> Result<File, StateError> {
    let before = exact_artifact_metadata(path)?;
    if let Some(metadata) = &before
        && (!metadata.is_file() || !is_private_owner_file(metadata))
    {
        return Err(StateError::InvalidTransitionLease);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let after = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !after.is_file() || (before.is_some() && !is_private_owner_file(&after)) {
        return Err(StateError::InvalidTransitionLease);
    }
    if let Some(before) = before
        && file_identity(&before) != file_identity(&after)
    {
        return Err(StateError::InvalidTransitionLease);
    }
    set_private_file_permissions_handle(&file, path)?;
    let after_permissions = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !after_permissions.is_file() || !is_private_owner_file(&after_permissions) {
        return Err(StateError::InvalidTransitionLease);
    }
    Ok(file)
}

fn open_private_provisional_file(path: &Path, create_new: bool) -> Result<File, StateError> {
    let before = exact_artifact_metadata(path)?;
    if let Some(metadata) = &before
        && (!metadata.is_file() || !is_private_owner_file(metadata))
    {
        return Err(StateError::InvalidProvisionalLease);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StateError::InvalidProvisionalLease
        } else {
            StateError::io(path, error)
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

fn create_private_database_file(path: &Path) -> Result<File, StateError> {
    if exact_artifact_metadata(path)?.is_some() {
        return Err(StateError::StateRecoveryRequired(
            StateRecoveryReason::UnknownFreshRootArtifact,
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
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

fn set_private_file_permissions_handle(file: &File, path: &Path) -> Result<(), StateError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| StateError::io(path, error))
    }
    #[cfg(not(unix))]
    {
        super::utils::set_private_file_permissions(path)
    }
}

fn open_private_create_new_file(path: &Path) -> Result<File, StateError> {
    if exact_artifact_metadata(path)?.is_some() {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !metadata.is_file() || !is_current_owner(&metadata) {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    set_private_file_permissions_handle(&file, path)?;
    let private_metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !private_metadata.is_file() || !is_private_owner_file(&private_metadata) {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    Ok(file)
}

fn probe_transition_lock(path: &Path) -> Result<(), StateError> {
    let file = open_private_transition_file(path, false).map_err(|error| match error {
        StateError::InvalidTransitionLease => {
            StateError::FreshRootRejected(FreshRootRejection::NonPrivateTransitionLease)
        }
        other => other,
    })?;
    match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
        Ok(lock) => {
            drop(lock);
            Ok(())
        }
        Err((_file, _error)) => Err(StateError::FreshRootRejected(
            FreshRootRejection::LockedTransitionLease,
        )),
    }
}

/// Acquires the explicit transition lease required by confirmed migration and
/// journal mutation.  The lock is never created by this API.
pub fn acquire_transition_lease(root: &Path) -> Result<TransitionLease, StateError> {
    TransitionLease::acquire(root)
}

fn sync_directory(path: &Path) -> Result<(), StateError> {
    let directory = File::open(path).map_err(|error| StateError::io(path, error))?;
    directory
        .sync_all()
        .map_err(|error| StateError::io(path, error))
}

/// A monotonic budget for observer `SQLite` work.  Only `BUSY` and `LOCKED`
/// errors are retried; all other database errors return immediately.
#[derive(Clone, Copy, Debug)]
pub struct ObserverDatabaseDeadline {
    deadline: Instant,
    retry_delay: Duration,
}

impl ObserverDatabaseDeadline {
    #[must_use]
    pub fn from_now(duration: Duration) -> Self {
        Self {
            deadline: Instant::now() + duration,
            retry_delay: Duration::from_millis(1),
        }
    }

    #[must_use]
    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            retry_delay: Duration::from_millis(1),
        }
    }

    #[must_use]
    pub fn deadline(self) -> Instant {
        self.deadline
    }

    pub fn run<T, F>(self, operation: F) -> Result<T, ObserverDatabaseError>
    where
        F: FnMut() -> Result<T, rusqlite::Error>,
    {
        self.run_with_degraded_reason(operation)
            .map_err(|(error, _)| error)
    }

    fn run_with_degraded_reason<T, F>(
        self,
        mut operation: F,
    ) -> Result<T, (ObserverDatabaseError, ObserverDegradedReason)>
    where
        F: FnMut() -> Result<T, rusqlite::Error>,
    {
        if Instant::now() >= self.deadline {
            return Err((
                ObserverDatabaseError::DeadlineExceeded,
                ObserverDegradedReason::BusyDeadline,
            ));
        }
        let mut retry_reason = ObserverDegradedReason::BusyDeadline;
        loop {
            match operation() {
                Ok(value) => {
                    return Ok(value);
                }
                Err(error) if is_retryable_observer_error(&error) => {
                    retry_reason = observer_retry_reason(&error);
                    let now = Instant::now();
                    if now >= self.deadline {
                        return Err((ObserverDatabaseError::DeadlineExceeded, retry_reason));
                    }
                    let remaining = self.deadline.saturating_duration_since(now);
                    thread_sleep(self.retry_delay.min(remaining));
                }
                Err(error) => {
                    return Err((ObserverDatabaseError::Sqlite(error), retry_reason));
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum ObserverDatabaseError {
    DeadlineExceeded,
    Sqlite(rusqlite::Error),
}

/// Runs one bounded observer operation and records the closed degraded marker
/// when only retryable contention survives until the deadline.
pub fn run_observer_write_with_degraded_marker<T, F>(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
    deadline: ObserverDatabaseDeadline,
    operation: F,
) -> Result<T, StateError>
where
    F: FnMut() -> Result<T, rusqlite::Error>,
{
    match deadline.run_with_degraded_reason(operation) {
        Ok(value) => {
            clear_observer_degraded_marker(root, runtime_id, runtime_generation)?;
            Ok(value)
        }
        Err((ObserverDatabaseError::DeadlineExceeded, reason)) => {
            write_observer_degraded_marker(root, runtime_id, runtime_generation, reason)?;
            Err(StateError::ObserverDatabaseDeadlineExceeded)
        }
        Err((ObserverDatabaseError::Sqlite(error), _)) => {
            write_observer_degraded_marker(
                root,
                runtime_id,
                runtime_generation,
                ObserverDegradedReason::CommitFailed,
            )?;
            Err(StateError::Sqlite(error))
        }
    }
}

fn is_retryable_observer_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            )
    )
}

fn observer_retry_reason(error: &rusqlite::Error) -> ObserverDegradedReason {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::DatabaseLocked => {
            ObserverDegradedReason::LockedDeadline
        }
        _ => ObserverDegradedReason::BusyDeadline,
    }
}

fn thread_sleep(duration: Duration) {
    if !duration.is_zero() {
        std::thread::sleep(duration);
    }
}

/// Closed marker reason recorded after exact observer authority has been
/// established but a bounded `SQLite` commit cannot complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverDegradedReason {
    BusyDeadline,
    LockedDeadline,
    CommitFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserverDegradedMarkerWire {
    version: u8,
    runtime_id: String,
    runtime_generation: String,
    reason: ObserverDegradedReason,
}

/// Computes the one exact marker path for a Runtime generation.  Callers do
/// not discover markers by scanning the run tree.
pub fn observer_degraded_marker_path(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<PathBuf, StateError> {
    validate_generation(runtime_generation)?;
    let mut digest = Sha256::new();
    digest.update(runtime_generation.as_bytes());
    let digest = digest.finalize();
    let mut digest_hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut digest_hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(root
        .join("run")
        .join(runtime_id.to_string())
        .join("observer-degraded")
        .join(digest_hex))
}

fn observer_degraded_marker_temp_path(path: &Path) -> PathBuf {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("marker");
    path.with_file_name(format!("{filename}{OBSERVER_DEGRADED_MARKER_TEMP_SUFFIX}"))
}

fn check_observer_marker_deadline(deadline: Instant) -> Result<(), StateError> {
    if Instant::now() >= deadline {
        Err(StateError::ObserverDatabaseDeadlineExceeded)
    } else {
        Ok(())
    }
}

/// Writes or verifies one private, bounded degraded marker.  The body carries
/// no event, turn, message, payload, or diagnostic text.
#[allow(
    clippy::too_many_lines,
    reason = "The atomic marker protocol keeps exact-path validation, promotion, and crash recovery in one bounded operation."
)]
pub fn write_observer_degraded_marker(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
    reason: ObserverDegradedReason,
) -> Result<PathBuf, StateError> {
    write_observer_degraded_marker_with_deadline(
        root,
        runtime_id,
        runtime_generation,
        reason,
        Instant::now() + OBSERVER_DEGRADED_MARKER_BUDGET,
    )
}

/// Writes or verifies one marker until the supplied monotonic deadline. The
/// default helper above starts the fixed 250 ms outer margin at entry; this
/// variant lets the observer transition preserve one absolute cutoff when a
/// caller has already started that margin.
#[allow(
    clippy::too_many_lines,
    reason = "The atomic marker protocol keeps exact-path validation, promotion, and crash recovery in one bounded operation."
)]
pub fn write_observer_degraded_marker_with_deadline(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
    reason: ObserverDegradedReason,
    deadline: Instant,
) -> Result<PathBuf, StateError> {
    check_observer_marker_deadline(deadline)?;
    let path = observer_degraded_marker_path(root, runtime_id, runtime_generation)?;
    check_observer_marker_deadline(deadline)?;
    let temp_path = observer_degraded_marker_temp_path(&path);
    let parent = path
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let runtime_directory = parent
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let run_directory = runtime_directory
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    check_observer_marker_deadline(deadline)?;
    let root_metadata =
        exact_artifact_metadata(root)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
    if !root_metadata.is_dir() || !is_private_owner_directory(&root_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    // Create each exact derived directory separately.  `create_dir_all` can
    // follow a swapped-in symlink before the later metadata check gets a
    // chance to reject it.
    for directory in [run_directory, runtime_directory, parent] {
        check_observer_marker_deadline(deadline)?;
        ensure_private_marker_directory(directory)?;
    }
    check_observer_marker_deadline(deadline)?;
    let wire = ObserverDegradedMarkerWire {
        version: 1,
        runtime_id: runtime_id.to_string(),
        runtime_generation: runtime_generation.to_owned(),
        reason,
    };
    let body = serde_json::to_vec(&wire).map_err(|_| StateError::InvalidObserverDegradedMarker)?;
    if body.len() > 1024 {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    check_observer_marker_deadline(deadline)?;
    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
        &path,
        &temp_path,
        runtime_id,
        runtime_generation,
    )?;
    check_observer_marker_deadline(deadline)?;
    match (final_reason, temp_reason) {
        (Some(final_reason), Some(temp_reason)) => {
            if final_reason != temp_reason || final_reason != reason {
                return Err(StateError::InvalidObserverDegradedMarker);
            }
            return Ok(path);
        }
        (Some(final_reason), None) => {
            if final_reason != reason {
                return Err(StateError::InvalidObserverDegradedMarker);
            }
            return Ok(path);
        }
        (None, Some(temp_reason)) => {
            if temp_reason != reason {
                return Err(StateError::InvalidObserverDegradedMarker);
            }
            match fs::rename(&temp_path, &path) {
                Ok(()) => {
                    check_observer_marker_deadline(deadline)?;
                    sync_directory(parent)?;
                    check_observer_marker_deadline(deadline)?;
                    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
                        &path,
                        &temp_path,
                        runtime_id,
                        runtime_generation,
                    )?;
                    check_observer_marker_deadline(deadline)?;
                    if final_reason == Some(reason)
                        && temp_reason.is_none_or(|value| value == reason)
                    {
                        return Ok(path);
                    }
                    return Err(StateError::InvalidObserverDegradedMarker);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    check_observer_marker_deadline(deadline)?;
                    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
                        &path,
                        &temp_path,
                        runtime_id,
                        runtime_generation,
                    )?;
                    check_observer_marker_deadline(deadline)?;
                    if final_reason == Some(reason)
                        && temp_reason.is_none_or(|value| value == reason)
                    {
                        return Ok(path);
                    }
                    return Err(StateError::InvalidObserverDegradedMarker);
                }
                Err(error) => return Err(StateError::io(&path, error)),
            }
        }
        (None, None) => {}
    }
    check_observer_marker_deadline(deadline)?;
    let mut file = match open_private_create_new_file(&temp_path).map_err(|error| match error {
        StateError::InvalidObserverHandoverJournal => StateError::InvalidObserverDegradedMarker,
        other => other,
    }) {
        Ok(file) => file,
        Err(error) => {
            check_observer_marker_deadline(deadline)?;
            let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
                &path,
                &temp_path,
                runtime_id,
                runtime_generation,
            )?;
            check_observer_marker_deadline(deadline)?;
            if final_reason == Some(reason) && temp_reason.is_none_or(|value| value == reason) {
                return Ok(path);
            }
            return Err(error);
        }
    };
    check_observer_marker_deadline(deadline)?;
    file.write_all(&body)
        .map_err(|error| StateError::io(&temp_path, error))?;
    check_observer_marker_deadline(deadline)?;
    file.sync_all()
        .map_err(|error| StateError::io(&temp_path, error))?;
    check_observer_marker_deadline(deadline)?;
    fs::rename(&temp_path, &path).map_err(|error| StateError::io(&path, error))?;
    check_observer_marker_deadline(deadline)?;
    sync_directory(parent)?;
    check_observer_marker_deadline(deadline)?;
    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
        &path,
        &temp_path,
        runtime_id,
        runtime_generation,
    )?;
    check_observer_marker_deadline(deadline)?;
    if final_reason == Some(reason) && temp_reason.is_none_or(|value| value == reason) {
        Ok(path)
    } else {
        Err(StateError::InvalidObserverDegradedMarker)
    }
}

/// Reads only the exact generation-derived marker path.
pub fn read_observer_degraded_marker(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<Option<ObserverDegradedReason>, StateError> {
    validate_observer_marker_ancestors(root, runtime_id)?;
    let path = observer_degraded_marker_path(root, runtime_id, runtime_generation)?;
    let temp_path = observer_degraded_marker_temp_path(&path);
    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
        &path,
        &temp_path,
        runtime_id,
        runtime_generation,
    )?;
    match (final_reason, temp_reason) {
        (None, None) => Ok(None),
        (Some(reason), None) | (None, Some(reason)) => Ok(Some(reason)),
        (Some(final_reason), Some(temp_reason)) if final_reason == temp_reason => {
            Ok(Some(final_reason))
        }
        (Some(_), Some(_)) => Err(StateError::InvalidObserverDegradedMarker),
    }
}

/// Removes only the exact current-generation degraded marker and its exact
/// temporary candidate after validating both candidates.  This is an
/// explicit reconciliation step: it never scans the run tree, follows a
/// derived symlink, or touches a marker for another Runtime generation.
pub fn clear_observer_degraded_marker(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<(), StateError> {
    // Reading first validates the root ancestry, exact wire identity, and
    // agreement between final and temporary candidates.  A malformed or
    // foreign candidate therefore fails closed before anything is removed.
    if read_observer_degraded_marker(root, runtime_id, runtime_generation)?.is_none() {
        return Ok(());
    }
    let path = observer_degraded_marker_path(root, runtime_id, runtime_generation)?;
    let temp_path = observer_degraded_marker_temp_path(&path);
    remove_observer_degraded_marker_candidate(&path, runtime_id, runtime_generation)?;
    remove_observer_degraded_marker_candidate(&temp_path, runtime_id, runtime_generation)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn remove_observer_degraded_marker_candidate(
    path: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<(), StateError> {
    let Some(metadata) = exact_artifact_metadata(path)? else {
        return Ok(());
    };
    // Re-read the exact candidate immediately before removal and compare its
    // identity with the earlier lstat.  This rejects a foreign or swapped-in
    // candidate instead of unlinking an unrelated path at the derived name.
    read_observer_degraded_marker_candidate(path, runtime_id, runtime_generation)?
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let current =
        exact_artifact_metadata(path)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
    if file_identity(&metadata) != file_identity(&current)
        || !current.is_file()
        || !is_private_owner_file(&current)
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StateError::io(path, error)),
    }
}

fn validate_observer_marker_ancestors(
    root: &Path,
    runtime_id: RuntimeId,
) -> Result<(), StateError> {
    let Some(root_metadata) = exact_artifact_metadata(root)? else {
        return Ok(());
    };
    if !root_metadata.is_dir() || !is_private_owner_directory(&root_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let run_directory = root.join("run");
    let Some(run_metadata) = exact_artifact_metadata(&run_directory)? else {
        return Ok(());
    };
    if !run_metadata.is_dir() || !is_private_owner_directory(&run_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let runtime_directory = run_directory.join(runtime_id.to_string());
    let Some(runtime_metadata) = exact_artifact_metadata(&runtime_directory)? else {
        return Ok(());
    };
    if !runtime_metadata.is_dir() || !is_private_owner_directory(&runtime_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let marker_directory = runtime_directory.join("observer-degraded");
    if let Some(marker_metadata) = exact_artifact_metadata(&marker_directory)?
        && (!marker_metadata.is_dir() || !is_private_owner_directory(&marker_metadata))
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(())
}

fn read_observer_degraded_marker_candidates(
    final_path: &Path,
    temp_path: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<
    (
        Option<ObserverDegradedReason>,
        Option<ObserverDegradedReason>,
    ),
    StateError,
> {
    Ok((
        read_observer_degraded_marker_candidate(final_path, runtime_id, runtime_generation)?,
        read_observer_degraded_marker_candidate(temp_path, runtime_id, runtime_generation)?,
    ))
}

fn read_observer_degraded_marker_candidate(
    path: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<Option<ObserverDegradedReason>, StateError> {
    let Some(path_metadata) = exact_artifact_metadata(path)? else {
        return Ok(None);
    };
    validate_observer_marker_directories(path)?;
    if !path_metadata.is_file() || !is_private_owner_file(&path_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !metadata.is_file()
        || !is_private_owner_file(&metadata)
        || file_identity(&metadata) != file_identity(&path_metadata)
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let mut body = Vec::new();
    file.take(1025)
        .read_to_end(&mut body)
        .map_err(|error| StateError::io(path, error))?;
    if body.len() > 1024 {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let wire: ObserverDegradedMarkerWire =
        serde_json::from_slice(&body).map_err(|_| StateError::InvalidObserverDegradedMarker)?;
    if wire.version != 1
        || wire.runtime_id != runtime_id.to_string()
        || wire.runtime_generation != runtime_generation
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(Some(wire.reason))
}

fn validate_observer_marker_directories(path: &Path) -> Result<(), StateError> {
    let parent = path
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let runtime_directory = parent
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let run_directory = runtime_directory
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let root = run_directory
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    for directory in [root, run_directory, runtime_directory, parent] {
        let metadata =
            exact_artifact_metadata(directory)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
        if !metadata.is_dir() || !is_private_owner_directory(&metadata) {
            return Err(StateError::InvalidObserverDegradedMarker);
        }
    }
    Ok(())
}

fn ensure_private_marker_directory(path: &Path) -> Result<(), StateError> {
    match exact_artifact_metadata(path)? {
        Some(metadata) if !metadata.is_dir() || !is_current_owner(&metadata) => {
            return Err(StateError::InvalidObserverDegradedMarker);
        }
        Some(_) => {}
        None => match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StateError::io(path, error)),
        },
    }
    let metadata =
        exact_artifact_metadata(path)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
    if !metadata.is_dir() || !is_current_owner(&metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut options = OpenOptions::new();
        options.read(true);
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
        let directory = options
            .open(path)
            .map_err(|error| StateError::io(path, error))?;
        let opened = directory
            .metadata()
            .map_err(|error| StateError::io(path, error))?;
        if !opened.is_dir() || file_identity(&opened) != file_identity(&metadata) {
            return Err(StateError::InvalidObserverDegradedMarker);
        }
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| StateError::io(path, error))?;
    }
    #[cfg(not(unix))]
    {
        super::utils::set_private_directory_permissions(path)?;
    }
    let private_metadata =
        fs::symlink_metadata(path).map_err(|error| StateError::io(path, error))?;
    if !private_metadata.is_dir() || !is_private_owner_directory(&private_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(())
}

fn validate_generation(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\0', '\n', '\r']) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(())
}

/// Process identity stored in a private handover journal.  It contains only
/// bounded corroboration fields and no provider or terminal content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverProcessIdentity {
    pub pid: u32,
    pub birth: String,
    pub executable: String,
}

impl ObserverProcessIdentity {
    fn validate(&self) -> Result<(), StateError> {
        if self.pid == 0
            || self.birth.is_empty()
            || self.birth.len() > 256
            || self.executable.is_empty()
            || self.executable.len() > 4096
            || self.birth.contains(['\0', '\n', '\r'])
            || self.executable.contains(['\0', '\n', '\r'])
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoverPhase {
    Prepared,
    StandbyReady,
    OldFrozen,
    HandleSwapped,
    OldCleaning,
    Complete,
}

impl HandoverPhase {
    fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Prepared, Self::StandbyReady)
                | (Self::StandbyReady, Self::OldFrozen)
                | (Self::OldFrozen, Self::HandleSwapped)
                | (Self::HandleSwapped, Self::OldCleaning)
                | (Self::OldCleaning, Self::Complete)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverHandoverJournal {
    pub version: u8,
    pub runtime_id: String,
    pub runtime_generation: String,
    pub old_observer: ObserverProcessIdentity,
    pub standby_observer: ObserverProcessIdentity,
    pub expected_handle_revision: Revision,
    pub phase: HandoverPhase,
}

/// Durable, privacy-bounded proof that the exact standby observed its
/// committed handle assignment and drained its ordered parsed-event buffer.
/// This survives launcher interruption without retaining provider payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverHandoverActivationAck {
    pub version: u8,
    pub runtime_id: String,
    pub runtime_generation: String,
    pub standby_observer: ObserverProcessIdentity,
    pub handle_revision: Revision,
}

impl ObserverHandoverActivationAck {
    pub fn validate(&self) -> Result<(), StateError> {
        if self.version != 1
            || self.runtime_id.parse::<RuntimeId>().is_err()
            || self.runtime_generation.is_empty()
            || self.runtime_generation.len() > 256
            || self.runtime_generation.contains(['\0', '\n', '\r'])
            || self.handle_revision.value() < 1
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
        self.standby_observer.validate()
    }

    pub fn matches_journal(&self, journal: &ObserverHandoverJournal) -> Result<bool, StateError> {
        self.validate()?;
        journal.validate()?;
        let assigned_revision = Revision::try_from(
            journal
                .expected_handle_revision
                .value()
                .checked_add(1)
                .ok_or(StateError::InvalidObserverHandoverJournal)?,
        )
        .map_err(|_| StateError::InvalidObserverHandoverJournal)?;
        Ok(self.runtime_id == journal.runtime_id
            && self.runtime_generation == journal.runtime_generation
            && self.standby_observer == journal.standby_observer
            && self.handle_revision == assigned_revision
            && matches!(
                journal.phase,
                HandoverPhase::HandleSwapped | HandoverPhase::OldCleaning | HandoverPhase::Complete
            ))
    }
}

impl ObserverHandoverJournal {
    pub fn validate(&self) -> Result<(), StateError> {
        if self.version != 1
            || self.runtime_id.parse::<RuntimeId>().is_err()
            || self.runtime_generation.is_empty()
            || self.runtime_generation.len() > 256
            || self.runtime_generation.contains(['\0', '\n', '\r'])
            || self.expected_handle_revision.value() < 1
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
        self.old_observer.validate()?;
        self.standby_observer.validate()?;
        if self.old_observer.pid == self.standby_observer.pid
            && self.old_observer.birth == self.standby_observer.birth
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
        Ok(())
    }

    pub fn transition(&mut self, next: HandoverPhase) -> Result<(), StateError> {
        if !self.phase.permits(next) {
            return Err(StateError::InvalidObserverHandoverTransition);
        }
        self.phase = next;
        Ok(())
    }

    pub fn restart_action(
        &self,
        current: &CurrentObserverHandleProof,
    ) -> Result<HandoverRestartAction, StateError> {
        self.validate()?;
        current.validate()?;
        let runtime_id = self
            .runtime_id
            .parse::<RuntimeId>()
            .map_err(|_| StateError::InvalidObserverHandoverJournal)?;
        if current.runtime_id != runtime_id || current.runtime_generation != self.runtime_generation
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
        let old = current.pid == self.old_observer.pid && current.birth == self.old_observer.birth;
        let standby = current.pid == self.standby_observer.pid
            && current.birth == self.standby_observer.birth;
        let expected_revision = self.expected_handle_revision;
        let next_revision = Revision::try_from(
            expected_revision
                .value()
                .checked_add(1)
                .ok_or(StateError::InvalidObserverHandoverJournal)?,
        )
        .map_err(|_| StateError::InvalidObserverHandoverJournal)?;
        match self.phase {
            HandoverPhase::Prepared | HandoverPhase::StandbyReady => {
                if old && current.revision == expected_revision {
                    Ok(HandoverRestartAction::RestoreOldObserver)
                } else {
                    Err(StateError::InvalidObserverHandoverJournal)
                }
            }
            HandoverPhase::OldFrozen => {
                if old && current.revision == expected_revision {
                    Ok(HandoverRestartAction::RestoreOldObserver)
                } else if standby && current.revision == next_revision {
                    Ok(HandoverRestartAction::FinishOldObserverCleanup)
                } else {
                    Err(StateError::InvalidObserverHandoverJournal)
                }
            }
            HandoverPhase::HandleSwapped | HandoverPhase::OldCleaning => {
                if standby && current.revision == next_revision {
                    Ok(HandoverRestartAction::FinishOldObserverCleanup)
                } else {
                    Err(StateError::InvalidObserverHandoverJournal)
                }
            }
            HandoverPhase::Complete => {
                if standby && current.revision == next_revision {
                    Ok(HandoverRestartAction::RemoveJournal)
                } else {
                    Err(StateError::InvalidObserverHandoverJournal)
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentObserverHandleProof {
    pub runtime_id: RuntimeId,
    pub runtime_generation: String,
    pub pid: u32,
    pub birth: String,
    pub revision: Revision,
}

impl CurrentObserverHandleProof {
    fn validate(&self) -> Result<(), StateError> {
        if self.runtime_generation.is_empty()
            || self.runtime_generation.len() > 256
            || self.runtime_generation.contains(['\0', '\n', '\r'])
            || self.pid == 0
            || self.birth.is_empty()
            || self.birth.len() > 256
            || self.birth.contains(['\0', '\n', '\r'])
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoverRestartAction {
    RestoreOldObserver,
    FinishOldObserverCleanup,
    RemoveJournal,
}

/// The two exact journal paths are the only files this module recognizes.
#[must_use]
pub fn observer_handover_journal_path(root: &Path) -> PathBuf {
    root.join(OBSERVER_HANDOVER_JOURNAL_FILE)
}

#[must_use]
pub fn observer_handover_journal_temp_path(root: &Path) -> PathBuf {
    root.join(OBSERVER_HANDOVER_JOURNAL_TEMP_FILE)
}

#[must_use]
pub fn observer_handover_activation_ack_path(root: &Path) -> PathBuf {
    root.join(OBSERVER_HANDOVER_ACTIVATION_ACK_FILE)
}

#[must_use]
pub fn observer_handover_activation_ack_temp_path(root: &Path) -> PathBuf {
    root.join(OBSERVER_HANDOVER_ACTIVATION_ACK_TEMP_FILE)
}

/// Creates or revalidates the one exact durable standby-activation proof.
/// The current journal must already prove the same post-CAS handover.
pub fn write_observer_handover_activation_ack(
    root: &Path,
    ack: &ObserverHandoverActivationAck,
) -> Result<(), StateError> {
    ack.validate()?;
    let journal =
        read_observer_handover_journal(root)?.ok_or(StateError::InvalidObserverHandoverJournal)?;
    if !ack.matches_journal(&journal)? {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let path = observer_handover_activation_ack_path(root);
    if let Some(existing) = read_observer_handover_activation_ack(root)? {
        return (existing == *ack)
            .then_some(())
            .ok_or(StateError::InvalidObserverHandoverJournal);
    }
    let body = serde_json::to_vec(ack).map_err(|_| StateError::InvalidObserverHandoverJournal)?;
    if body.len() > MAX_OBSERVER_HANDOVER_ACTIVATION_ACK_BYTES {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let temporary = observer_handover_activation_ack_temp_path(root);
    let mut file = open_private_create_new_file(&temporary)?;
    file.write_all(&body)
        .map_err(|error| StateError::io(&temporary, error))?;
    file.sync_all()
        .map_err(|error| StateError::io(&temporary, error))?;
    fs::rename(&temporary, &path).map_err(|error| StateError::io(&path, error))?;
    sync_directory(root)?;
    let persisted = read_observer_handover_activation_ack(root)?
        .ok_or(StateError::InvalidObserverHandoverJournal)?;
    (persisted == *ack)
        .then_some(())
        .ok_or(StateError::InvalidObserverHandoverJournal)
}

pub fn read_observer_handover_activation_ack(
    root: &Path,
) -> Result<Option<ObserverHandoverActivationAck>, StateError> {
    if !validate_handover_journal_root(root)? {
        return Ok(None);
    }
    let final_path = observer_handover_activation_ack_path(root);
    let temp_path = observer_handover_activation_ack_temp_path(root);
    let final_ack = read_observer_handover_activation_ack_candidate(&final_path)?;
    let temp_ack = read_observer_handover_activation_ack_candidate(&temp_path)?;
    match (final_ack, temp_ack) {
        (None, None) => Ok(None),
        (Some(ack), None) | (None, Some(ack)) => Ok(Some(ack)),
        (Some(final_ack), Some(temp_ack)) if final_ack == temp_ack => Ok(Some(final_ack)),
        (Some(_), Some(_)) => Err(StateError::InvalidObserverHandoverJournal),
    }
}

fn read_observer_handover_activation_ack_candidate(
    path: &Path,
) -> Result<Option<ObserverHandoverActivationAck>, StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StateError::io(path, error)),
    };
    if !metadata.is_file() || !is_private_owner_file(&metadata) {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let opened = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !opened.is_file()
        || !is_private_owner_file(&opened)
        || file_identity(&opened) != file_identity(&metadata)
    {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let mut body = Vec::new();
    file.take((MAX_OBSERVER_HANDOVER_ACTIVATION_ACK_BYTES as u64).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|error| StateError::io(path, error))?;
    if body.len() > MAX_OBSERVER_HANDOVER_ACTIVATION_ACK_BYTES {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let ack: ObserverHandoverActivationAck =
        serde_json::from_slice(&body).map_err(|_| StateError::InvalidObserverHandoverJournal)?;
    ack.validate()?;
    Ok(Some(ack))
}

/// Atomically replaces the exact private journal through its one recognized
/// temporary path and syncs the containing state directory.
pub fn write_observer_handover_journal(
    lease: &TransitionLease,
    journal: &ObserverHandoverJournal,
) -> Result<(), StateError> {
    lease.require_root(lease.root())?;
    journal.validate()?;
    let root = lease.root();
    let temp = observer_handover_journal_temp_path(root);
    let final_path = observer_handover_journal_path(root);
    let existing_final = read_observer_handover_candidate(&final_path)?;
    if let Some(existing_final) = existing_final.as_ref()
        && (!same_handover_identity(existing_final, journal)
            || (existing_final.phase != journal.phase
                && !existing_final.phase.permits(journal.phase)))
    {
        // Journal state is a monotonic proof, not a general-purpose mutable
        // record.  A backward phase or changed identity must never replace a
        // durable candidate under an otherwise valid lease.
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    for path in [&temp, &final_path] {
        if let Some(metadata) = exact_artifact_metadata(path)?
            && (!metadata.is_file() || !is_private_owner_file(&metadata))
        {
            return Err(StateError::InvalidObserverHandoverJournal);
        }
    }
    if exact_artifact_metadata(&temp)?.is_some() {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let body =
        serde_json::to_vec(journal).map_err(|_| StateError::InvalidObserverHandoverJournal)?;
    if body.len() > 4096 {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let mut file = open_private_create_new_file(&temp).map_err(|error| match error {
        StateError::InvalidObserverHandoverJournal => StateError::InvalidObserverHandoverJournal,
        other => other,
    })?;
    file.write_all(&body)
        .map_err(|error| StateError::io(&temp, error))?;
    file.sync_all()
        .map_err(|error| StateError::io(&temp, error))?;
    lease.require_root(root)?;
    fs::rename(&temp, &final_path).map_err(|error| StateError::io(&final_path, error))?;
    sync_directory(root)
}

pub fn read_observer_handover_journal(
    root: &Path,
) -> Result<Option<ObserverHandoverJournal>, StateError> {
    if !validate_handover_journal_root(root)? {
        return Ok(None);
    }
    let final_path = observer_handover_journal_path(root);
    let temp_path = observer_handover_journal_temp_path(root);
    let final_journal = read_observer_handover_candidate(&final_path)?;
    let temp_journal = read_observer_handover_candidate(&temp_path)?;
    match (final_journal, temp_journal) {
        (None, None) => Ok(None),
        (Some(final_journal), None) => Ok(Some(final_journal)),
        (None, Some(temp_journal)) => Ok(Some(temp_journal)),
        (Some(final_journal), Some(temp_journal)) => {
            if !same_handover_identity(&final_journal, &temp_journal)
                || !final_journal.phase.permits(temp_journal.phase)
            {
                return Err(StateError::InvalidObserverHandoverJournal);
            }
            Ok(Some(temp_journal))
        }
    }
}

fn validate_handover_journal_root(root: &Path) -> Result<bool, StateError> {
    let Some(metadata) = exact_artifact_metadata(root)? else {
        return Ok(false);
    };
    if !metadata.is_dir() || !is_private_owner_directory(&metadata) {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    Ok(true)
}

/// Promotes a valid, exact temporary journal candidate after a restart.  The
/// held lease is required for this mutation; no other path is discovered or
/// adopted.  A final+temporary pair is recoverable only when the temporary
/// candidate is the exact next phase of the final candidate.
pub fn recover_observer_handover_journal(
    lease: &TransitionLease,
) -> Result<Option<ObserverHandoverJournal>, StateError> {
    lease.require_root(lease.root())?;
    let root = lease.root();
    let final_path = observer_handover_journal_path(root);
    let temp_path = observer_handover_journal_temp_path(root);
    let final_journal = read_observer_handover_candidate(&final_path)?;
    let temp_journal = read_observer_handover_candidate(&temp_path)?;
    let candidate = match (&final_journal, &temp_journal) {
        (None, None) => return Ok(None),
        (Some(final_journal), None) => return Ok(Some(final_journal.clone())),
        (None, Some(temp_journal)) => temp_journal.clone(),
        (Some(final_journal), Some(temp_journal)) => {
            if !same_handover_identity(final_journal, temp_journal)
                || !final_journal.phase.permits(temp_journal.phase)
            {
                return Err(StateError::InvalidObserverHandoverJournal);
            }
            temp_journal.clone()
        }
    };
    promote_observer_handover_journal(lease, &final_path, &temp_path, &candidate)
}

fn promote_observer_handover_journal(
    lease: &TransitionLease,
    final_path: &Path,
    temp_path: &Path,
    candidate: &ObserverHandoverJournal,
) -> Result<Option<ObserverHandoverJournal>, StateError> {
    lease.require_root(lease.root())?;
    candidate.validate()?;
    let _temp_metadata =
        exact_artifact_metadata(temp_path)?.ok_or(StateError::InvalidObserverHandoverJournal)?;
    let final_metadata = exact_artifact_metadata(final_path)?;
    if let Some(metadata) = final_metadata
        && (!metadata.is_file() || !is_private_owner_file(&metadata))
    {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    match fs::rename(temp_path, final_path) {
        Ok(()) => sync_directory(lease.root())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent promoter may have completed the same exact step;
            // accept it only after re-reading both recognized paths.
            let recovered = read_observer_handover_journal(lease.root())?;
            if recovered.as_ref() != Some(candidate) {
                return Err(StateError::InvalidObserverHandoverJournal);
            }
            return Ok(recovered);
        }
        Err(error) => return Err(StateError::io(final_path, error)),
    }
    let recovered = read_observer_handover_journal(lease.root())?;
    if recovered.as_ref() != Some(candidate) {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    Ok(recovered)
}

fn same_handover_identity(
    final_journal: &ObserverHandoverJournal,
    temp_journal: &ObserverHandoverJournal,
) -> bool {
    final_journal.version == temp_journal.version
        && final_journal.runtime_id == temp_journal.runtime_id
        && final_journal.runtime_generation == temp_journal.runtime_generation
        && final_journal.old_observer == temp_journal.old_observer
        && final_journal.standby_observer == temp_journal.standby_observer
        && final_journal.expected_handle_revision == temp_journal.expected_handle_revision
}

fn read_observer_handover_candidate(
    path: &Path,
) -> Result<Option<ObserverHandoverJournal>, StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StateError::io(path, error)),
    };
    if !metadata.is_file() || !is_private_owner_file(&metadata) {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let opened = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !opened.is_file()
        || !is_private_owner_file(&opened)
        || file_identity(&opened) != file_identity(&metadata)
    {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let mut body = Vec::new();
    file.take(4097)
        .read_to_end(&mut body)
        .map_err(|error| StateError::io(path, error))?;
    if body.len() > 4096 {
        return Err(StateError::InvalidObserverHandoverJournal);
    }
    let journal: ObserverHandoverJournal =
        serde_json::from_slice(&body).map_err(|_| StateError::InvalidObserverHandoverJournal)?;
    journal.validate()?;
    Ok(Some(journal))
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        sync::atomic::{AtomicU64, Ordering},
    };

    use rusqlite::{Connection, types::Value};
    use uuid::Uuid;

    use super::*;
    use crate::domain::{HostId, IdGenerator, LocationId, Revision};
    use crate::onboarding::{ShellCommandDecision, classify_shell_command};
    use crate::repository::RepositoryRegistration;
    use crate::runtime::RuntimePaths;
    use crate::state::OpenCodeLifecycleObservation;
    use crate::state::utils::{set_private_directory_permissions, set_private_file_permissions};

    #[derive(Default)]
    struct SequenceIds(AtomicU64);

    impl IdGenerator for SequenceIds {
        fn uuid(&self) -> Uuid {
            Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed) + 1))
        }
    }

    fn private_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("temporary root")
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the exact schema-12 fixture remains one auditable pre-D16 inventory"
    )]
    fn schema12_root() -> (tempfile::TempDir, LocationId, LocationId) {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).expect("root permissions");
        let path = temporary.path().join("host.sqlite");
        let connection = Connection::open(&path).expect("host database");
        connection
            .execute_batch(HOST_SCHEMA_SQL)
            .expect("schema 12 fixture");
        let host_id = HostId::new();
        connection
            .execute(
                "INSERT INTO host_identity (
                    singleton, host_id, registry_generation, schema_version
                 ) VALUES (1, ?1, 'generation-a', 12)",
                [host_id.to_string()],
            )
            .expect("host identity");
        let first = LocationId::from(Uuid::from_u128(2));
        let second = LocationId::from(Uuid::from_u128(3));
        connection
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display, revision
                 ) VALUES (?1, '/first', 'first', ?2, 'origin-a', 7),
                        (?3, '/second', 'second', ?2, 'origin-a', 8)",
                params![
                    first.to_string(),
                    format!("git-remote-v1:{}", "a".repeat(64)),
                    second.to_string(),
                ],
            )
            .expect("locations");
        let integration_id = Uuid::from_u128(20);
        let workstream_id = Uuid::from_u128(21);
        let runtime_id = Uuid::from_u128(22);
        let binding_id = Uuid::from_u128(23);
        let operation_id = Uuid::from_u128(24);
        let opencode_workstream_id = Uuid::from_u128(25);
        let opencode_runtime_id = Uuid::from_u128(26);
        let opencode_binding_id = Uuid::from_u128(27);
        connection
            .execute(
                "INSERT INTO codex_integrations (
                    integration_id, profile_name, canonical_profile_path,
                    owner_id, profile_schema_version, hook_executable_path,
                    generated_content_hash, lifecycle, revision
                 ) VALUES (?1, 'fixture-profile', '/fixture/profile', 'owner-a',
                    3, '/fixture/hook', 'hash-a', 'active', 4)",
                [integration_id.to_string()],
            )
            .expect("integration");
        connection
            .execute(
                "INSERT INTO project_browser_settings (singleton, root_path, revision)
                 VALUES (1, '/fixture/browser', 5)",
                [],
            )
            .expect("browser settings");
        connection
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin,
                    source_workstream_id, lifecycle, archived_at_millis,
                    last_activity_sequence, last_activity_at_millis, revision
                 ) VALUES (?1, ?2, 'codex', 'external', NULL, 'open',
                    NULL, 17, 123456789, 6)",
                params![workstream_id.to_string(), first.to_string()],
            )
            .expect("workstream");
        connection
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin,
                    source_workstream_id, lifecycle, archived_at_millis,
                    last_activity_sequence, last_activity_at_millis, revision
                 ) VALUES (?1, ?2, 'opencode', 'independent', ?3, 'parked',
                    123456700, 23, 123456799, 14)",
                params![
                    opencode_workstream_id.to_string(),
                    second.to_string(),
                    workstream_id.to_string(),
                ],
            )
            .expect("OpenCode workstream");
        connection
            .execute(
                "INSERT INTO independent_creation_requests (
                    request_key, source_workstream_id, source_revision, workstream_id
                 ) VALUES ('fixture-request', ?1, 6, ?1)",
                [workstream_id.to_string()],
            )
            .expect("independent request");
        connection
            .execute(
                "INSERT INTO runtimes (
                    runtime_id, workstream_id, provider, tmux_generation,
                    tmux_session, cwd, provider_pid, process_birth, lifecycle, revision
                 ) VALUES (?1, ?2, 'codex', 'tmux-generation-a', 'tmux-session-a',
                    '/fixture/cwd', 4321, 'birth-a', 'idle', 7)",
                params![runtime_id.to_string(), workstream_id.to_string()],
            )
            .expect("runtime");
        connection
            .execute(
                "INSERT INTO runtimes (
                    runtime_id, workstream_id, provider, tmux_generation,
                    tmux_session, cwd, provider_pid, process_birth, lifecycle, revision
                 ) VALUES (?1, ?2, 'opencode', 'runtime-generation-opencode',
                    'tmux-session-opencode', '/fixture/opencode-cwd', 5321,
                    'birth-opencode', 'working', 15)",
                params![
                    opencode_runtime_id.to_string(),
                    opencode_workstream_id.to_string()
                ],
            )
            .expect("OpenCode runtime");
        connection
            .execute(
                "INSERT INTO opencode_runtime_handles (
                    runtime_id, runtime_generation, endpoint_host, endpoint_port,
                    version, native_session_id, observer_pid, observer_birth,
                    observer_status, revision
                 ) VALUES (?1, 'runtime-generation-opencode', '127.0.0.1', 43123,
                    '1.2.3', 'native-session-opencode', 5322,
                    'observer-birth-opencode', 'ready', 16)",
                [opencode_runtime_id.to_string()],
            )
            .expect("OpenCode handle");
        connection
            .execute(
                "INSERT INTO provider_bindings (
                    binding_id, runtime_id, provider, native_session_id,
                    start_source, last_settled_turn_id, observed_thread_name,
                    name_state, name_observed_at, predecessor_native_session_id,
                    predecessor_effective_name, runtime_generation, revision
                 ) VALUES (?1, ?2, 'codex', 'native-session-a', 'created',
                    'turn-a', 'thread-a', 'known', 123456790, 'native-session-previous',
                    'previous-name', 'runtime-generation-a', 9)",
                params![binding_id.to_string(), runtime_id.to_string()],
            )
            .expect("provider binding");
        connection
            .execute(
                "INSERT INTO provider_bindings (
                    binding_id, runtime_id, provider, native_session_id,
                    start_source, last_settled_turn_id, observed_thread_name,
                    name_state, name_observed_at, predecessor_native_session_id,
                    predecessor_effective_name, runtime_generation, revision
                 ) VALUES (?1, ?2, 'opencode', 'native-session-opencode', 'recovered',
                    'turn-opencode', 'thread-opencode', 'unavailable', 223456790,
                    NULL, NULL, 'runtime-generation-opencode', 17)",
                params![
                    opencode_binding_id.to_string(),
                    opencode_runtime_id.to_string()
                ],
            )
            .expect("OpenCode provider binding");
        connection
            .execute(
                "INSERT INTO attention_states (
                    workstream_id, result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_native_session_provider, latest_turn_id, revision
                 ) VALUES (?1, 10, 11, 'native-session-a', 'codex', 'turn-a', 12)",
                [workstream_id.to_string()],
            )
            .expect("attention state");
        connection
            .execute(
                "INSERT INTO attention_states (
                    workstream_id, result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_native_session_provider, latest_turn_id, revision
                 ) VALUES (?1, NULL, 18, 'native-session-opencode',
                    'opencode', 'turn-opencode', 19)",
                [opencode_workstream_id.to_string()],
            )
            .expect("OpenCode attention state");
        connection
            .execute(
                "INSERT INTO compound_operations (
                    operation_id, request_key, kind, phase,
                    expected_revisions_json, effect_watermark, outcome_json, revision
                 ) VALUES (?1, 'fixture-operation', 'create', 'settled',
                    '{\"location\":6}', 'watermark-a', '{\"ok\":true}', 13)",
                [operation_id.to_string()],
            )
            .expect("compound operation");
        connection
            .execute_batch("PRAGMA user_version = 12;")
            .expect("schema version");
        set_private_file_permissions(&path).expect("host database permissions");
        (temporary, first, second)
    }

    fn transition_lease(path: &Path) -> TransitionLease {
        set_private_directory_permissions(path).expect("root permissions");
        let lock_path = path.join(TRANSITION_LOCK_FILE);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .expect("transition lock");
        set_private_file_permissions_handle(&file, &lock_path).expect("lock permissions");
        drop(file);
        acquire_transition_lease(path).expect("transition lease")
    }

    fn onboarding_prepare_request(
        state_path: &Path,
        candidate_runtime_id: RuntimeId,
    ) -> OnboardingPrepareRequest {
        let worktree_root = state_path.join("worktree");
        fs::create_dir(&worktree_root).expect("worktree root");
        let shell_cwd = worktree_root.join("nested");
        fs::create_dir(&shell_cwd).expect("shell cwd");
        let arguments = [OsString::from("--model"), OsString::from("gpt-5.6")];
        let ShellCommandDecision::ManagedFresh(launch) =
            classify_shell_command(ProviderKind::Codex, &arguments).expect("managed launch")
        else {
            panic!("fixture must be promotable");
        };
        OnboardingPrepareRequest {
            request_key: "d17-onboarding-request".to_owned(),
            presentation_id: Uuid::from_u128(700),
            presentation_revision: Revision::INITIAL,
            slot_generation: Uuid::from_u128(701),
            candidate_runtime_id,
            runtime_paths: RuntimePaths::for_runtime(state_path, candidate_runtime_id),
            provider: ProviderKind::Codex,
            repository: RepositoryRegistration {
                project_root: worktree_root,
                display_name: "worktree".to_owned(),
                remote_identity_fingerprint: Some(format!("git-remote-v1:{}", "a".repeat(64))),
                remote_identity_display: Some("github.com/example/worktree".to_owned()),
            },
            shell_cwd,
            shell_pid: 710,
            shell_birth: "birth-710".to_owned(),
            shell_process_group: 710,
            shell_session: 710,
            argv_digest: launch.argv_digest().to_owned(),
            boot_provenance: format!("d17-boot-v1:sha256:{}", "b".repeat(64)),
            now_monotonic_millis: 10,
            expiry_monotonic_millis: 1_010,
        }
    }

    fn sample_journal() -> ObserverHandoverJournal {
        ObserverHandoverJournal {
            version: 1,
            runtime_id: RuntimeId::from(Uuid::from_u128(11)).to_string(),
            runtime_generation: "generation-a".to_owned(),
            old_observer: ObserverProcessIdentity {
                pid: 10,
                birth: "birth-old".to_owned(),
                executable: "opencode-observer".to_owned(),
            },
            standby_observer: ObserverProcessIdentity {
                pid: 11,
                birth: "birth-new".to_owned(),
                executable: "opencode-observer".to_owned(),
            },
            expected_handle_revision: Revision::INITIAL,
            phase: HandoverPhase::Prepared,
        }
    }

    fn authoritative_snapshot(connection: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
        let browser_settings_present = table_exists(connection, "project_browser_settings")
            .expect("browser settings table inspection");
        let queries = [
            (
                "host_identity",
                "SELECT singleton, host_id, registry_generation FROM host_identity",
            ),
            (
                "codex_integrations",
                "SELECT integration_id, profile_name, canonical_profile_path, owner_id,
                        profile_schema_version, hook_executable_path, generated_content_hash,
                        lifecycle, revision FROM codex_integrations ORDER BY integration_id",
            ),
            (
                "project_locations",
                "SELECT location_id, repository_path, repository_display_name,
                        remote_identity_fingerprint, remote_identity_display, revision
                 FROM project_locations ORDER BY location_id",
            ),
            (
                "workstreams",
                "SELECT workstream_id, location_id, provider, origin, source_workstream_id,
                        lifecycle, archived_at_millis, last_activity_sequence,
                        last_activity_at_millis, revision FROM workstreams ORDER BY workstream_id",
            ),
            (
                "independent_creation_requests",
                "SELECT request_key, source_workstream_id, source_revision, workstream_id
                 FROM independent_creation_requests ORDER BY request_key",
            ),
            (
                "project_browser_settings",
                "SELECT singleton, root_path, revision FROM project_browser_settings",
            ),
            (
                "runtimes",
                "SELECT runtime_id, workstream_id, provider, tmux_generation, tmux_session,
                        cwd, provider_pid, process_birth, lifecycle, revision
                 FROM runtimes ORDER BY runtime_id",
            ),
            (
                "opencode_runtime_handles",
                "SELECT runtime_id, runtime_generation, endpoint_host, endpoint_port, version,
                        native_session_id, observer_pid, observer_birth, observer_status, revision
                 FROM opencode_runtime_handles ORDER BY runtime_id",
            ),
            (
                "provider_bindings",
                "SELECT binding_id, runtime_id, provider, native_session_id, start_source,
                        last_settled_turn_id, observed_thread_name, name_state, name_observed_at,
                        predecessor_native_session_id, predecessor_effective_name,
                        runtime_generation, revision FROM provider_bindings ORDER BY binding_id",
            ),
            (
                "attention_states",
                "SELECT workstream_id, result_unseen_since_revision,
                        recovery_unseen_since_revision, latest_native_session_id,
                        latest_native_session_provider, latest_turn_id, revision
                 FROM attention_states ORDER BY workstream_id",
            ),
            (
                "compound_operations",
                "SELECT operation_id, request_key, kind, phase, expected_revisions_json,
                        effect_watermark, outcome_json, revision
                 FROM compound_operations ORDER BY operation_id",
            ),
        ];
        queries
            .into_iter()
            .filter(|(name, _)| *name != "project_browser_settings" || browser_settings_present)
            .map(|(name, query)| {
                let mut statement = connection.prepare(query).expect("snapshot query");
                let columns = statement.column_count();
                let rows = statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|index| row.get::<_, Value>(index))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .expect("snapshot rows")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("snapshot values");
                (name.to_owned(), rows)
            })
            .collect()
    }

    fn project_snapshot(connection: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
        [
            (
                "projects",
                "SELECT project_id, label_location_id, display_name,
                        repository_fingerprint, revision FROM projects ORDER BY project_id",
            ),
            (
                "project_locations",
                "SELECT location_id, project_id, repository_display_name,
                        remote_identity_fingerprint, remote_identity_display, revision
                 FROM project_locations ORDER BY location_id",
            ),
        ]
        .into_iter()
        .map(|(name, query)| {
            let mut statement = connection.prepare(query).unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| row.get::<_, Value>(index))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (name.to_owned(), rows)
        })
        .collect()
    }

    #[test]
    fn fresh_create_writes_schema13_without_host_registry_activation() {
        let temporary = private_root();
        let root = temporary.path().join("state");
        let ids = SequenceIds::default();
        let state = fresh_create(&root, &ids).expect("fresh schema 13");
        assert_eq!(state.mode(), D16OpenMode::FreshCreate);
        assert_eq!(state.schema_version().unwrap(), D16_HOST_SCHEMA_VERSION);
        assert!(root.join("host.sqlite").is_file());
        assert!(!root.join(TRANSITION_LOCK_FILE).exists());
        assert_eq!(state.projects().unwrap(), Vec::new());
    }

    #[test]
    fn fresh_classifier_rejects_unknown_artifact_without_adoption() {
        let temporary = private_root();
        let root = temporary.path().join("state");
        std::fs::create_dir(&root).unwrap();
        set_private_directory_permissions(&root).unwrap();
        std::fs::write(root.join("orphan.sqlite"), b"not adopted").unwrap();
        assert!(matches!(
            classify_fresh_root(&root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::UnknownArtifact
            ))
        ));
        assert!(!root.join("host.sqlite").exists());
    }

    #[cfg(unix)]
    #[test]
    fn fresh_create_rejects_a_symlink_root_without_mutating_its_target() {
        let temporary = private_root();
        let target = private_root();
        let root = temporary.path().join("state");
        std::os::unix::fs::symlink(target.path(), &root).unwrap();
        assert!(matches!(
            fresh_create(&root, &SequenceIds::default()),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NotDirectory
            ))
        ));
        assert!(!target.path().join("host.sqlite").exists());
    }

    #[test]
    fn fresh_create_rechecks_and_adopts_only_an_existing_unlocked_lease() {
        let temporary = private_root();
        let root = temporary.path().join("state");
        fs::create_dir(&root).unwrap();
        set_private_directory_permissions(&root).unwrap();
        let lock_path = root.join(TRANSITION_LOCK_FILE);
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        set_private_file_permissions_handle(&lock, &lock_path).unwrap();
        drop(lock);
        let state = fresh_create(&root, &SequenceIds::default()).unwrap();
        assert_eq!(state.mode(), D16OpenMode::FreshCreate);
        assert!(!lock_path.exists());
    }

    #[test]
    fn migration_groups_exact_origins_and_keeps_missing_separate() {
        let (temporary, first, second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let state = open_confirmed_cutover(&root, &ids, &lease).expect("cutover migration");
        assert_eq!(state.schema_version().unwrap(), D16_HOST_SCHEMA_VERSION);
        let projects = state.projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].label_location_id, first);
        let second_project: ProjectId = state
            .connection
            .query_row(
                "SELECT project_id FROM project_locations WHERE location_id = ?1",
                [second.to_string()],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(second_project, projects[0].project_id);
        // The migration never touches the retired client database path.
        assert!(!temporary.path().join(LEGACY_CLIENT_DATABASE_FILE).exists());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn opencode_settled_messages_are_durable_and_revision_idempotent() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let state = open_confirmed_cutover(&root, &SequenceIds::default(), &lease).unwrap();
        let mut registry = state.into_host_registry_under_lease(&lease).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(26));
        let workstream_id = WorkstreamId::from(Uuid::from_u128(25));
        let session =
            ProviderSessionId::new(ProviderKind::OpenCode, "native-session-opencode").unwrap();
        let observation = |runtime_revision, message_id: &str| OpenCodeLifecycleObservation {
            generation: "runtime-generation-opencode".to_owned(),
            cwd: PathBuf::from("/fixture/opencode-cwd"),
            runtime_revision,
            session: session.clone(),
            observer_pid: 5322,
            observer_birth: "observer-birth-opencode".to_owned(),
            hint: LifecycleHint::Settled {
                message_id: Some(message_id.to_owned()),
            },
        };
        let first_revision = Revision::try_from(15).unwrap();
        let revision_snapshot = |registry: &HostRegistry| {
            let runtime_revision: i64 = registry
                .connection
                .query_row(
                    "SELECT revision FROM runtimes WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let binding_revision: i64 = registry
                .connection
                .query_row(
                    "SELECT revision FROM provider_bindings WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let workstream_revision: i64 = registry
                .connection
                .query_row(
                    "SELECT revision FROM workstreams WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let attention_revision: i64 = registry
                .connection
                .query_row(
                    "SELECT revision FROM attention_states WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let latest_turn: String = registry
                .connection
                .query_row(
                    "SELECT latest_turn_id FROM attention_states WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            (
                runtime_revision,
                binding_revision,
                workstream_revision,
                attention_revision,
                latest_turn,
            )
        };
        let migrated_latest: String = registry
            .connection
            .query_row(
                "SELECT last_settled_turn_id FROM provider_bindings
                 WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let before_migrated_duplicate = revision_snapshot(&registry);
        assert_eq!(
            registry
                .apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(first_revision, &migrated_latest),
                )
                .unwrap(),
            first_revision
        );
        assert_eq!(revision_snapshot(&registry), before_migrated_duplicate);
        assert_eq!(
            registry
                .apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(first_revision, "message-a"),
                )
                .unwrap(),
            first_revision.next()
        );
        let after_first = revision_snapshot(&registry);
        assert_eq!(
            registry
                .apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(first_revision.next(), "message-a"),
                )
                .unwrap(),
            first_revision.next()
        );
        assert_eq!(revision_snapshot(&registry), after_first);

        let second_revision = first_revision.next();
        assert_eq!(
            registry
                .apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(second_revision, "message-b"),
                )
                .unwrap(),
            second_revision.next()
        );
        let after_second = revision_snapshot(&registry);
        assert_eq!(after_second.4, "message-b");
        assert_eq!(
            registry
                .apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(second_revision.next(), "message-a"),
                )
                .unwrap(),
            second_revision.next()
        );
        assert_eq!(revision_snapshot(&registry), after_second);

        // A generation/session may retain more than the old bounded window.
        // The first identity must remain a no-op even after 257 later
        // settled messages have been accepted.
        let mut revision = second_revision.next();
        for index in 0..257 {
            let message_id = format!("message-{index}");
            assert_eq!(
                registry
                    .apply_opencode_lifecycle_observation(
                        runtime_id,
                        &observation(revision, &message_id),
                    )
                    .unwrap(),
                revision.next()
            );
            revision = revision.next();
        }
        let after_long_history = revision_snapshot(&registry);
        assert_eq!(
            registry
                .apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(revision, "message-a"),
                )
                .unwrap(),
            revision
        );
        assert_eq!(revision_snapshot(&registry), after_long_history);
        let retained_count: i64 = registry
            .connection
            .query_row(
                "SELECT COUNT(*) FROM opencode_settled_messages
                 WHERE runtime_id = ?1 AND runtime_generation = ?2
                   AND native_session_id = ?3",
                params![
                    runtime_id.to_string(),
                    "runtime-generation-opencode",
                    session.native_id(),
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained_count, 259);
        let lifecycle: String = registry
            .connection
            .query_row(
                "SELECT lifecycle FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lifecycle, "attention");
    }

    #[test]
    fn schema12_migration_preserves_every_authoritative_row_and_field() {
        let (temporary, _first, _second) = schema12_root();
        let connection = Connection::open(temporary.path().join("host.sqlite")).unwrap();
        let before = authoritative_snapshot(&connection);
        drop(connection);
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let state = open_confirmed_cutover(&root, &ids, &lease).unwrap();
        let after = authoritative_snapshot(&state.connection);
        assert_eq!(before, after);
        assert_eq!(state.schema_version().unwrap(), D16_HOST_SCHEMA_VERSION);
        assert_eq!(state.projects().unwrap().len(), 1);
    }

    #[test]
    fn schema13_to14_removes_only_browser_settings_and_records_pending_lock_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        let fresh = fresh_create(&state_path, &SequenceIds::default()).unwrap();
        fresh
            .connection
            .execute(
                "INSERT INTO project_browser_settings (singleton, root_path, revision)
                 VALUES (1, '/fixture/browser-root', 7)",
                [],
            )
            .unwrap();
        let before = authoritative_snapshot(&fresh.connection)
            .into_iter()
            .filter(|(name, _)| name != "project_browser_settings")
            .collect::<Vec<_>>();
        drop(fresh);

        let lease = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &lease).unwrap();
        state.migrate_schema13_to14(&lease).unwrap();
        assert_eq!(state.schema_version().unwrap(), D17_HOST_SCHEMA_VERSION);
        validate_schema14(&state.connection).unwrap();
        assert_eq!(
            authoritative_snapshot(&state.connection),
            before,
            "schema 14 must preserve every non-browser authoritative row"
        );
        assert!(!table_exists(&state.connection, "project_browser_settings").unwrap());
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT provisional_lease_generation, provisional_lock_phase,
                            provisional_lock_device, provisional_lock_inode
                     FROM host_operational_metadata WHERE singleton = 1",
                    [],
                    |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?
                    )),
                )
                .unwrap(),
            (1, "pending".to_owned(), None, None)
        );

        state.migrate_schema13_to14(&lease).unwrap();
        drop(state);
        drop(lease);
        fs::remove_file(state_path.join(TRANSITION_LOCK_FILE)).unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::UnsupportedFutureHostSchema(
                D17_HOST_SCHEMA_VERSION
            ))
        ));
        let mut d17 = open_d17_current_only(&root).unwrap();
        assert_eq!(d17.mode(), D16OpenMode::D17Current);
        assert_eq!(d17.schema_version().unwrap(), D17_HOST_SCHEMA_VERSION);
        let provisional = d17.acquire_d17_provisional_lease().unwrap();
        assert_eq!(provisional.lease_generation(), 1);
        provisional.revalidate_for_mutation(&state_path).unwrap();
        drop(provisional);
        drop(d17);

        let mut reopened = open_d17_current_only(&root).unwrap();
        let reacquired = reopened.acquire_d17_provisional_lease().unwrap();
        assert_eq!(reacquired.lease_generation(), 1);
        drop(reacquired);
        drop(reopened);
        let d17 = open_d17_current_only(&root).unwrap();
        assert!(matches!(
            d17.into_host_registry(),
            Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::UnsupportedLegacySchema
            ))
        ));
    }

    #[test]
    fn schema13_to14_refuses_a_pre_schema_provisional_lock_without_mutation() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        let fresh = fresh_create(&state_path, &SequenceIds::default()).unwrap();
        let before = authoritative_snapshot(&fresh.connection);
        drop(fresh);
        let provisional = state_path.join("provisional.lock");
        let marker = b"foreign pre-schema evidence";
        fs::write(&provisional, marker).unwrap();
        set_private_file_permissions(&provisional).unwrap();
        let lease = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &lease).unwrap();
        assert!(matches!(
            state.migrate_schema13_to14(&lease),
            Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::ProvisionalLockPresent
            ))
        ));
        assert_eq!(
            schema_version(&state.connection).unwrap(),
            D16_HOST_SCHEMA_VERSION
        );
        assert_eq!(authoritative_snapshot(&state.connection), before);
        assert_eq!(fs::read(&provisional).unwrap(), marker);
    }

    #[test]
    fn d17_open_refuses_a_provisional_lock_without_a_schema14_database() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        fs::create_dir(&state_path).unwrap();
        set_private_directory_permissions(&state_path).unwrap();
        let lock = state_path.join(PROVISIONAL_LOCK_FILE);
        fs::write(&lock, b"unexpected").unwrap();
        set_private_file_permissions(&lock).unwrap();

        assert!(matches!(
            open_d17_current_only(&StateRoot::select(&state_path)),
            Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::ProvisionalLockPresent
            ))
        ));
    }

    #[test]
    fn schema14_validates_the_bounded_onboarding_capability_journal_shape() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());
        let transition = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        state
            .connection
            .execute(
                "INSERT INTO compound_operations (
                    operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision,
                    launch_token_id, launch_token_verifier,
                    launch_token_expiry_monotonic, launch_claims_digest
                 ) VALUES (?1, 'onboard-journal', 'onboard', 'prepared', '{}',
                    NULL, NULL, 1, NULL, NULL, NULL, NULL)",
                [Uuid::from_u128(901).to_string()],
            )
            .unwrap();
        validate_schema14(&state.connection).unwrap();

        state
            .connection
            .execute(
                "UPDATE compound_operations SET phase = 'capability_issued'
                 WHERE request_key = 'onboard-journal'",
                [],
            )
            .unwrap();
        assert!(matches!(
            validate_schema14(&state.connection),
            Err(StateError::MalformedHostSchema)
        ));

        let token_id = Uuid::from_u128(902).to_string();
        let verifier = format!("d17-launch-verifier-v1:sha256:{}", "a".repeat(64));
        let claims_digest = format!("d17-launch-claims-v1:sha256:{}", "b".repeat(64));
        state
            .connection
            .execute(
                "UPDATE compound_operations
                 SET launch_token_id = ?1,
                     launch_token_verifier = ?2,
                     launch_token_expiry_monotonic = 17,
                     launch_claims_digest = ?3
                 WHERE request_key = 'onboard-journal'",
                params![token_id, verifier, claims_digest],
            )
            .unwrap();
        validate_schema14(&state.connection).unwrap();

        state
            .connection
            .execute(
                "UPDATE compound_operations
                 SET launch_claims_digest = 'not-a-digest'
                 WHERE request_key = 'onboard-journal'",
                [],
            )
            .unwrap();
        assert!(matches!(
            validate_schema14(&state.connection),
            Err(StateError::MalformedHostSchema)
        ));
        state
            .connection
            .execute(
                "UPDATE compound_operations
                 SET launch_claims_digest = ?1, kind = 'start'
                 WHERE request_key = 'onboard-journal'",
                [format!("d17-launch-claims-v1:sha256:{}", "b".repeat(64))],
            )
            .unwrap();
        assert!(matches!(
            validate_schema14(&state.connection),
            Err(StateError::MalformedHostSchema)
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn schema14_onboarding_preparation_adopts_one_exact_candidate_and_never_reissues_its_token() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());
        let transition = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        let provisional = state
            .install_or_acquire_provisional_lease(&transition)
            .unwrap();
        let candidate_runtime_id = RuntimeId::from(Uuid::from_u128(702));
        let request = onboarding_prepare_request(&state_path, candidate_runtime_id);
        let ids = SequenceIds::default();
        state
            .connection
            .busy_timeout(Duration::from_millis(73))
            .unwrap();

        let issued = match state
            .prepare_d17_onboarding(&transition, &provisional, &request, &ids)
            .unwrap()
        {
            OnboardingPreparation::Issued(reservation) => reservation,
            OnboardingPreparation::Existing(_) => panic!("first preparation must issue"),
        };
        assert_eq!(issued.runtime().runtime_id, candidate_runtime_id);
        assert_eq!(issued.runtime().provider, ProviderKind::Codex);
        assert_eq!(issued.runtime().status, RuntimeStatus::Starting);
        assert_eq!(issued.runtime().cwd, request.repository.project_root);
        assert_eq!(
            issued.runtime().tmux_session,
            request.runtime_paths.session_name
        );
        let token = issued.capability().token().to_owned();
        assert!(!format!("{issued:?}").contains(&token));

        let persisted = state
            .connection
            .query_row(
                "SELECT kind, phase, launch_token_id, launch_token_verifier,
                        launch_token_expiry_monotonic, launch_claims_digest
                 FROM compound_operations WHERE operation_id = ?1",
                [issued.operation_id().to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(persisted.0, "onboard");
        assert_eq!(persisted.1, "capability_issued");
        assert_ne!(persisted.2, token);
        assert_ne!(persisted.3, token);
        assert_eq!(persisted.4, request.expiry_monotonic_millis);
        assert!(persisted.5.starts_with("d17-launch-claims-v1:sha256:"));
        assert_eq!(
            state
                .connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            73
        );
        assert_eq!(
            state
                .connection
                .query_row("SELECT COUNT(*) FROM project_locations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .connection
                .query_row("SELECT COUNT(*) FROM workstreams", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .connection
                .query_row("SELECT COUNT(*) FROM runtimes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        validate_schema14(&state.connection).unwrap();

        match state
            .prepare_d17_onboarding(&transition, &provisional, &request, &ids)
            .unwrap()
        {
            OnboardingPreparation::Existing(existing) => {
                assert_eq!(existing.operation_id, issued.operation_id());
                assert_eq!(existing.location_id, issued.location_id());
                assert_eq!(existing.workstream_id, issued.workstream_id());
                assert_eq!(existing.runtime_id, candidate_runtime_id);
            }
            OnboardingPreparation::Issued(_) => panic!("replay must not reissue a token"),
        }
        assert_eq!(
            state
                .connection
                .query_row("SELECT COUNT(*) FROM compound_operations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );

        let mut mismatched = request.clone();
        mismatched.provider = ProviderKind::OpenCode;
        assert!(matches!(
            state.prepare_d17_onboarding(&transition, &provisional, &mismatched, &ids),
            Err(StateError::OperationRequestMismatch)
        ));
        let mut invalid_path = request.clone();
        invalid_path.runtime_paths.session_name = "wsnav-replacement".to_owned();
        assert!(matches!(
            state.prepare_d17_onboarding(&transition, &provisional, &invalid_path, &ids),
            Err(StateError::InvalidOnboardingPreparation)
        ));
        assert_eq!(
            state
                .connection
                .query_row("SELECT COUNT(*) FROM compound_operations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );

        assert!(matches!(
            state.consume_d17_onboarding(
                &transition,
                &provisional,
                &request,
                &token,
                request.expiry_monotonic_millis,
            ),
            Err(StateError::OnboardingCapabilityExpired)
        ));
        let mut rejected_token = token.clone();
        let final_character = rejected_token.pop().unwrap();
        rejected_token.push(if final_character == '0' { '1' } else { '0' });
        assert!(matches!(
            state.consume_d17_onboarding(
                &transition,
                &provisional,
                &request,
                &rejected_token,
                request.now_monotonic_millis + 1,
            ),
            Err(StateError::OnboardingCapabilityRejected)
        ));
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT phase FROM compound_operations WHERE operation_id = ?1",
                    [issued.operation_id().to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "capability_issued"
        );

        let ownership = state
            .consume_d17_onboarding(
                &transition,
                &provisional,
                &request,
                &token,
                request.now_monotonic_millis + 1,
            )
            .unwrap();
        assert_eq!(ownership.operation_id, issued.operation_id());
        assert_eq!(ownership.location_id, issued.location_id());
        assert_eq!(ownership.workstream_id, issued.workstream_id());
        assert_eq!(ownership.runtime_id, candidate_runtime_id);
        assert_eq!(ownership.operation_revision.value(), 3);
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT phase, revision FROM compound_operations WHERE operation_id = ?1",
                    [issued.operation_id().to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("runtime_owned_launching".to_owned(), 3)
        );
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT lifecycle FROM runtimes WHERE runtime_id = ?1",
                    [candidate_runtime_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "starting",
            "ownership alone must not claim provider execution or attach authority"
        );
        assert!(matches!(
            state.consume_d17_onboarding(
                &transition,
                &provisional,
                &request,
                &token,
                request.now_monotonic_millis + 1,
            ),
            Err(StateError::OnboardingOperationUnavailable)
        ));
        assert_eq!(
            state
                .connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            73
        );
        validate_schema14(&state.connection).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn schema14_current_onboarding_uses_only_the_retained_provisional_lease() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());

        let transition = transition_lease(&state_path);
        let mut migrating = open_cutover_transition(&root, &transition).unwrap();
        migrating.migrate_schema13_to14(&transition).unwrap();
        drop(migrating);
        drop(transition);
        fs::remove_file(state_path.join(TRANSITION_LOCK_FILE)).unwrap();

        let mut state = open_d17_current_only(&root).unwrap();
        let provisional = state.acquire_d17_provisional_lease().unwrap();
        let candidate_runtime_id = RuntimeId::from(Uuid::from_u128(703));
        let request = onboarding_prepare_request(&state_path, candidate_runtime_id);
        let ids = SequenceIds::default();
        state
            .connection
            .busy_timeout(Duration::from_millis(81))
            .unwrap();

        let issued = match state
            .prepare_d17_onboarding_current(&provisional, &request, &ids)
            .unwrap()
        {
            OnboardingPreparation::Issued(reservation) => reservation,
            OnboardingPreparation::Existing(_) => panic!("first preparation must issue"),
        };
        let token = issued.capability().token().to_owned();
        assert_eq!(issued.runtime().runtime_id, candidate_runtime_id);
        assert_eq!(issued.runtime().status, RuntimeStatus::Starting);
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT phase FROM compound_operations WHERE operation_id = ?1",
                    [issued.operation_id().to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "capability_issued"
        );
        assert_eq!(
            state
                .connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            81
        );

        match state
            .prepare_d17_onboarding_current(&provisional, &request, &ids)
            .unwrap()
        {
            OnboardingPreparation::Existing(existing) => {
                assert_eq!(existing.operation_id, issued.operation_id());
                assert_eq!(existing.runtime_id, candidate_runtime_id);
            }
            OnboardingPreparation::Issued(_) => panic!("replay must not reissue a token"),
        }

        let ownership = state
            .consume_d17_onboarding_current(
                &provisional,
                &request,
                &token,
                request.now_monotonic_millis + 1,
            )
            .unwrap();
        assert_eq!(ownership.operation_id, issued.operation_id());
        assert_eq!(ownership.runtime_id, candidate_runtime_id);
        assert_eq!(ownership.operation_revision.value(), 3);
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT phase, revision FROM compound_operations WHERE operation_id = ?1",
                    [issued.operation_id().to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("runtime_owned_launching".to_owned(), 3)
        );
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT lifecycle FROM runtimes WHERE runtime_id = ?1",
                    [candidate_runtime_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "starting",
            "current ownership alone must not claim provider execution"
        );
        assert!(matches!(
            state.consume_d17_onboarding_current(
                &provisional,
                &request,
                &token,
                request.now_monotonic_millis + 1,
            ),
            Err(StateError::OnboardingOperationUnavailable)
        ));
        assert!(matches!(
            state.record_d17_provider_exec_started_current(&provisional, ownership),
            Err(StateError::Domain(
                crate::domain::DomainError::InvalidOnboardingTransition { .. }
            ))
        ));
        let preparation = state
            .record_d17_provider_preparation_current(&provisional, ownership)
            .unwrap();
        assert_eq!(preparation.operation_revision.value(), 4);
        assert!(matches!(
            state.record_d17_provider_preparation_current(&provisional, ownership),
            Err(StateError::ConcurrentWrite),
        ));
        let effect_started = state
            .record_d17_provider_external_effect_started_current(&provisional, preparation)
            .unwrap();
        assert_eq!(effect_started.operation_revision.value(), 5);
        let exec_started = state
            .record_d17_provider_exec_started_current(&provisional, effect_started)
            .unwrap();
        assert_eq!(exec_started.operation_revision.value(), 6);
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT phase, revision FROM compound_operations WHERE operation_id = ?1",
                    [issued.operation_id().to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("provider_exec_started".to_owned(), 6)
        );
        assert!(matches!(
            state.record_d17_provider_preparation_current(&provisional, exec_started),
            Err(StateError::Domain(
                crate::domain::DomainError::InvalidOnboardingTransition { .. }
            ))
        ));
        assert_eq!(
            state
                .connection
                .query_row("SELECT COUNT(*) FROM provider_bindings", [], |row| row
                    .get::<_, i64>(0),)
                .unwrap(),
            0,
            "journal fences never manufacture a provider binding"
        );
        assert_eq!(
            state
                .connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            81
        );
        validate_schema14(&state.connection).unwrap();
    }

    #[test]
    fn schema14_provisional_lease_installs_pending_lock_then_reacquires_same_inode() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());

        let transition = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        let lock_path = state_path.join(PROVISIONAL_LOCK_FILE);
        let host_id: String = state
            .connection
            .query_row(
                "SELECT host_id FROM host_identity WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let expected_contents = provisional_lock_contents(&host_id, 1).unwrap();

        let first = state
            .install_or_acquire_provisional_lease(&transition)
            .unwrap();
        assert_eq!(first.lease_generation(), 1);
        first.revalidate_for_mutation(&state_path).unwrap();
        #[cfg(unix)]
        {
            let descriptor_flags =
                nix::fcntl::fcntl(&*first.file, nix::fcntl::FcntlArg::F_GETFD).unwrap();
            assert_ne!(descriptor_flags & nix::libc::FD_CLOEXEC, 0);
        }
        assert_eq!(fs::read(&lock_path).unwrap(), expected_contents);
        let first_identity = file_identity(&fs::metadata(&lock_path).unwrap());
        assert!(matches!(
            state.install_or_acquire_provisional_lease(&transition),
            Err(StateError::ProvisionalLeaseBusy)
        ));
        drop(first);

        let second = state
            .install_or_acquire_provisional_lease(&transition)
            .unwrap();
        assert_eq!(second.lease_generation(), 1);
        second.revalidate_for_mutation(&state_path).unwrap();
        assert_eq!(
            file_identity(&fs::metadata(&lock_path).unwrap()),
            first_identity,
            "a ready provisional lock must be reacquired, never recreated"
        );
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT provisional_lease_generation, provisional_lock_phase,
                            provisional_lock_device, provisional_lock_inode
                     FROM host_operational_metadata WHERE singleton = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .unwrap(),
            (
                1,
                "ready".to_owned(),
                Some(i64::try_from(first_identity.device).unwrap()),
                Some(i64::try_from(first_identity.inode).unwrap()),
            )
        );
    }

    #[test]
    fn schema14_provisional_lease_finalizes_an_exact_pending_crash_file() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());

        let transition = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        let host_id: String = state
            .connection
            .query_row(
                "SELECT host_id FROM host_identity WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let lock_path = state_path.join(PROVISIONAL_LOCK_FILE);
        let expected_contents = provisional_lock_contents(&host_id, 1).unwrap();
        let mut crashed_installer = open_private_provisional_file(&lock_path, true).unwrap();
        crashed_installer.write_all(&expected_contents).unwrap();
        crashed_installer.sync_all().unwrap();
        sync_directory(&state_path).unwrap();
        drop(crashed_installer);
        let crash_identity = file_identity(&fs::metadata(&lock_path).unwrap());

        let recovered = state
            .install_or_acquire_provisional_lease(&transition)
            .unwrap();
        recovered.revalidate_for_mutation(&state_path).unwrap();
        assert_eq!(
            load_provisional_lock_metadata(&state.connection)
                .unwrap()
                .phase,
            ProvisionalLockPhase::Ready {
                expected_identity: crash_identity
            }
        );
        assert_eq!(fs::read(&lock_path).unwrap(), expected_contents);
    }

    #[test]
    fn schema14_ready_provisional_lease_refuses_missing_and_recreated_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());

        let transition = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        let held = state
            .install_or_acquire_provisional_lease(&transition)
            .unwrap();
        let lock_path = state_path.join(PROVISIONAL_LOCK_FILE);
        let expected_contents = fs::read(&lock_path).unwrap();
        let original_metadata = load_provisional_lock_metadata(&state.connection).unwrap();
        fs::remove_file(&lock_path).unwrap();
        assert!(matches!(
            held.revalidate_for_mutation(&state_path),
            Err(StateError::InvalidProvisionalLease)
        ));
        drop(held);
        assert!(matches!(
            state.install_or_acquire_provisional_lease(&transition),
            Err(StateError::InvalidProvisionalLease)
        ));

        fs::write(&lock_path, &expected_contents).unwrap();
        set_private_file_permissions(&lock_path).unwrap();
        assert!(matches!(
            state.install_or_acquire_provisional_lease(&transition),
            Err(StateError::InvalidProvisionalLease)
        ));
        assert_eq!(
            load_provisional_lock_metadata(&state.connection).unwrap(),
            original_metadata,
            "ready metadata must retain its original inode after every refusal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn schema14_ready_provisional_lease_refuses_a_symlink_without_following_it() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());

        let transition = transition_lease(&state_path);
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        let held = state
            .install_or_acquire_provisional_lease(&transition)
            .unwrap();
        let lock_path = state_path.join(PROVISIONAL_LOCK_FILE);
        let target = state_path.join("provisional.lock.target");
        let target_contents = fs::read(&lock_path).unwrap();
        drop(held);
        fs::remove_file(&lock_path).unwrap();
        fs::write(&target, &target_contents).unwrap();
        set_private_file_permissions(&target).unwrap();
        std::os::unix::fs::symlink(&target, &lock_path).unwrap();

        assert!(matches!(
            state.install_or_acquire_provisional_lease(&transition),
            Err(StateError::InvalidProvisionalLease)
        ));
        assert_eq!(fs::read(&target).unwrap(), target_contents);
        assert!(
            fs::symlink_metadata(&lock_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn migration_restores_the_connection_busy_timeout_after_cutover() {
        let (temporary, _first, _second) = schema12_root();
        let mut connection = Connection::open(temporary.path().join("host.sqlite")).unwrap();
        connection.busy_timeout(Duration::from_millis(73)).unwrap();
        let before: i64 = connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 73);
        let lease = transition_lease(temporary.path());
        migrate_schema12_to13(&mut connection, &SequenceIds::default(), &lease).unwrap();
        let after: i64 = connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, before);
        validate_schema13(&connection).unwrap();
    }

    #[test]
    fn migration_contention_stops_within_budget_and_leaves_schema12_untouched() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let writer = Connection::open(root.host_database_path()).unwrap();
        writer
            .execute_batch("BEGIN IMMEDIATE")
            .expect("competing writer");
        let before = authoritative_snapshot(&writer);
        let lease = transition_lease(temporary.path());
        let started = Instant::now();
        let result = open_confirmed_cutover(&root, &SequenceIds::default(), &lease);
        let elapsed = started.elapsed();
        assert!(matches!(
            result,
            Err(StateError::ObserverDatabaseDeadlineExceeded)
        ));
        assert!(
            elapsed <= Duration::from_millis(750),
            "migration took {elapsed:?}"
        );
        assert_eq!(schema_version(&writer).unwrap(), D16_SCHEMA_12_VERSION);
        assert_eq!(authoritative_snapshot(&writer), before);
        writer.execute_batch("ROLLBACK").unwrap();
        drop(writer);
        let check = Connection::open(root.host_database_path()).unwrap();
        assert_eq!(schema_version(&check).unwrap(), D16_SCHEMA_12_VERSION);
        assert_eq!(authoritative_snapshot(&check), before);
    }

    #[test]
    fn malformed_schema12_relationship_refuses_before_replacing_schema() {
        let (temporary, _first, _second) = schema12_root();
        let path = temporary.path().join("host.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = OFF")
            .unwrap();
        connection
            .execute(
                "UPDATE workstreams SET location_id = ?1",
                [LocationId::from(Uuid::from_u128(99)).to_string()],
            )
            .unwrap();
        let before = authoritative_snapshot(&connection);
        drop(connection);

        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        assert!(matches!(
            open_confirmed_cutover(&root, &SequenceIds::default(), &lease),
            Err(StateError::MalformedHostSchema)
        ));
        let check = Connection::open(&path).unwrap();
        assert_eq!(schema_version(&check).unwrap(), D16_SCHEMA_12_VERSION);
        assert_eq!(authoritative_snapshot(&check), before);
    }

    #[test]
    fn migration_requires_matching_lease_and_refuses_locked_or_nonregular_roots() {
        let (missing_lock, _first, _second) = schema12_root();
        let root = StateRoot::select(missing_lock.path());
        let missing_lock_result = acquire_transition_lease(missing_lock.path());
        assert!(matches!(
            missing_lock_result,
            Err(StateError::TransitionLeaseRequired)
        ));
        assert_eq!(
            Connection::open(root.host_database_path())
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            D16_SCHEMA_12_VERSION
        );

        let (locked_root, _first, _second) = schema12_root();
        set_private_directory_permissions(locked_root.path()).unwrap();
        let lock_path = locked_root.path().join(TRANSITION_LOCK_FILE);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        set_private_file_permissions_handle(&lock_file, &lock_path).unwrap();
        let held_lock =
            nix::fcntl::Flock::lock(lock_file, nix::fcntl::FlockArg::LockExclusiveNonblock)
                .unwrap();
        assert!(matches!(
            acquire_transition_lease(locked_root.path()),
            Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::LockedTransitionLease
            ))
        ));
        assert_eq!(
            Connection::open(locked_root.path().join("host.sqlite"))
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            D16_SCHEMA_12_VERSION
        );
        drop(held_lock);

        let (foreign_root, _first, _second) = schema12_root();
        let foreign_lease = transition_lease(missing_lock.path());
        let foreign_state_root = StateRoot::select(foreign_root.path());
        let before = authoritative_snapshot(
            &Connection::open(foreign_state_root.host_database_path()).unwrap(),
        );
        assert!(matches!(
            open_confirmed_cutover(&foreign_state_root, &SequenceIds::default(), &foreign_lease),
            Err(StateError::TransitionLeaseRootMismatch)
        ));
        let check = Connection::open(foreign_state_root.host_database_path()).unwrap();
        assert_eq!(schema_version(&check).unwrap(), D16_SCHEMA_12_VERSION);
        assert_eq!(authoritative_snapshot(&check), before);

        let (nonregular_root, _first, _second) = schema12_root();
        set_private_directory_permissions(nonregular_root.path()).unwrap();
        fs::create_dir(nonregular_root.path().join(TRANSITION_LOCK_FILE)).unwrap();
        assert!(matches!(
            acquire_transition_lease(nonregular_root.path()),
            Err(StateError::InvalidTransitionLease)
        ));
    }

    #[test]
    fn cutover_transition_is_read_write_and_migration_is_a_separate_step() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let client_path = temporary.path().join(LEGACY_CLIENT_DATABASE_FILE);
        fs::write(&client_path, b"client-owned opaque bytes").unwrap();
        let before = authoritative_snapshot(&Connection::open(root.host_database_path()).unwrap());
        let lease = transition_lease(temporary.path());
        let mut state = open_cutover_transition(&root, &lease).unwrap();
        assert_eq!(state.mode(), D16OpenMode::CutoverTransition);
        assert_eq!(state.schema_version().unwrap(), D16_SCHEMA_12_VERSION);
        // The under-lease transition handle is explicitly read-write, while
        // this transition seam still leaves migration and client cleanup to
        // their separate steps.
        state
            .connection
            .execute(
                "UPDATE host_identity SET registry_generation = 'transition-read-write'
                 WHERE singleton = 1",
                [],
            )
            .unwrap();
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT registry_generation FROM host_identity WHERE singleton = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "transition-read-write"
        );
        assert_eq!(state.schema_version().unwrap(), D16_SCHEMA_12_VERSION);
        state
            .migrate_schema12_to13(&lease, &SequenceIds::default())
            .unwrap();
        assert_eq!(state.schema_version().unwrap(), D16_HOST_SCHEMA_VERSION);
        // A second call validates schema 13 idempotently and does not add a
        // second Project set or otherwise rewrite the host rows.
        let projects = project_snapshot(&state.connection);
        state
            .migrate_schema12_to13(&lease, &SequenceIds::default())
            .unwrap();
        assert_eq!(project_snapshot(&state.connection), projects);
        assert_eq!(
            fs::read(&client_path).unwrap(),
            b"client-owned opaque bytes"
        );
        assert_ne!(
            authoritative_snapshot(&state.connection),
            before,
            "the test's explicit read-write update is the only pre-migration mutation"
        );
    }

    #[test]
    fn cutover_transition_refuses_a_foreign_lease_without_mutation() {
        let (foreign, _first, _second) = schema12_root();
        let (lease_root, _first, _second) = schema12_root();
        let root = StateRoot::select(foreign.path());
        let before = authoritative_snapshot(&Connection::open(root.host_database_path()).unwrap());
        let lease = transition_lease(lease_root.path());
        assert!(matches!(
            open_cutover_transition(&root, &lease),
            Err(StateError::TransitionLeaseRootMismatch)
        ));
        let check = Connection::open(root.host_database_path()).unwrap();
        assert_eq!(schema_version(&check).unwrap(), D16_SCHEMA_12_VERSION);
        assert_eq!(authoritative_snapshot(&check), before);
    }

    #[test]
    fn cutover_observer_handles_are_deterministic_and_revision_cas_guarded() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let mut state = open_cutover_transition(&root, &lease).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(26));
        let handles = state.live_opencode_observers().unwrap();
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].runtime_id, runtime_id);
        assert_eq!(handles[0].runtime_generation, "runtime-generation-opencode");
        assert_eq!(handles[0].observer_pid, Some(5322));
        assert_eq!(
            handles[0].observer_birth.as_deref(),
            Some("observer-birth-opencode")
        );
        let current = state.current_observer(runtime_id).unwrap();
        assert_eq!(current.runtime_id, runtime_id);
        assert_eq!(current.runtime_generation, "runtime-generation-opencode");
        assert_eq!(current.pid, 5322);
        assert_eq!(current.birth, "observer-birth-opencode");
        assert_eq!(current.revision, Revision::try_from(16).unwrap());

        let standby = ObserverProcessIdentity {
            pid: 6000,
            birth: "standby-birth".to_owned(),
            executable: "opencode-observer".to_owned(),
        };
        let replaced = state
            .compare_and_swap_observer(&lease, runtime_id, current.revision, &standby)
            .unwrap();
        assert_eq!(replaced.pid, standby.pid);
        assert_eq!(replaced.birth, standby.birth);
        assert_eq!(replaced.revision, Revision::try_from(17).unwrap());
        assert_eq!(state.current_observer(runtime_id).unwrap(), replaced);
        assert!(matches!(
            state.compare_and_swap_observer(&lease, runtime_id, current.revision, &standby),
            Err(StateError::ConcurrentWrite)
        ));
        let handles = state.live_opencode_observers().unwrap();
        assert_eq!(handles[0].observer_pid, Some(standby.pid));
        assert_eq!(
            handles[0].observer_birth.as_deref(),
            Some(standby.birth.as_str())
        );
    }

    #[test]
    fn live_opencode_observer_projection_keeps_runtime_and_handle_bound() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let state = open_cutover_transition(&root, &lease).unwrap();
        let projections = state.live_opencode_observer_projections().unwrap();
        assert_eq!(projections.len(), 1);
        let projection = &projections[0];
        assert_eq!(projection.runtime.runtime_id, projection.handle.runtime_id);
        assert_eq!(
            projection.runtime.provider,
            crate::domain::ProviderKind::OpenCode
        );
        assert_eq!(
            projection.runtime.tmux_generation,
            projection.handle.runtime_generation
        );
        assert_eq!(projection.runtime.provider_pid, Some(5321));
        assert_eq!(
            projection.runtime.process_birth.as_deref(),
            Some("birth-opencode")
        );
        assert_eq!(
            projection.runtime.cwd,
            PathBuf::from("/fixture/opencode-cwd")
        );
        assert_eq!(projection.handle.observer_pid, Some(5322));
    }

    #[test]
    fn live_opencode_observers_require_a_non_stopped_runtime_lifecycle() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let state = open_cutover_transition(&root, &lease).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(26));
        state
            .connection
            .execute(
                "UPDATE runtimes SET lifecycle = 'stopped' WHERE runtime_id = ?1",
                [runtime_id.to_string()],
            )
            .unwrap();
        assert!(state.live_opencode_observers().unwrap().is_empty());
        assert!(matches!(
            state.current_observer(runtime_id),
            Err(StateError::HookEvidenceMismatch)
        ));
    }

    #[test]
    fn live_opencode_observers_refuse_mismatched_or_malformed_handles() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let state = open_cutover_transition(&root, &lease).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(26));
        state
            .connection
            .execute(
                "UPDATE opencode_runtime_handles
                 SET runtime_generation = 'wrong-generation'
                 WHERE runtime_id = ?1",
                [runtime_id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            state.live_opencode_observers(),
            Err(StateError::ProviderIdentityMismatch)
        ));
        assert!(matches!(
            state.current_observer(runtime_id),
            Err(StateError::ProviderIdentityMismatch)
        ));

        state
            .connection
            .execute(
                "UPDATE opencode_runtime_handles
                 SET runtime_generation = 'runtime-generation-opencode',
                     observer_pid = NULL, observer_birth = NULL,
                     observer_status = 'ready'
                 WHERE runtime_id = ?1",
                [runtime_id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            state.live_opencode_observers(),
            Err(StateError::InvalidPersistedValue(_))
        ));
        assert!(matches!(
            state.current_observer(runtime_id),
            Err(StateError::InvalidPersistedValue(_))
        ));
    }

    #[test]
    fn live_opencode_observers_require_matching_workstream_provider() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let state = open_cutover_transition(&root, &lease).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(26));
        state
            .connection
            .execute(
                "UPDATE workstreams SET provider = 'codex'
                 WHERE workstream_id = (
                    SELECT workstream_id FROM runtimes WHERE runtime_id = ?1
                 )",
                [runtime_id.to_string()],
            )
            .unwrap();
        assert!(matches!(
            state.live_opencode_observers(),
            Err(StateError::ProviderIdentityMismatch)
        ));
        assert!(matches!(
            state.current_observer(runtime_id),
            Err(StateError::ProviderIdentityMismatch)
        ));
    }

    #[test]
    fn schema13_projection_is_bounded_and_conversion_is_mode_safe() {
        let (temporary, first, second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let lease = transition_lease(temporary.path());
        let state = open_confirmed_cutover(&root, &SequenceIds::default(), &lease).unwrap();
        let projections = state.project_location_projections().unwrap();
        let expected_fingerprint = format!("git-remote-v1:{}", "a".repeat(64));
        assert_eq!(projections.len(), 1);
        let projection = &projections[0];
        assert_eq!(projection.label_location_id, first);
        assert_eq!(projection.display_name, "first");
        assert_eq!(
            projection.repository_fingerprint.as_deref(),
            Some(expected_fingerprint.as_str())
        );
        assert_eq!(projection.locations.len(), 2);
        assert_eq!(projection.locations[0].location_id, first);
        assert!(projection.locations[0].is_label_source);
        assert_eq!(projection.locations[0].display_name, "first");
        assert_eq!(
            projection.locations[0].origin_display.as_deref(),
            Some("origin-a")
        );
        assert_eq!(projection.locations[1].location_id, second);
        assert!(!projection.locations[1].is_label_source);
        assert_eq!(projection.locations[1].display_name, "second");
        // No repository path is present in the public projection type.
        assert_eq!(
            projection.locations[1].origin_display.as_deref(),
            Some("origin-a")
        );
        assert!(state.project_projections().is_ok());
        let registry = state.into_host_registry_under_lease(&lease).unwrap();
        drop(registry);

        let fresh_root = private_root();
        let fresh_path = fresh_root.path().join("fresh-state");
        let fresh = fresh_create(&fresh_path, &SequenceIds::default()).unwrap();
        drop(fresh);
        let fresh_state_root = StateRoot::select(&fresh_path);
        let state = open_current_only(&fresh_state_root).unwrap();
        state
            .connection
            .execute(
                "UPDATE host_identity SET registry_generation = 'current-rw'
                 WHERE singleton = 1",
                [],
            )
            .unwrap();
        assert_eq!(state.project_projections().unwrap(), Vec::new());
        drop(state.into_host_registry().unwrap());
    }

    #[test]
    fn current_only_refresh_captures_private_members_and_rejects_transition_artifacts() {
        let (temporary, first, second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let state = open_confirmed_cutover(&root, &ids, &lease).unwrap();
        drop(state);
        drop(lease);
        fs::remove_file(temporary.path().join(TRANSITION_LOCK_FILE)).unwrap();

        let mut state = open_current_only(&root).unwrap();
        let project = state.projects().unwrap().pop().unwrap();
        let capture = state.capture_project_refresh(project.project_id).unwrap();
        assert_eq!(capture.project_id, project.project_id);
        assert_eq!(capture.project_revision, project.revision);
        assert_eq!(
            capture
                .members
                .iter()
                .map(|member| member.location_id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        assert_eq!(capture.members[0].repository_path, PathBuf::from("/first"));
        assert_eq!(
            capture.members[0].expected_revision,
            Revision::try_from(7).unwrap()
        );

        let fingerprint = format!("git-remote-v1:{}", "a".repeat(64));
        let input = ProjectRefreshInput {
            selected_project_id: capture.project_id,
            selected_project_revision: capture.project_revision,
            members: capture
                .members
                .iter()
                .map(|member| ProjectRefreshMember {
                    location_id: member.location_id,
                    expected_revision: member.expected_revision,
                    observation: ProjectRefreshObservation {
                        display_name: if member.location_id == first {
                            "first".to_owned()
                        } else {
                            "second".to_owned()
                        },
                        repository_fingerprint: Some(fingerprint.clone()),
                        remote_identity_display: Some("origin-a".to_owned()),
                    },
                })
                .collect(),
        };
        state.refresh_project(&input, &ids).unwrap();

        let lock_path = temporary.path().join(TRANSITION_LOCK_FILE);
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        set_private_file_permissions_handle(&lock, &lock_path).unwrap();
        drop(lock);
        assert!(matches!(
            state.capture_project_refresh(project.project_id),
            Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::TransitionLeasePresent
            ))
        ));
    }

    #[test]
    fn browser_root_update_uses_exact_revision_cas() {
        let temporary = private_root();
        let root_path = temporary.path().join("state");
        let ids = SequenceIds::default();
        let _state = fresh_create(&root_path, &ids).unwrap();
        let state_root = StateRoot::select(&root_path);
        let mut state = open_current_only(&state_root).unwrap();
        let browser_root = temporary.path().join("browser");
        fs::create_dir(&browser_root).unwrap();
        let initial = state.project_browser_root_revision().unwrap();
        let next = state
            .set_project_browser_root(initial, browser_root.to_str().unwrap())
            .unwrap();
        // A missing settings row is the virtual initial revision; the first
        // persisted value therefore remains at Revision::INITIAL.
        assert_eq!(next.revision, initial);
        assert_eq!(
            state.project_browser_root_revision().unwrap(),
            next.revision
        );
        let second_root = temporary.path().join("browser-second");
        fs::create_dir(&second_root).unwrap();
        let bumped = state
            .set_project_browser_root(next.revision, second_root.to_str().unwrap())
            .unwrap();
        assert_eq!(bumped.revision, next.revision.next());
        assert!(matches!(
            state.set_project_browser_root(initial, browser_root.to_str().unwrap()),
            Err(StateError::ConcurrentWrite)
        ));
    }

    #[test]
    fn registration_creates_or_joins_project_atomically() {
        let temporary = private_root();
        let root_path = temporary.path().join("state");
        let ids = SequenceIds::default();
        let _state = fresh_create(&root_path, &ids).unwrap();
        let state_root = StateRoot::select(&root_path);
        let mut state = open_current_only(&state_root).unwrap();
        let first_path = temporary.path().join("first");
        let second_path = temporary.path().join("second");
        let third_path = temporary.path().join("third");
        let fingerprint = format!("git-remote-v1:{}", "c".repeat(64));

        let first = state
            .register_project_location(&first_path, "first", None, None, &ids)
            .unwrap();
        assert_eq!(first.project.revision, Revision::INITIAL);
        let second = state
            .register_project_location(
                &second_path,
                "second",
                Some(&fingerprint),
                Some("origin-c"),
                &ids,
            )
            .unwrap();
        assert_eq!(second.project.revision, Revision::INITIAL);
        let third = state
            .register_project_location(
                &third_path,
                "third",
                Some(&fingerprint),
                Some("origin-c"),
                &ids,
            )
            .unwrap();
        assert_eq!(third.project.project_id, second.project.project_id);
        assert_eq!(third.project.revision, second.project.revision.next());
        assert_eq!(state.project_projections().unwrap().len(), 2);
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
                    [second.project.project_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn project_reconciliation_splits_and_preserves_the_surviving_label_source() {
        let (temporary, first, second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let mut state = open_confirmed_cutover(&root, &ids, &lease).expect("cutover migration");
        let original = state.projects().unwrap().pop().unwrap();
        let split = format!("git-remote-v1:{}", "b".repeat(64));
        let second_project = state
            .refresh_project(
                &ProjectRefreshInput {
                    selected_project_id: original.project_id,
                    selected_project_revision: original.revision,
                    members: vec![
                        ProjectRefreshMember {
                            location_id: first,
                            expected_revision: Revision::try_from(7).unwrap(),
                            observation: ProjectRefreshObservation {
                                display_name: "first".to_owned(),
                                repository_fingerprint: Some(format!(
                                    "git-remote-v1:{}",
                                    "a".repeat(64)
                                )),
                                remote_identity_display: Some("origin-a".to_owned()),
                            },
                        },
                        ProjectRefreshMember {
                            location_id: second,
                            expected_revision: Revision::try_from(8).unwrap(),
                            observation: ProjectRefreshObservation {
                                display_name: "second-new".to_owned(),
                                repository_fingerprint: Some(split.clone()),
                                remote_identity_display: Some("origin-b".to_owned()),
                            },
                        },
                    ],
                },
                &ids,
            )
            .unwrap();
        let second_project = second_project
            .projects
            .into_iter()
            .find(|project| project.label_location_id == second)
            .unwrap();
        assert_ne!(second_project.project_id, original.project_id);
        assert_eq!(second_project.label_location_id, second);
        let remaining = state.project(original.project_id).unwrap().unwrap();
        assert_eq!(remaining.label_location_id, first);
        assert_eq!(remaining.display_name, "first");
        assert_eq!(remaining.revision, Revision::INITIAL.next());

        let first_fingerprint = format!("git-remote-v1:{}", "c".repeat(64));
        let updated = state
            .refresh_project(
                &ProjectRefreshInput {
                    selected_project_id: original.project_id,
                    selected_project_revision: state
                        .project(original.project_id)
                        .unwrap()
                        .unwrap()
                        .revision,
                    members: vec![ProjectRefreshMember {
                        location_id: first,
                        expected_revision: Revision::try_from(8).unwrap(),
                        observation: ProjectRefreshObservation {
                            display_name: "first-renamed".to_owned(),
                            repository_fingerprint: Some(first_fingerprint.clone()),
                            remote_identity_display: Some("origin-c".to_owned()),
                        },
                    }],
                },
                &ids,
            )
            .unwrap();
        let updated = updated.selected_project.unwrap();
        assert_eq!(updated.project_id, original.project_id);
        assert_eq!(updated.label_location_id, first);
        assert_eq!(updated.display_name, "first-renamed");
        assert_eq!(
            updated.repository_fingerprint.as_deref(),
            Some(first_fingerprint.as_str())
        );
        assert_eq!(updated.revision, Revision::try_from(3).unwrap());
    }

    #[test]
    fn project_membership_join_bumps_the_surviving_project_revision_once() {
        let (temporary, first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let mut state = open_confirmed_cutover(&root, &ids, &lease).unwrap();
        let project = state.projects().unwrap().pop().unwrap();
        let joined = LocationId::from(Uuid::from_u128(4));
        let fingerprint = format!("git-remote-v1:{}", "a".repeat(64));
        state
            .connection
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display,
                    revision, project_id
                 ) VALUES (?1, '/joined', 'joined', ?2, 'origin-a', 3, NULL)",
                params![joined.to_string(), fingerprint],
            )
            .unwrap();
        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        reconcile_location_in_transaction(
            &transaction,
            &ProjectRefreshMember {
                location_id: joined,
                expected_revision: Revision::try_from(3).unwrap(),
                observation: ProjectRefreshObservation {
                    display_name: "joined".to_owned(),
                    repository_fingerprint: Some(format!("git-remote-v1:{}", "a".repeat(64))),
                    remote_identity_display: Some("origin-a".to_owned()),
                },
            },
            &ids,
        )
        .unwrap();
        validate_project_membership_transaction(&transaction).unwrap();
        transaction.commit().unwrap();
        let joined_project = state.project(project.project_id).unwrap().unwrap();
        assert_eq!(joined_project.label_location_id, first);
        assert_eq!(joined_project.revision, project.revision.next());
    }

    #[test]
    fn project_refresh_revalidates_complete_membership_before_any_mutation() {
        let (temporary, first, second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let mut state = open_confirmed_cutover(&root, &ids, &lease).unwrap();
        let project = state.projects().unwrap().pop().unwrap();
        let before = project_snapshot(&state.connection);
        let stale_refresh = ProjectRefreshInput {
            selected_project_id: project.project_id,
            selected_project_revision: project.revision,
            members: vec![
                ProjectRefreshMember {
                    location_id: first,
                    expected_revision: Revision::try_from(7).unwrap(),
                    observation: ProjectRefreshObservation {
                        display_name: "first-renamed".to_owned(),
                        repository_fingerprint: Some(format!("git-remote-v1:{}", "a".repeat(64))),
                        remote_identity_display: Some("origin-a".to_owned()),
                    },
                },
                ProjectRefreshMember {
                    location_id: second,
                    expected_revision: Revision::try_from(999).unwrap(),
                    observation: ProjectRefreshObservation {
                        display_name: "second-renamed".to_owned(),
                        repository_fingerprint: None,
                        remote_identity_display: None,
                    },
                },
            ],
        };
        assert!(matches!(
            state.refresh_project(&stale_refresh, &ids),
            Err(StateError::ConcurrentWrite)
        ));
        assert_eq!(project_snapshot(&state.connection), before);

        let success = ProjectRefreshInput {
            selected_project_id: project.project_id,
            selected_project_revision: project.revision,
            members: vec![
                stale_refresh.members[0].clone(),
                ProjectRefreshMember {
                    location_id: second,
                    expected_revision: Revision::try_from(8).unwrap(),
                    observation: ProjectRefreshObservation {
                        display_name: "second-renamed".to_owned(),
                        repository_fingerprint: None,
                        remote_identity_display: None,
                    },
                },
            ],
        };
        let result = state.refresh_project(&success, &ids).unwrap();
        assert_eq!(
            result.selected_project.unwrap().project_id,
            project.project_id
        );
        let second_project: String = state
            .connection
            .query_row(
                "SELECT project_id FROM project_locations WHERE location_id = ?1",
                [second.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(second_project, project.project_id.to_string());
    }

    #[test]
    fn missing_fingerprint_on_label_source_retains_project_and_reopens_cleanly() {
        let (temporary, first, second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let ids = SequenceIds::default();
        let lease = transition_lease(temporary.path());
        let mut state = open_confirmed_cutover(&root, &ids, &lease).unwrap();
        let project = state.projects().unwrap().pop().unwrap();
        let positive = format!("git-remote-v1:{}", "a".repeat(64));
        let result = state
            .refresh_project(
                &ProjectRefreshInput {
                    selected_project_id: project.project_id,
                    selected_project_revision: project.revision,
                    members: vec![
                        ProjectRefreshMember {
                            location_id: first,
                            expected_revision: Revision::try_from(7).unwrap(),
                            observation: ProjectRefreshObservation {
                                display_name: "first-without-origin".to_owned(),
                                repository_fingerprint: None,
                                remote_identity_display: None,
                            },
                        },
                        ProjectRefreshMember {
                            location_id: second,
                            expected_revision: Revision::try_from(8).unwrap(),
                            observation: ProjectRefreshObservation {
                                display_name: "second".to_owned(),
                                repository_fingerprint: Some(positive.clone()),
                                remote_identity_display: Some("origin-a".to_owned()),
                            },
                        },
                    ],
                },
                &ids,
            )
            .unwrap();
        assert_eq!(
            result
                .selected_project
                .unwrap()
                .repository_fingerprint
                .as_deref(),
            Some(positive.as_str())
        );
        drop(state);
        drop(lease);
        fs::remove_file(temporary.path().join(TRANSITION_LOCK_FILE)).unwrap();
        let reopened = open_current_only(&root).unwrap();
        let reopened_project = reopened.projects().unwrap().pop().unwrap();
        assert_eq!(
            reopened_project.repository_fingerprint.as_deref(),
            Some(positive.as_str())
        );
        let source_fingerprint: Option<String> = reopened
            .connection
            .query_row(
                "SELECT remote_identity_fingerprint FROM project_locations
                 WHERE location_id = ?1",
                [first.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_fingerprint, None);
    }

    #[test]
    fn current_only_accepts_only_clean_schema13_and_rejects_client_artifact() {
        let temporary = private_root();
        let root_path = temporary.path().join("state");
        let ids = SequenceIds::default();
        fresh_create(&root_path, &ids).unwrap();
        let root = StateRoot::select(&root_path);
        let state = open_current_only(&root).unwrap();
        assert_eq!(state.mode(), D16OpenMode::CurrentOnly);
        std::fs::write(root_path.join(LEGACY_CLIENT_DATABASE_FILE), b"opaque").unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::CutoverRequired)
        ));
    }

    #[test]
    fn current_only_refuses_an_exact_transition_lease_but_observer_bypasses_it() {
        let temporary = private_root();
        let root_path = temporary.path().join("state");
        let ids = SequenceIds::default();
        fresh_create(&root_path, &ids).unwrap();
        let root = StateRoot::select(&root_path);
        let lease = transition_lease(&root_path);
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::StateRecoveryRequired(
                StateRecoveryReason::TransitionLeasePresent
            ))
        ));
        let observer = open_observer_transition(&root).unwrap();
        assert_eq!(observer.mode(), D16OpenMode::ObserverTransition);
        drop(observer);
        drop(lease);
        fs::remove_file(root_path.join(TRANSITION_LOCK_FILE)).unwrap();
        assert_eq!(
            open_current_only(&root).unwrap().mode(),
            D16OpenMode::CurrentOnly
        );
    }

    #[cfg(unix)]
    #[test]
    fn d16_open_refuses_symlink_or_nonregular_host_database_without_mutation() {
        let temporary = private_root();
        let root_path = temporary.path().join("state");
        fresh_create(&root_path, &SequenceIds::default()).unwrap();
        let root = StateRoot::select(&root_path);
        let host = root.host_database_path();
        let real = root_path.join("host.sqlite.real");
        fs::rename(&host, &real).unwrap();
        std::os::unix::fs::symlink(&real, &host).unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::MalformedHostSchema)
        ));
        assert!(
            fs::symlink_metadata(&host)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&host).unwrap();
        fs::create_dir(&host).unwrap();
        assert!(matches!(
            open_observer_transition(&root),
            Err(StateError::MalformedHostSchema)
        ));
        assert!(fs::symlink_metadata(&host).unwrap().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn direct_d16_opens_validate_the_root_before_inspecting_target_or_sentinel() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temporary = private_root();
        let target = temporary.path().join("target");
        let state = fresh_create(&target, &SequenceIds::default()).unwrap();
        drop(state);
        let target_database = target.join("host.sqlite");
        let target_before = fs::read(&target_database).unwrap();
        let sentinel = target.join("sentinel");
        fs::write(&sentinel, b"target sentinel").unwrap();
        let link = temporary.path().join("link");
        symlink(&target, &link).unwrap();
        let linked_root = StateRoot::select(&link);
        assert!(matches!(
            open_current_only(&linked_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NotDirectory
            ))
        ));
        assert!(matches!(
            open_observer_transition(&linked_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NotDirectory
            ))
        ));
        assert_eq!(fs::read(&target_database).unwrap(), target_before);
        assert_eq!(fs::read(&sentinel).unwrap(), b"target sentinel");

        fs::remove_file(&link).unwrap();
        let parent_link = temporary.path().join("parent-link");
        symlink(temporary.path(), &parent_link).unwrap();
        let ancestor_linked_root = StateRoot::select(parent_link.join("target"));
        assert!(matches!(
            open_current_only(&ancestor_linked_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NonCanonicalDirectory
            ))
        ));
        assert!(matches!(
            open_observer_transition(&ancestor_linked_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NonCanonicalDirectory
            ))
        ));
        assert_eq!(fs::read(&target_database).unwrap(), target_before);
        assert_eq!(fs::read(&sentinel).unwrap(), b"target sentinel");
        fs::remove_file(&parent_link).unwrap();

        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let nonprivate_root = StateRoot::select(&target);
        assert!(matches!(
            open_current_only(&nonprivate_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NonPrivateDirectory
            ))
        ));
        assert!(matches!(
            open_observer_transition(&nonprivate_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NonPrivateDirectory
            ))
        ));
        assert_eq!(fs::read(&target_database).unwrap(), target_before);
        assert_eq!(fs::read(&sentinel).unwrap(), b"target sentinel");

        let sentinel_root_path = temporary.path().join("sentinel-root");
        fs::write(&sentinel_root_path, b"root sentinel").unwrap();
        let non_directory_root = StateRoot::select(&sentinel_root_path);
        assert!(matches!(
            open_current_only(&non_directory_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NotDirectory
            ))
        ));
        assert!(matches!(
            open_observer_transition(&non_directory_root),
            Err(StateError::FreshRootRejected(
                FreshRootRejection::NotDirectory
            ))
        ));
        assert_eq!(fs::read(&sentinel_root_path).unwrap(), b"root sentinel");
    }

    #[test]
    fn schema13_validation_rejects_foreign_key_and_source_membership_corruption() {
        let (foreign_root, _first, _second) = schema12_root();
        let root = StateRoot::select(foreign_root.path());
        let lease = transition_lease(foreign_root.path());
        let state = open_confirmed_cutover(&root, &SequenceIds::default(), &lease).unwrap();
        drop(state);
        let corrupt = Connection::open(root.host_database_path()).unwrap();
        corrupt.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        corrupt
            .execute(
                "UPDATE project_locations SET project_id = 'not-a-project'",
                [],
            )
            .unwrap();
        drop(corrupt);
        drop(lease);
        fs::remove_file(foreign_root.path().join(TRANSITION_LOCK_FILE)).unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::MalformedHostSchema)
        ));

        let (source_root, _first, _second) = schema12_root();
        let root = StateRoot::select(source_root.path());
        let lease = transition_lease(source_root.path());
        let state = open_confirmed_cutover(&root, &SequenceIds::default(), &lease).unwrap();
        drop(state);
        let corrupt = Connection::open(root.host_database_path()).unwrap();
        corrupt
            .execute("UPDATE projects SET display_name = 'wrong-source'", [])
            .unwrap();
        drop(corrupt);
        drop(lease);
        fs::remove_file(source_root.path().join(TRANSITION_LOCK_FILE)).unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::MalformedHostSchema)
        ));

        let (fingerprint_root, _first, _second) = schema12_root();
        let root = StateRoot::select(fingerprint_root.path());
        let lease = transition_lease(fingerprint_root.path());
        let state = open_confirmed_cutover(&root, &SequenceIds::default(), &lease).unwrap();
        drop(state);
        let corrupt = Connection::open(root.host_database_path()).unwrap();
        corrupt
            .execute(
                "UPDATE projects SET repository_fingerprint = ?1",
                [format!("git-remote-v1:{}", "b".repeat(64))],
            )
            .unwrap();
        drop(corrupt);
        drop(lease);
        fs::remove_file(fingerprint_root.path().join(TRANSITION_LOCK_FILE)).unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::MalformedHostSchema)
        ));

        let (none_root, _first, _second) = schema12_root();
        let root = StateRoot::select(none_root.path());
        let lease = transition_lease(none_root.path());
        let state = open_confirmed_cutover(&root, &SequenceIds::default(), &lease).unwrap();
        drop(state);
        let corrupt = Connection::open(root.host_database_path()).unwrap();
        corrupt
            .execute("UPDATE projects SET repository_fingerprint = NULL", [])
            .unwrap();
        drop(corrupt);
        drop(lease);
        fs::remove_file(none_root.path().join(TRANSITION_LOCK_FILE)).unwrap();
        assert!(matches!(
            open_current_only(&root),
            Err(StateError::MalformedHostSchema)
        ));
    }

    #[test]
    fn observer_transition_accepts_schema12_but_not_legacy_versions() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let state = open_observer_transition(&root).unwrap();
        assert_eq!(state.mode(), D16OpenMode::ObserverTransition);
        assert_eq!(
            state
                .connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(state);
        let connection = Connection::open(root.host_database_path()).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 11;")
            .unwrap();
        drop(connection);
        assert!(matches!(
            open_observer_transition(&root),
            Err(StateError::HostStateResetRequired(11))
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn schema12_observer_settled_transition_is_idempotent_without_schema13_table() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let mut state = open_observer_transition(&root).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(26));
        let session =
            ProviderSessionId::new(ProviderKind::OpenCode, "native-session-opencode").unwrap();
        let observation = |runtime_revision| OpenCodeLifecycleObservation {
            generation: "runtime-generation-opencode".to_owned(),
            cwd: PathBuf::from("/fixture/opencode-cwd"),
            runtime_revision,
            session: session.clone(),
            observer_pid: 5322,
            observer_birth: "observer-birth-opencode".to_owned(),
            hint: LifecycleHint::Settled {
                message_id: Some("schema12-message".to_owned()),
            },
        };
        let first_revision = Revision::try_from(15).unwrap();
        assert_eq!(
            state
                .observer_apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(first_revision),
                    ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
                )
                .unwrap(),
            first_revision.next()
        );
        let snapshot = |state: &D16State| {
            let runtime_revision: i64 = state
                .connection
                .query_row(
                    "SELECT revision FROM runtimes WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let binding_revision: i64 = state
                .connection
                .query_row(
                    "SELECT revision FROM provider_bindings WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let workstream_revision: i64 = state
                .connection
                .query_row(
                    "SELECT revision FROM workstreams WHERE workstream_id = ?1",
                    [WorkstreamId::from(Uuid::from_u128(25)).to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let attention_revision: i64 = state
                .connection
                .query_row(
                    "SELECT revision FROM attention_states WHERE workstream_id = ?1",
                    [WorkstreamId::from(Uuid::from_u128(25)).to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            (
                runtime_revision,
                binding_revision,
                workstream_revision,
                attention_revision,
            )
        };
        let after_first = snapshot(&state);
        let table_count: i64 = state
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'opencode_settled_messages'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
        assert_eq!(
            state
                .observer_apply_opencode_lifecycle_observation(
                    runtime_id,
                    &observation(first_revision.next()),
                    ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
                )
                .unwrap(),
            first_revision.next()
        );
        assert_eq!(snapshot(&state), after_first);

        let working = |runtime_revision| OpenCodeLifecycleObservation {
            generation: "runtime-generation-opencode".to_owned(),
            cwd: PathBuf::from("/fixture/opencode-cwd"),
            runtime_revision,
            session: session.clone(),
            observer_pid: 5322,
            observer_birth: "observer-birth-opencode".to_owned(),
            hint: LifecycleHint::Working,
        };
        let working_revision = first_revision.next();
        assert_eq!(
            state
                .observer_apply_opencode_lifecycle_observation(
                    runtime_id,
                    &working(working_revision),
                    ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
                )
                .unwrap(),
            working_revision.next()
        );
        let after_working = snapshot(&state);
        assert_eq!(
            state
                .observer_apply_opencode_lifecycle_observation(
                    runtime_id,
                    &working(working_revision.next()),
                    ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
                )
                .unwrap(),
            working_revision.next()
        );
        assert_eq!(snapshot(&state), after_working);
    }

    #[test]
    fn observer_write_uses_the_transition_state_connection_only() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let mut state = open_observer_transition(&root).unwrap();
        let changed = state
            .observer_write(ObserverDatabaseDeadline::from_now(Duration::from_millis(50)), |connection| {
                connection.execute(
                    "UPDATE host_identity SET registry_generation = 'observer-write' WHERE singleton = 1",
                    [],
                )
            })
            .unwrap();
        assert_eq!(changed, 1);
        let generation: String = state
            .connection
            .query_row(
                "SELECT registry_generation FROM host_identity WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation, "observer-write");
        let fresh_root = private_root();
        let fresh_path = fresh_root.path().join("state");
        let ids = SequenceIds::default();
        let mut fresh = fresh_create(&fresh_path, &ids).unwrap();
        assert!(matches!(
            fresh.observer_write(
                ObserverDatabaseDeadline::from_now(Duration::from_millis(1)),
                |_| { Ok::<_, rusqlite::Error>(()) }
            ),
            Err(StateError::StateRecoveryRequired(_))
        ));

        let runtime_id = RuntimeId::from(Uuid::from_u128(22));
        let failed: Result<(), StateError> = state.observer_write_with_degraded_marker(
            runtime_id,
            "generation-a",
            ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
            |_| Err(rusqlite::Error::InvalidQuery),
        );
        assert!(matches!(failed, Err(StateError::Sqlite(_))));
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-a").unwrap(),
            Some(ObserverDegradedReason::CommitFailed)
        );
    }

    #[test]
    fn observer_transition_routes_codex_lifecycle_and_metadata_through_typed_state() {
        let (temporary, _first, _second) = schema12_root();
        let root = StateRoot::select(temporary.path());
        let mut state = open_observer_transition(&root).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(22));
        let runtime = state.observer_runtime_by_id(runtime_id).unwrap().unwrap();
        assert_eq!(runtime.provider, ProviderKind::Codex);
        assert_eq!(runtime.tmux_generation, "tmux-generation-a");
        state
            .connection
            .execute(
                "UPDATE provider_bindings SET name_state = 'unavailable',
                        runtime_generation = 'tmux-generation-a'
                 WHERE runtime_id = ?1",
                [runtime_id.to_string()],
            )
            .unwrap();

        let observation = LifecycleObservation {
            event: LifecycleEvent::UserPromptSubmit,
            cwd: "/fixture/cwd".to_owned(),
            native_session_id: "native-session-a".to_owned(),
            turn_id: None,
            source: None,
        };
        state
            .observer_apply_codex_lifecycle_observation(
                runtime_id,
                "tmux-generation-a",
                &observation,
                ObserverDatabaseDeadline::from_now(Duration::from_millis(750)),
            )
            .unwrap();
        state
            .observer_record_thread_metadata(
                runtime_id,
                "tmux-generation-a",
                &ProviderSessionId::new(ProviderKind::Codex, "native-session-a").unwrap(),
                Some("renamed thread"),
                ObserverDatabaseDeadline::from_now(Duration::from_millis(750)),
            )
            .unwrap();

        assert_eq!(
            state
                .observer_runtime_by_id(runtime_id)
                .unwrap()
                .unwrap()
                .status,
            RuntimeStatus::Working
        );
        let name: String = state
            .connection
            .query_row(
                "SELECT observed_thread_name FROM provider_bindings WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "renamed thread");
    }

    #[test]
    fn marker_is_generation_scoped_and_contains_no_event_payload() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(9));
        let path = write_observer_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-a",
            ObserverDegradedReason::BusyDeadline,
        )
        .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("turn"));
        assert!(!body.contains("payload"));
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-a").unwrap(),
            Some(ObserverDegradedReason::BusyDeadline)
        );
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-b").unwrap(),
            None
        );
    }

    #[test]
    fn marker_reconciliation_removes_only_the_current_generation() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(90));
        write_observer_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-current",
            ObserverDegradedReason::BusyDeadline,
        )
        .unwrap();
        write_observer_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-stale",
            ObserverDegradedReason::CommitFailed,
        )
        .unwrap();

        clear_observer_degraded_marker(temporary.path(), runtime_id, "generation-current").unwrap();

        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-current")
                .unwrap(),
            None
        );
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-stale")
                .unwrap(),
            Some(ObserverDegradedReason::CommitFailed)
        );
    }

    #[test]
    fn successful_observer_reconciliation_clears_current_marker() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(91));
        run_observer_write_with_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-reconciled",
            ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
            || Err::<(), _>(rusqlite::Error::InvalidQuery),
        )
        .unwrap_err();
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-reconciled")
                .unwrap(),
            Some(ObserverDegradedReason::CommitFailed)
        );
        run_observer_write_with_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-reconciled",
            ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
            || Ok::<_, rusqlite::Error>(()),
        )
        .unwrap();
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-reconciled")
                .unwrap(),
            None
        );
    }

    #[test]
    fn marker_reconciliation_rejects_malformed_or_foreign_candidates() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(92));
        let generation = "generation-protected";
        let marker = write_observer_degraded_marker(
            temporary.path(),
            runtime_id,
            generation,
            ObserverDegradedReason::BusyDeadline,
        )
        .unwrap();
        fs::write(&marker, b"not a marker").unwrap();
        set_private_file_permissions(&marker).unwrap();
        assert!(matches!(
            clear_observer_degraded_marker(temporary.path(), runtime_id, generation),
            Err(StateError::InvalidObserverDegradedMarker)
        ));
        assert!(marker.exists());

        let wire = serde_json::to_vec(&ObserverDegradedMarkerWire {
            version: 1,
            runtime_id: runtime_id.to_string(),
            runtime_generation: "foreign-generation".to_owned(),
            reason: ObserverDegradedReason::BusyDeadline,
        })
        .unwrap();
        fs::write(&marker, wire).unwrap();
        set_private_file_permissions(&marker).unwrap();
        assert!(matches!(
            clear_observer_degraded_marker(temporary.path(), runtime_id, generation),
            Err(StateError::InvalidObserverDegradedMarker)
        ));
        assert!(marker.exists());
    }

    #[test]
    fn marker_deadline_cutoff_is_fail_closed_before_creating_derived_state() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(11));
        let result = write_observer_degraded_marker_with_deadline(
            temporary.path(),
            runtime_id,
            "generation-cutoff",
            ObserverDegradedReason::BusyDeadline,
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("one second is representable before now"),
        );
        assert!(matches!(
            result,
            Err(StateError::ObserverDatabaseDeadlineExceeded)
        ));
        assert!(temporary.path().join("run").metadata().is_err());
    }

    #[test]
    fn marker_promotes_a_valid_crash_leftover_and_rejects_divergent_candidates() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(10));
        let marker =
            observer_degraded_marker_path(temporary.path(), runtime_id, "generation-a").unwrap();
        let parent = marker.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        for directory in [
            parent,
            parent.parent().unwrap(),
            parent.parent().unwrap().parent().unwrap(),
        ] {
            set_private_directory_permissions(directory).unwrap();
        }
        let temp = observer_degraded_marker_temp_path(&marker);
        let body = serde_json::to_vec(&ObserverDegradedMarkerWire {
            version: 1,
            runtime_id: runtime_id.to_string(),
            runtime_generation: "generation-a".to_owned(),
            reason: ObserverDegradedReason::BusyDeadline,
        })
        .unwrap();
        fs::write(&temp, body).unwrap();
        set_private_file_permissions(&temp).unwrap();
        File::open(&temp).unwrap().sync_all().unwrap();
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-a").unwrap(),
            Some(ObserverDegradedReason::BusyDeadline)
        );
        write_observer_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-a",
            ObserverDegradedReason::BusyDeadline,
        )
        .unwrap();
        assert!(!temp.exists());
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-a").unwrap(),
            Some(ObserverDegradedReason::BusyDeadline)
        );

        let divergent_body = serde_json::to_vec(&ObserverDegradedMarkerWire {
            version: 1,
            runtime_id: runtime_id.to_string(),
            runtime_generation: "generation-a".to_owned(),
            reason: ObserverDegradedReason::CommitFailed,
        })
        .unwrap();
        fs::write(&temp, divergent_body).unwrap();
        set_private_file_permissions(&temp).unwrap();
        File::open(&temp).unwrap().sync_all().unwrap();
        assert!(matches!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-a"),
            Err(StateError::InvalidObserverDegradedMarker)
        ));
        assert!(matches!(
            write_observer_degraded_marker(
                temporary.path(),
                runtime_id,
                "generation-a",
                ObserverDegradedReason::BusyDeadline,
            ),
            Err(StateError::InvalidObserverDegradedMarker)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn marker_rejects_a_symlinked_derived_directory_without_following_it() {
        let temporary = private_root();
        let outside = private_root();
        std::os::unix::fs::symlink(outside.path(), temporary.path().join("run")).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(13));
        assert!(matches!(
            write_observer_degraded_marker(
                temporary.path(),
                runtime_id,
                "generation-symlink",
                ObserverDegradedReason::BusyDeadline,
            ),
            Err(StateError::InvalidObserverDegradedMarker)
        ));
        assert!(matches!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-symlink"),
            Err(StateError::InvalidObserverDegradedMarker)
        ));
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn observer_deadline_retries_only_busy_and_stops_at_budget() {
        let result: Result<(), ObserverDatabaseError> =
            ObserverDatabaseDeadline::from_now(Duration::from_millis(3)).run(|| {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                        extended_code: 5,
                    },
                    None,
                ))
            });
        assert!(matches!(
            result,
            Err(ObserverDatabaseError::DeadlineExceeded)
        ));
    }

    #[test]
    fn observer_deadline_does_not_relabel_a_successful_operation_after_deadline() {
        let result = ObserverDatabaseDeadline::from_now(Duration::from_millis(1)).run(|| {
            std::thread::sleep(Duration::from_millis(5));
            Ok::<_, rusqlite::Error>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn observer_database_deadline_is_one_absolute_budget_when_reused() {
        let deadline = ObserverDatabaseDeadline::until(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("one second is representable before now"),
        );
        assert_eq!(deadline.deadline(), deadline.deadline());
        assert!(matches!(
            deadline.run(|| Ok::<_, rusqlite::Error>(())),
            Err(ObserverDatabaseError::DeadlineExceeded)
        ));
        assert!(matches!(
            deadline.run(|| Ok::<_, rusqlite::Error>(())),
            Err(ObserverDatabaseError::DeadlineExceeded)
        ));
    }

    #[test]
    fn observer_deadline_records_locked_reason_and_nonretryable_commit_failure() {
        let temporary = private_root();
        set_private_directory_permissions(temporary.path()).unwrap();
        let runtime_id = RuntimeId::from(Uuid::from_u128(12));
        let locked: Result<(), StateError> = run_observer_write_with_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-locked",
            ObserverDatabaseDeadline::from_now(Duration::from_millis(3)),
            || {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ffi::ErrorCode::DatabaseLocked,
                        extended_code: 6,
                    },
                    None,
                ))
            },
        );
        assert!(matches!(
            locked,
            Err(StateError::ObserverDatabaseDeadlineExceeded)
        ));
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-locked")
                .unwrap(),
            Some(ObserverDegradedReason::LockedDeadline)
        );

        let failed: Result<(), StateError> = run_observer_write_with_degraded_marker(
            temporary.path(),
            runtime_id,
            "generation-failed",
            ObserverDatabaseDeadline::from_now(Duration::from_millis(50)),
            || Err(rusqlite::Error::InvalidQuery),
        );
        assert!(matches!(failed, Err(StateError::Sqlite(_))));
        assert_eq!(
            read_observer_degraded_marker(temporary.path(), runtime_id, "generation-failed")
                .unwrap(),
            Some(ObserverDegradedReason::CommitFailed)
        );
    }

    #[test]
    fn handover_journal_round_trips_and_restart_action_is_idempotent() {
        let temporary = private_root();
        let mut journal = ObserverHandoverJournal {
            version: 1,
            runtime_id: RuntimeId::from(Uuid::from_u128(11)).to_string(),
            runtime_generation: "generation-a".to_owned(),
            old_observer: ObserverProcessIdentity {
                pid: 10,
                birth: "birth-old".to_owned(),
                executable: "opencode-observer".to_owned(),
            },
            standby_observer: ObserverProcessIdentity {
                pid: 11,
                birth: "birth-new".to_owned(),
                executable: "opencode-observer".to_owned(),
            },
            expected_handle_revision: Revision::INITIAL,
            phase: HandoverPhase::Prepared,
        };
        let lease = transition_lease(temporary.path());
        write_observer_handover_journal(&lease, &journal).unwrap();
        assert_eq!(
            read_observer_handover_journal(temporary.path())
                .unwrap()
                .unwrap()
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 10,
                    birth: "birth-old".to_owned(),
                    revision: Revision::INITIAL,
                })
                .unwrap(),
            HandoverRestartAction::RestoreOldObserver
        );
        journal.transition(HandoverPhase::StandbyReady).unwrap();
        write_observer_handover_journal(&lease, &journal).unwrap();
        assert_eq!(
            read_observer_handover_journal(temporary.path())
                .unwrap()
                .unwrap()
                .phase,
            HandoverPhase::StandbyReady
        );
        assert!(matches!(
            journal.transition(HandoverPhase::Complete),
            Err(StateError::InvalidObserverHandoverTransition)
        ));
    }

    #[test]
    fn handover_activation_ack_is_exact_durable_and_journal_bound() {
        let temporary = private_root();
        let lease = transition_lease(temporary.path());
        let mut journal = sample_journal();
        write_observer_handover_journal(&lease, &journal).unwrap();
        for phase in [
            HandoverPhase::StandbyReady,
            HandoverPhase::OldFrozen,
            HandoverPhase::HandleSwapped,
        ] {
            journal.transition(phase).unwrap();
            write_observer_handover_journal(&lease, &journal).unwrap();
        }
        let ack = ObserverHandoverActivationAck {
            version: 1,
            runtime_id: journal.runtime_id.clone(),
            runtime_generation: journal.runtime_generation.clone(),
            standby_observer: journal.standby_observer.clone(),
            handle_revision: journal.expected_handle_revision.next(),
        };
        write_observer_handover_activation_ack(temporary.path(), &ack).unwrap();
        assert_eq!(
            read_observer_handover_activation_ack(temporary.path()).unwrap(),
            Some(ack.clone())
        );
        write_observer_handover_activation_ack(temporary.path(), &ack).unwrap();

        let mut foreign = ack;
        foreign.standby_observer.pid += 1;
        assert!(matches!(
            write_observer_handover_activation_ack(temporary.path(), &foreign),
            Err(StateError::InvalidObserverHandoverJournal)
        ));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one restart test covers the correlated evidence matrix and temp recovery"
    )]
    fn handover_restart_requires_correlated_handle_evidence_and_recovers_temp_candidate() {
        let temporary = private_root();
        let lease = transition_lease(temporary.path());
        let mut journal = sample_journal();
        journal.phase = HandoverPhase::OldFrozen;
        assert_eq!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 10,
                    birth: "birth-old".to_owned(),
                    revision: Revision::INITIAL,
                })
                .unwrap(),
            HandoverRestartAction::RestoreOldObserver
        );
        assert_eq!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 11,
                    birth: "birth-new".to_owned(),
                    revision: Revision::INITIAL.next(),
                })
                .unwrap(),
            HandoverRestartAction::FinishOldObserverCleanup
        );
        assert!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 99,
                    birth: "birth-other".to_owned(),
                    revision: Revision::INITIAL,
                })
                .is_err()
        );
        assert!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(99)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 10,
                    birth: "birth-old".to_owned(),
                    revision: Revision::INITIAL,
                })
                .is_err()
        );
        assert!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 11,
                    birth: "birth-new".to_owned(),
                    revision: Revision::INITIAL.next().next(),
                })
                .is_err()
        );

        journal.phase = HandoverPhase::HandleSwapped;
        assert_eq!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 11,
                    birth: "birth-new".to_owned(),
                    revision: Revision::INITIAL.next(),
                })
                .unwrap(),
            HandoverRestartAction::FinishOldObserverCleanup
        );
        assert!(
            journal
                .restart_action(&CurrentObserverHandleProof {
                    runtime_id: RuntimeId::from(Uuid::from_u128(11)),
                    runtime_generation: "generation-a".to_owned(),
                    pid: 10,
                    birth: "birth-old".to_owned(),
                    revision: Revision::INITIAL,
                })
                .is_err()
        );

        let mut temp_only = sample_journal();
        temp_only.phase = HandoverPhase::StandbyReady;
        write_observer_handover_journal(&lease, &sample_journal()).unwrap();
        let final_path = observer_handover_journal_path(temporary.path());
        let temp_path = observer_handover_journal_temp_path(temporary.path());
        fs::remove_file(&final_path).unwrap();
        fs::write(&temp_path, serde_json::to_vec(&temp_only).unwrap()).unwrap();
        set_private_file_permissions(&temp_path).unwrap();
        File::open(&temp_path).unwrap().sync_all().unwrap();
        assert_eq!(
            read_observer_handover_journal(temporary.path())
                .unwrap()
                .unwrap()
                .phase,
            HandoverPhase::StandbyReady
        );
        assert_eq!(
            recover_observer_handover_journal(&lease)
                .unwrap()
                .unwrap()
                .phase,
            HandoverPhase::StandbyReady
        );
        assert!(final_path.is_file());
        assert!(!temp_path.exists());

        let mut progressed = temp_only.clone();
        progressed.transition(HandoverPhase::OldFrozen).unwrap();
        write_observer_handover_journal(&lease, &progressed).unwrap();

        let divergent = sample_journal();
        assert!(matches!(
            write_observer_handover_journal(&lease, &divergent),
            Err(StateError::InvalidObserverHandoverJournal)
        ));
        let mut invalid_temp = divergent.clone();
        invalid_temp.phase = HandoverPhase::Complete;
        fs::write(&temp_path, serde_json::to_vec(&invalid_temp).unwrap()).unwrap();
        set_private_file_permissions(&temp_path).unwrap();
        assert!(matches!(
            read_observer_handover_journal(temporary.path()),
            Err(StateError::InvalidObserverHandoverJournal)
        ));
    }
}
