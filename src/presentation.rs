//! Disposable private tmux ownership for the local navigator presentation.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    domain::{OnboardingPhase, Revision, WorkstreamId},
    private_tmux::{TERMINAL_CAPABILITY_CONFIG, copy_mode_scroll_config},
    process::{BoundedProcessError, output_bounded},
    provisional::{PROVISIONAL_MARKER_FILE, ProvisionalPhase, ProvisionalSlot, read_marker},
    runtime::RuntimePaths,
    state::{D16State, ProvisionalLease, TransitionLease, d16::D17OnboardingOperationInventory},
};

const PRESENTATION_DIRECTORY: &str = "presentation";
const PRESENTATION_PREFIX: &str = "wsnav-presentation-";
const NAVIGATOR_WINDOW: &str = "navigator";
const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
const NAVIGATOR_WIDTH_HOOKS: [&str; 2] = ["client-attached", "window-resized"];
/// The normal narrow navigator width, including its outside borders.
const DEFAULT_NAVIGATOR_PANE_WIDTH: u16 = 32;
const PREFERRED_PROVIDER_PANE_WIDTH: u16 = 96;
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_ATTACHMENT_STATUS_BYTES: u64 = 4 * 1024;
const MAX_LEGACY_PRESENTATION_ENTRIES: usize = 32;
const MAX_LEGACY_PANES: usize = 3;
const MAX_LEGACY_CLIENTS: usize = 32;
const MAX_LEGACY_PROCESS_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_LEGACY_CONFIG_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENT_STATUS_BYTES_USIZE: usize = 4 * 1024;
const MAX_LEGACY_RETIREMENT_MARKER_BYTES: usize = 8 * 1024;
const MAX_LEGACY_RETIREMENT_ATTEMPTS: usize = 20;
const ATTACHMENT_STATUS_FILE: &str = "attachment.json";
const PRESENTATION_OWNERSHIP_MARKER_FILE: &str = "ownership.json";
const MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES: usize = 4 * 1024;
const D17_PRESENTATION_CONTEXT_VERSION: u8 = 1;
const MAX_D17_PROVISIONAL_INVENTORY_ENTRIES: usize = 128;
const LEGACY_RETIREMENT_MARKER_FILE: &str = "d16-retirement.json";
const ROLE_OPTION: &str = "@wsnav_role";
const WORKSTREAM_OPTION: &str = "@wsnav_workstream_id";
const SHELL_CLAIM_OPTION: &str = "@wsnav_shell_claim";
const SHELL_CLAIM_ATTEMPTS: usize = 20;
const SHELL_CLAIM_RETRY: Duration = Duration::from_millis(5);
const NAVIGATOR_STOP_ATTEMPTS: usize = 20;
const NAVIGATOR_STOP_RETRY: Duration = Duration::from_millis(5);
const TOPOLOGY_FORMAT: &str = "#{pane_id}\t#{@wsnav_role}\t#{@wsnav_workstream_id}\t#{pane_dead}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}\t#{window_width}\t#{window_height}";
// This value is intentionally confined to the explicit schema-12 cutover
// proof path below. Current presentation topology never reads or writes a host
// alias, but cutover still needs to recognize the old layout exactly.
const LEGACY_PROOF_TOPOLOGY_FORMAT: &str = "#{pane_id}\t#{@wsnav_role}\t#{@wsnav_host_alias}\t#{@wsnav_workstream_id}\t#{pane_dead}\t#{pane_pid}\t#{pane_current_command}\t#{pane_start_command}\t#{pane_title}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}\t#{window_width}\t#{window_height}";
const PRESENTATION_TMUX_CONFIG_PREFIX: &str = concat!(
    "set -g status off\n",
    "set -g mouse on\n",
    "set -g remain-on-exit on\n",
    "set -g prefix C-b\n",
    "set -g prefix2 None\n",
    "unbind-key -a -T prefix\n",
    "unbind-key -a -T root\n",
);
const PRESENTATION_TMUX_CONFIG_SUFFIX: &str = concat!(
    "bind-key -T root MouseDown1Pane select-pane -t = \\; send-keys -M\n",
    "bind-key -T root MouseUp1Pane select-pane -t = \\; send-keys -M\n",
    "bind-key -T root MouseDrag1Pane if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -M\"\n",
    "bind-key -T root WheelUpPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -e\"\n",
    "bind-key -T root WheelDownPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"send-keys -M\"\n",
);

fn presentation_tmux_config() -> String {
    let copy_mode_scroll_config = copy_mode_scroll_config();
    [
        PRESENTATION_TMUX_CONFIG_PREFIX,
        TERMINAL_CAPABILITY_CONFIG,
        &copy_mode_scroll_config,
        PRESENTATION_TMUX_CONFIG_SUFFIX,
    ]
    .concat()
}

fn private_tmux_command() -> Command {
    let mut command = Command::new("tmux");
    command.env_remove("TMUX").arg("-u");
    command
}

fn expected_presentation_config_identity() -> LegacyFileIdentity {
    let config = presentation_tmux_config();
    let mut digest = Sha256::new();
    digest.update(config.as_bytes());
    LegacyFileIdentity {
        size: config.len() as u64,
        mode: 0o600,
        device: 0,
        inode: 0,
        digest: Some(digest.finalize().into()),
    }
}

fn config_content_matches(identity: &LegacyFileIdentity) -> bool {
    let expected = expected_presentation_config_identity();
    identity.size == expected.size && identity.digest == expected.digest
}

/// Returns the exact private D15 presentation configuration for disposable
/// classifier fixtures.  This is hidden from generated API documentation so
/// production callers cannot treat the configuration as a customization
/// surface.
#[doc(hidden)]
#[must_use]
pub fn legacy_presentation_config_for_test() -> String {
    presentation_tmux_config()
}

/// Actions exposed by the private presentation prefix table. The strings are
/// fixed internal ABI values; no arbitrary tmux command can enter this path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationAction {
    CreateOrFocusShell,
    SuppressSplit,
    CloseShell,
    FocusNext,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    LiteralCtrlB,
}

impl PresentationAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateOrFocusShell => "create-or-focus-shell",
            Self::SuppressSplit => "suppress-split",
            Self::CloseShell => "close-shell",
            Self::FocusNext => "focus-next",
            Self::FocusUp => "focus-up",
            Self::FocusDown => "focus-down",
            Self::FocusLeft => "focus-left",
            Self::FocusRight => "focus-right",
            Self::LiteralCtrlB => "literal-c-b",
        }
    }
}

impl FromStr for PresentationAction {
    type Err = PresentationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "create-or-focus-shell" => Ok(Self::CreateOrFocusShell),
            "suppress-split" => Ok(Self::SuppressSplit),
            "close-shell" => Ok(Self::CloseShell),
            "focus-next" => Ok(Self::FocusNext),
            "focus-up" => Ok(Self::FocusUp),
            "focus-down" => Ok(Self::FocusDown),
            "focus-left" => Ok(Self::FocusLeft),
            "focus-right" => Ok(Self::FocusRight),
            "literal-c-b" => Ok(Self::LiteralCtrlB),
            _ => Err(PresentationError::InvalidControlAction),
        }
    }
}

/// A role recognized only after exact private tmux evidence is parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationPaneRole {
    Navigator,
    Provider,
    Utility,
}

/// Ephemeral provider-pane attempt metadata read only by the local navigator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttachmentStatus {
    pub attempt_id: uuid::Uuid,
    pub workstream_id: WorkstreamId,
    pub phase: AttachmentPhase,
}

/// Immutable, presentation-private D17 shell-onboarding context. The seed is
/// intentionally available only to the future D17 materializer; it never
/// enters a navigator snapshot, provider command, or durable host registry.
#[derive(Clone, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the D17 presentation context remains unreachable until the atomic Navigator cutover"
)]
pub(crate) struct D17PresentationContext {
    presentation_id: uuid::Uuid,
    presentation_revision: Revision,
    seed_cwd: PathBuf,
}

impl std::fmt::Debug for D17PresentationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("D17PresentationContext")
            .field("presentation_id", &"<opaque>")
            .field("presentation_revision", &self.presentation_revision)
            .field("seed_cwd", &"<private>")
            .finish()
    }
}

#[allow(
    dead_code,
    reason = "the D17 presentation context remains unreachable until the atomic Navigator cutover"
)]
impl D17PresentationContext {
    #[must_use]
    pub(crate) const fn presentation_id(&self) -> uuid::Uuid {
        self.presentation_id
    }

    #[must_use]
    pub(crate) const fn presentation_revision(&self) -> Revision {
        self.presentation_revision
    }

    #[must_use]
    pub(crate) fn seed_cwd(&self) -> &Path {
        &self.seed_cwd
    }
}

/// Result of the read-only D17 provisional-slot classifier. The caller must
/// hold the stable provisional lease before acting on this result; the
/// classifier itself never creates, adopts, removes, attaches, or signals a
/// presentation or Runtime artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the D17 provisional singleton classifier remains unreachable until the atomic Navigator cutover"
)]
pub(crate) enum D17ProvisionalInventory {
    Vacant,
    Occupied,
}

/// Bounded refusal from D17's cross-presentation provisional-slot inventory.
/// No path, marker body, operation identifier, shell evidence, or provider
/// content crosses this boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[allow(
    dead_code,
    reason = "the D17 provisional singleton classifier remains unreachable until the atomic Navigator cutover"
)]
pub(crate) enum D17ProvisionalInventoryError {
    #[error("D17 provisional inventory is unavailable")]
    Unavailable,
    #[error("D17 provisional inventory is ambiguous")]
    Ambiguous,
}

/// Cross-checks all D17 presentation markers against exact durable onboarding
/// journal claims and registered Runtime paths. Any malformed, changed,
/// markerless, or unregistered runtime-shaped evidence is a closed refusal;
/// the function never makes a new candidate to evade ambiguity.
///
/// The registered paths and operation inventory must come from the same
/// schema-14 passive read while the caller retains the stable provisional
/// lease. They are intentionally private classifier inputs, not Navigator
/// projection data.
#[allow(
    dead_code,
    clippy::too_many_lines,
    reason = "the singleton proof intentionally keeps every marker, journal, and runtime-path cross-check in one fail-closed classifier"
)]
pub(crate) fn classify_d17_provisional_inventory(
    state_root: &Path,
    registered_runtime_paths: &[RuntimePaths],
    operations: &[D17OnboardingOperationInventory],
) -> Result<D17ProvisionalInventory, D17ProvisionalInventoryError> {
    if registered_runtime_paths.len() > MAX_D17_PROVISIONAL_INVENTORY_ENTRIES
        || operations.len() > MAX_D17_PROVISIONAL_INVENTORY_ENTRIES
    {
        return Err(D17ProvisionalInventoryError::Ambiguous);
    }
    let state_root = canonical_d17_inventory_root(state_root)?;
    let mut operations_by_id = BTreeMap::new();
    for operation in operations {
        if operations_by_id
            .insert(operation.operation_id.as_uuid(), operation)
            .is_some()
        {
            return Err(D17ProvisionalInventoryError::Ambiguous);
        }
    }

    let mut matched_operations = BTreeSet::new();
    let mut allowed_runtime_directories = registered_runtime_paths
        .iter()
        .map(|paths| paths.directory.clone())
        .collect::<BTreeSet<_>>();
    let mut occupied = false;
    let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
    match fs::symlink_metadata(&presentation_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(D17ProvisionalInventoryError::Unavailable),
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !is_private_owner_directory(&metadata) =>
        {
            return Err(D17ProvisionalInventoryError::Ambiguous);
        }
        Ok(_) => {
            let entries = fs::read_dir(&presentation_root)
                .map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
            for (count, entry) in entries.enumerate() {
                if count >= MAX_D17_PROVISIONAL_INVENTORY_ENTRIES {
                    return Err(D17ProvisionalInventoryError::Ambiguous);
                }
                let entry = entry.map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
                let directory = entry.path();
                let metadata = fs::symlink_metadata(&directory)
                    .map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_dir()
                    || !is_private_owner_directory(&metadata)
                    || presentation_session_name(&directory).is_none()
                {
                    return Err(D17ProvisionalInventoryError::Ambiguous);
                }
                let context = Presentation::d17_context_from_directory(&state_root, &directory)
                    .map_err(|_| D17ProvisionalInventoryError::Ambiguous)?;
                let marker_path = directory.join(PROVISIONAL_MARKER_FILE);
                let slot = match fs::symlink_metadata(&marker_path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => return Err(D17ProvisionalInventoryError::Unavailable),
                    Ok(_) => Some(
                        read_marker(&state_root, &directory)
                            .map_err(|_| D17ProvisionalInventoryError::Ambiguous)?,
                    ),
                };
                let Some(slot) = slot else {
                    continue;
                };
                if slot.presentation_id() != context.presentation_id()
                    || slot.presentation_revision() != context.presentation_revision()
                {
                    return Err(D17ProvisionalInventoryError::Ambiguous);
                }
                match slot.phase() {
                    ProvisionalPhase::Materializing => {
                        if operations
                            .iter()
                            .any(|operation| operation.runtime_id == slot.candidate_runtime_id())
                        {
                            return Err(D17ProvisionalInventoryError::Ambiguous);
                        }
                        occupied = true;
                        allowed_runtime_directories.insert(slot.runtime_paths().directory.clone());
                    }
                    ProvisionalPhase::Materialized => {
                        match_materialized_slot_operation(
                            &slot,
                            operations,
                            &mut matched_operations,
                        )?;
                        occupied = true;
                        allowed_runtime_directories.insert(slot.runtime_paths().directory.clone());
                    }
                    ProvisionalPhase::HandoffIssued => {
                        match_slot_operation(
                            &slot,
                            &operations_by_id,
                            &mut matched_operations,
                            &[OnboardingPhase::CapabilityIssued],
                        )?;
                        occupied = true;
                        allowed_runtime_directories.insert(slot.runtime_paths().directory.clone());
                    }
                    ProvisionalPhase::RuntimeOwnedLaunching => {
                        match_slot_operation(
                            &slot,
                            &operations_by_id,
                            &mut matched_operations,
                            &[
                                OnboardingPhase::RuntimeOwnedLaunching,
                                OnboardingPhase::ProviderPreparation,
                                OnboardingPhase::ProviderExternalEffectStarted,
                                OnboardingPhase::ProviderExecStarted,
                                OnboardingPhase::KnownAbsentExec,
                                OnboardingPhase::RecoveryRequired,
                                OnboardingPhase::ProviderExecProven,
                            ],
                        )?;
                        require_registered_runtime_path(&slot, registered_runtime_paths)?;
                    }
                    ProvisionalPhase::ProviderExecProven => {
                        match_slot_operation(
                            &slot,
                            &operations_by_id,
                            &mut matched_operations,
                            &[OnboardingPhase::ProviderExecProven],
                        )?;
                        require_registered_runtime_path(&slot, registered_runtime_paths)?;
                    }
                    ProvisionalPhase::Cancelled => {
                        return Err(D17ProvisionalInventoryError::Ambiguous);
                    }
                }
            }
        }
    }
    if operations.iter().any(|operation| {
        operation.phase != OnboardingPhase::RolledBack
            && !matched_operations.contains(&operation.operation_id.as_uuid())
    }) {
        return Err(D17ProvisionalInventoryError::Ambiguous);
    }
    classify_d17_runtime_namespace(&state_root, &allowed_runtime_directories)?;
    Ok(if occupied {
        D17ProvisionalInventory::Occupied
    } else {
        D17ProvisionalInventory::Vacant
    })
}

/// Schema-12 attachment metadata retained only for private legacy proof.
/// Current attachment status is deliberately host-local and uses
/// [`AttachmentStatus`] above.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LegacyAttachmentStatus {
    attempt_id: uuid::Uuid,
    host_alias: String,
    workstream_id: WorkstreamId,
    phase: AttachmentPhase,
}

/// Observable provider attachment phases. These never enter durable host state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentPhase {
    Pending,
    Running,
    Completed,
    Failed,
}

/// The exact private paths and tmux session owned by one navigator client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationPaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
    pub attachment_status: PathBuf,
    pub session_name: String,
}

/// The result of the read-only legacy presentation classifier.
///
/// The launcher uses this value to decide whether it may present the D16
/// confirmation or must first offer a drain-only attachment.  In particular,
/// the classifier never adopts, closes, removes, or otherwise mutates the
/// presentation it describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyPresentationState {
    /// No presentation directory exists below the selected state root.
    None,
    /// One exact, live, detached, two-pane presentation was proven.
    DetachedOrdinary,
    /// One exact presentation has one or more attached clients.
    Attached,
    /// One exact presentation has a live utility shell pane.
    UtilityShell,
    /// One exact presentation has a provider pane running the exact native
    /// observer-review helper command.
    ObserverReview,
    /// Exact private artifacts remain but their tmux session or navigator is
    /// no longer live.  This state is still owned evidence and is never
    /// removed by classification.
    DeadOwned,
    /// The selected presentation contains malformed topology or unknown
    /// private-directory entries.
    Malformed,
    /// The evidence belongs to another owner, executable, state root, or
    /// presentation session.
    Foreign,
    /// The evidence could not be read safely (for example, permissions or a
    /// process table that cannot be inspected).
    Inaccessible,
}

impl LegacyPresentationState {
    const fn into_probe(self) -> LegacyProbeFailure {
        match self {
            Self::Inaccessible => LegacyProbeFailure::Inaccessible,
            Self::Foreign => LegacyProbeFailure::Foreign,
            _ => LegacyProbeFailure::Malformed,
        }
    }
}

/// A read-only classification of the selected state's presentation directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPresentationAssessment {
    state: LegacyPresentationState,
    proof: Option<LegacyPresentationProof>,
}

impl LegacyPresentationAssessment {
    #[must_use]
    pub const fn state(&self) -> LegacyPresentationState {
        self.state
    }

    /// Returns the exact proof only for owned evidence.  Malformed, foreign,
    /// and inaccessible outcomes intentionally carry no mutation authority.
    #[must_use]
    pub const fn proof(&self) -> Option<&LegacyPresentationProof> {
        self.proof.as_ref()
    }

    const fn none() -> Self {
        Self {
            state: LegacyPresentationState::None,
            proof: None,
        }
    }

    const fn classified(state: LegacyPresentationState) -> Self {
        Self { state, proof: None }
    }
}

/// All identity evidence needed to repeat a legacy presentation comparison
/// under a D16 transition lease.  The type contains no terminal bytes,
/// provider output, or process-control capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPresentationProof {
    directory: PathBuf,
    directory_identity: LegacyFileIdentity,
    socket: PathBuf,
    socket_identity: Option<LegacyFileIdentity>,
    config_identity: LegacyFileIdentity,
    attachment_identity: Option<LegacyFileIdentity>,
    session_name: String,
    session_id: Option<String>,
    window_id: Option<String>,
    navigator: Option<LegacyPaneProof>,
    provider: Option<LegacyPaneProof>,
    utility: Option<LegacyPaneProof>,
    clients: Vec<LegacyClientProof>,
    shell_claim_present: bool,
    legacy_executable: Option<LegacyExecutableProof>,
    attachment_status: Option<LegacyAttachmentStatus>,
}

impl LegacyPresentationProof {
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    #[must_use]
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    #[must_use]
    pub fn attached_client_count(&self) -> usize {
        self.clients.len()
    }

    #[must_use]
    pub fn navigator_pid(&self) -> Option<u32> {
        self.navigator
            .as_ref()
            .and_then(|pane| pane.process.as_ref())
            .map(|process| process.pid)
    }

    #[must_use]
    pub fn navigator_process_birth(&self) -> Option<u64> {
        self.navigator
            .as_ref()
            .and_then(|pane| pane.process.as_ref())
            .map(|process| process.birth)
    }

    /// Returns the legacy controller executable identity established by the
    /// exact navigator process.  This is intentionally independent from the
    /// executable currently running the D16 launcher.
    #[must_use]
    pub fn legacy_executable_identity(&self) -> Option<LegacyFileIdentity> {
        self.legacy_executable
            .as_ref()
            .map(|executable| executable.identity)
    }

    #[must_use]
    pub fn legacy_executable_path(&self) -> Option<&Path> {
        self.legacy_executable
            .as_ref()
            .map(|executable| executable.path.as_path())
    }

    /// Returns whether the exact navigator and provider controller processes
    /// were both proven from one stable legacy executable identity.
    #[must_use]
    pub fn controller_proven(&self) -> bool {
        self.navigator.is_some() && self.provider.is_some() && self.legacy_executable.is_some()
    }

    #[must_use]
    pub fn utility_present(&self) -> bool {
        self.utility.is_some()
    }

    #[must_use]
    pub fn observer_review_present(&self) -> bool {
        self.provider
            .as_ref()
            .is_some_and(|pane| pane.command == LegacyPaneCommand::ObserverReview)
    }

    #[must_use]
    pub fn shell_claim_present(&self) -> bool {
        self.shell_claim_present
    }
}

/// Stable identity for one owned regular file or private socket.  Device and
/// inode are populated on Unix; size/mode remain useful on other platforms.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegacyFileIdentity {
    pub size: u64,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
    pub digest: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LegacyRetirementMarker {
    version: u8,
    directory: PathBuf,
    directory_identity: LegacyFileIdentity,
    socket: PathBuf,
    session_name: String,
    config_identity: LegacyFileIdentity,
    socket_identity: Option<LegacyFileIdentity>,
    attachment_identity: Option<LegacyFileIdentity>,
}

/// Exact ownership evidence written before a private presentation server is
/// started. A path-shaped directory is never enough authority for normal
/// presentation cleanup: the marker, its private artifact identities, and the
/// bounded directory allowlist must all still match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PresentationOwnershipMarker {
    version: u8,
    directory: PathBuf,
    socket: PathBuf,
    session_name: String,
    directory_identity: LegacyFileIdentity,
    config_identity: LegacyFileIdentity,
    socket_identity: Option<LegacyFileIdentity>,
    /// Omitted from ordinary D16 markers so their serialized shape remains
    /// unchanged. D17 writes this only at its own atomic cutover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    d17: Option<D17PresentationMarker>,
}

/// The bounded D17 fields carried by the existing presentation-ownership
/// marker. Keeping this embedded prevents a second loose artifact from being
/// mistaken for presentation authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct D17PresentationMarker {
    version: u8,
    presentation_id: uuid::Uuid,
    presentation_revision: Revision,
    seed_cwd: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresentationOwnershipProof {
    marker: PresentationOwnershipMarker,
    marker_identity: LegacyFileIdentity,
    socket_identity: Option<LegacyFileIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyExecutableProof {
    path: PathBuf,
    identity: LegacyFileIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyProcessProof {
    pid: u32,
    birth: u64,
    executable: LegacyExecutableProof,
    arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyPaneProof {
    id: String,
    role: PresentationPaneRole,
    dead: bool,
    process: Option<LegacyProcessProof>,
    command: LegacyPaneCommand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyPaneCommand {
    Navigator,
    ProviderWait,
    ProviderAttach,
    ObserverReview,
    PresentationShell,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyClientProof {
    name: String,
    window_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyPaneEvidence {
    pane: LegacyOwnedPane,
    pid: Option<u32>,
    current_command: String,
    start_command: String,
    process: Option<LegacyProcessProof>,
    process_stable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyOwnedPane {
    id: String,
    role: PresentationPaneRole,
    host_alias: Option<String>,
    workstream_id: Option<WorkstreamId>,
    dead: bool,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyPresentationEvidence {
    directory: LegacyFileIdentity,
    socket: Option<LegacyFileIdentity>,
    config: LegacyFileIdentity,
    attachment: Option<LegacyFileIdentity>,
    attachment_status: Option<LegacyAttachmentStatus>,
    session_id: Option<String>,
    window_id: Option<String>,
    panes: Vec<LegacyPaneEvidence>,
    clients: Vec<LegacyClientProof>,
    shell_claim_present: bool,
}

impl PresentationPaths {
    /// Creates a collision-resistant private presentation location below one
    /// state root. The presentation has no durable identity or focus record.
    #[must_use]
    pub fn fresh(state_root: &Path) -> Self {
        let full_identifier = uuid::Uuid::new_v4().simple().to_string();
        let identifier = &full_identifier[..12];
        let directory = state_root
            .join(PRESENTATION_DIRECTORY)
            .join(format!("presentation-{identifier}"));
        Self {
            socket: directory.join("tmux.sock"),
            config: directory.join("tmux.conf"),
            attachment_status: directory.join(ATTACHMENT_STATUS_FILE),
            session_name: format!("{PRESENTATION_PREFIX}{identifier}"),
            directory,
        }
    }

    /// Validates that an internal navigator process can control only a
    /// presentation beneath the supplied state root.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket or session does not describe an exact
    /// private Workstream Navigator presentation.
    pub fn from_control(
        state_root: &Path,
        socket: PathBuf,
        session_name: String,
    ) -> Result<Self, PresentationError> {
        let parent = socket
            .parent()
            .ok_or_else(|| PresentationError::InvalidControlPath(socket.clone()))?;
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        let expected_session = presentation_session_name(parent);
        if parent.parent() != Some(presentation_root.as_path())
            || socket.file_name().is_none_or(|name| name != "tmux.sock")
            || expected_session.as_deref() != Some(&session_name)
        {
            return Err(PresentationError::InvalidControlPath(socket));
        }
        Ok(Self {
            config: parent.join("tmux.conf"),
            attachment_status: parent.join(ATTACHMENT_STATUS_FILE),
            directory: parent.to_path_buf(),
            socket,
            session_name,
        })
    }
}

/// Owns one disposable two-pane local presentation server.
#[derive(Clone, Debug)]
pub struct Presentation {
    paths: PresentationPaths,
    executable: PathBuf,
    state_root: PathBuf,
}

impl Presentation {
    /// Creates an unstarted presentation owner for the current executable.
    ///
    /// # Errors
    ///
    /// Returns an error when the current executable cannot be resolved.
    pub fn fresh(state_root: &Path) -> Result<Self, PresentationError> {
        let executable = std::env::current_exe().map_err(PresentationError::Io)?;
        Ok(Self::fresh_with_executable(state_root, executable))
    }

    /// Creates an owner with an explicitly fixed executable. This is used by
    /// disposable integration fixtures so a test harness can exercise the
    /// real hidden helper instead of becoming the helper itself.
    #[doc(hidden)]
    #[must_use]
    pub fn fresh_with_executable(state_root: &Path, executable: PathBuf) -> Self {
        Self {
            paths: PresentationPaths::fresh(state_root),
            executable,
            state_root: state_root.to_path_buf(),
        }
    }

    /// Reuses the one live owned presentation, or creates a fresh owner when
    /// no presentation is live. A detached presentation is intentionally kept
    /// so a later `wsnav` invocation can reconnect without disturbing any
    /// provider Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when an owned presentation is ambiguous, malformed, or
    /// cannot be queried through its exact private tmux socket.
    pub fn open_or_create(state_root: &Path) -> Result<(Self, bool), PresentationError> {
        let live = Self::discover_live(state_root)?;
        match live.as_slice() {
            [] => Ok((Self::fresh(state_root)?, true)),
            [presentation] => Ok((presentation.clone(), false)),
            _ => Err(PresentationError::AmbiguousPresentations),
        }
    }

    /// Reopens the exact owned presentation described by a hidden child
    /// command. This does not discover or use any ordinary tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the executable cannot be resolved or the supplied
    /// control values do not name an owned private presentation.
    pub fn from_control(
        state_root: &Path,
        socket: PathBuf,
        session_name: String,
    ) -> Result<Self, PresentationError> {
        Ok(Self {
            paths: PresentationPaths::from_control(state_root, socket, session_name)?,
            executable: std::env::current_exe().map_err(PresentationError::Io)?,
            state_root: state_root.to_path_buf(),
        })
    }

    #[must_use]
    pub fn paths(&self) -> &PresentationPaths {
        &self.paths
    }

    /// Captures the exact seed cwd for a fresh D17 presentation. The context
    /// is embedded in the already-proven ownership marker, so later D17
    /// materialization can bind its provisional slot without deriving identity
    /// from a directory name or a provider process.
    ///
    /// This dormant seam neither creates a provisional server nor opens host
    /// state. The atomic D17 cutover is its only future caller.
    #[allow(
        dead_code,
        reason = "the D17 presentation context remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn initialize_d17_context(
        &self,
        presentation_id: uuid::Uuid,
        seed_cwd: &Path,
    ) -> Result<D17PresentationContext, PresentationError> {
        let seed_cwd = canonical_d17_seed_cwd(seed_cwd)?;
        if presentation_id.is_nil() {
            return Err(PresentationError::D17ContextInvalid);
        }
        let mut ownership = read_presentation_ownership(&self.paths)?
            .ok_or(PresentationError::D17ContextUnavailable)?;
        if ownership.marker.d17.is_some() {
            return Err(PresentationError::D17ContextAlreadyInitialized);
        }
        let marker = D17PresentationMarker {
            version: D17_PRESENTATION_CONTEXT_VERSION,
            presentation_id,
            presentation_revision: Revision::INITIAL,
            seed_cwd,
        };
        let context = d17_context_from_marker(&marker)?;
        ownership.marker.d17 = Some(marker);
        write_presentation_ownership_marker(
            &self.paths,
            &ownership.marker,
            Some(&ownership.marker_identity),
        )?;
        Ok(context)
    }

    /// Reopens the bounded D17 context from the exact current presentation
    /// marker. It exposes no terminal data, provider input, or registry path.
    #[allow(
        dead_code,
        reason = "the D17 presentation context remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn d17_context(&self) -> Result<D17PresentationContext, PresentationError> {
        let ownership = read_d17_presentation_ownership(&self.paths)?
            .ok_or(PresentationError::D17ContextUnavailable)?;
        let marker = ownership
            .marker
            .d17
            .as_ref()
            .ok_or(PresentationError::D17ContextUnavailable)?;
        d17_context_from_marker(marker)
    }

    /// Reopens the D17 context only from an exact owned presentation directory
    /// beneath this state root. The inherited shell path is discovery input,
    /// not authority: this repeats the private ownership-marker proof before
    /// a shell gate may open schema-14 state.
    #[allow(
        dead_code,
        reason = "the D17 presentation context remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn d17_context_from_directory(
        state_root: &Path,
        presentation_directory: &Path,
    ) -> Result<D17PresentationContext, PresentationError> {
        let state_metadata = fs::symlink_metadata(state_root)
            .map_err(|_| PresentationError::D17ContextUnavailable)?;
        if state_metadata.file_type().is_symlink() || !state_metadata.is_dir() {
            return Err(PresentationError::D17ContextUnavailable);
        }
        let state_root =
            fs::canonicalize(state_root).map_err(|_| PresentationError::D17ContextUnavailable)?;
        if !state_root.is_dir() {
            return Err(PresentationError::D17ContextUnavailable);
        }
        let original = fs::symlink_metadata(presentation_directory)
            .map_err(|_| PresentationError::D17ContextUnavailable)?;
        if original.file_type().is_symlink() || !original.is_dir() {
            return Err(PresentationError::D17ContextUnavailable);
        }
        let presentation_directory = fs::canonicalize(presentation_directory)
            .map_err(|_| PresentationError::D17ContextUnavailable)?;
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        if presentation_directory.parent() != Some(presentation_root.as_path()) {
            return Err(PresentationError::D17ContextUnavailable);
        }
        let session_name = presentation_session_name(&presentation_directory)
            .ok_or(PresentationError::D17ContextUnavailable)?;
        let paths = PresentationPaths {
            socket: presentation_directory.join("tmux.sock"),
            config: presentation_directory.join("tmux.conf"),
            attachment_status: presentation_directory.join(ATTACHMENT_STATUS_FILE),
            session_name,
            directory: presentation_directory,
        };
        let ownership = read_d17_presentation_ownership(&paths)?
            .ok_or(PresentationError::D17ContextUnavailable)?;
        let marker = ownership
            .marker
            .d17
            .as_ref()
            .ok_or(PresentationError::D17ContextUnavailable)?;
        d17_context_from_marker(marker)
    }

    /// Creates exactly one private tmux server with a navigator pane and a
    /// blank provider-attachment pane. Neither command invokes a shell.
    ///
    /// # Errors
    ///
    /// Returns an error when the owned paths cannot be created or tmux rejects
    /// the private presentation setup.
    pub fn start(&self) -> Result<(), PresentationError> {
        let _ = self.start_with_d17_context(None)?;
        Ok(())
    }

    /// Starts a fresh private presentation with its D17 seed context written
    /// before the navigator pane can run. This ordering prevents the pane from
    /// deriving a seed or identity after its process has already started.
    #[allow(
        dead_code,
        reason = "the D17 presentation context remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn start_d17(
        &self,
        presentation_id: uuid::Uuid,
        seed_cwd: &Path,
    ) -> Result<D17PresentationContext, PresentationError> {
        self.start_with_d17_context(Some((presentation_id, seed_cwd)))?
            .ok_or(PresentationError::D17ContextUnavailable)
    }

    fn start_with_d17_context(
        &self,
        d17: Option<(uuid::Uuid, &Path)>,
    ) -> Result<Option<D17PresentationContext>, PresentationError> {
        create_paths(&self.paths)?;
        let is_d17 = d17.is_some();
        let context = d17
            .map(|(presentation_id, seed_cwd)| {
                self.complete_start_stage(
                    "D17 presentation context capture",
                    self.initialize_d17_context(presentation_id, seed_cwd),
                )
            })
            .transpose()?;
        let mut arguments = vec![
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            self.paths.session_name.clone().into(),
            "-n".into(),
            NAVIGATOR_WINDOW.into(),
        ];
        let navigator_command = if is_d17 {
            self.d17_navigator_command()
        } else {
            self.navigator_command()
        };
        arguments.extend(navigator_command);
        let result = self.invoke(Some(&self.paths.config), arguments);
        self.complete_start_stage("server creation", result)?;
        let result = self.capture_ownership_socket_identity();
        self.complete_start_stage("socket ownership capture", result)?;
        let result = self
            .set_pane_role(NAVIGATOR_PANE, PresentationPaneRole::Navigator, None)
            .and_then(|()| self.set_pane_remain_on_exit(NAVIGATOR_PANE, true));
        self.complete_start_stage("navigator pane setup", result)?;
        let wait = self.provider_wait_command();
        let result = self.invoke(
            None,
            vec![
                "split-window".into(),
                "-h".into(),
                "-d".into(),
                "-t".into(),
                format!("{}:0.0", self.paths.session_name).into(),
                "-l".into(),
                PREFERRED_PROVIDER_PANE_WIDTH.to_string().into(),
                wait[0].clone(),
                wait[1].clone(),
                wait[2].clone(),
                wait[3].clone(),
            ],
        );
        self.complete_start_stage("provider pane creation", result)?;
        let result = self
            .set_pane_role(PROVIDER_PANE, PresentationPaneRole::Provider, None)
            .and_then(|()| self.set_pane_remain_on_exit(PROVIDER_PANE, true))
            .and_then(|()| self.install_control_bindings());
        self.complete_start_stage("provider pane setup", result)?;
        let result = self.set_default_navigator_width();
        self.complete_start_stage("default navigator width", result)?;
        let result = self.install_navigator_width_hooks();
        self.complete_start_stage("navigator width hooks", result)?;
        Ok(context)
    }

    fn complete_start_stage<T>(
        &self,
        stage: &'static str,
        result: Result<T, PresentationError>,
    ) -> Result<T, PresentationError> {
        result.map_err(|source| {
            let _ = self.close();
            PresentationError::StartupFailed {
                stage,
                source: Box::new(source),
            }
        })
    }

    /// Directly attaches the caller's terminal to this private presentation.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux cannot attach to this exact private server.
    pub fn attach(&self) -> Result<(), PresentationError> {
        self.prepare_attach()?;
        let status = private_tmux_command()
            .arg("-S")
            .arg(&self.paths.socket)
            .args(["attach-session", "-t", &self.paths.session_name])
            .status()
            .map_err(PresentationError::Io)?;
        if stopped_owned_presentation(self.is_live()?) {
            self.close()?;
            return Ok(());
        }
        if status.success() {
            for _ in 0..NAVIGATOR_STOP_ATTEMPTS {
                if self.navigator_pane_is_dead()? {
                    self.close()?;
                    return Ok(());
                }
                thread::sleep(NAVIGATOR_STOP_RETRY);
            }
            return Ok(());
        }
        if self.navigator_pane_is_dead()? {
            self.close()?;
            return Ok(());
        }
        Err(PresentationError::TmuxRejected(
            "presentation attach failed".to_owned(),
        ))
    }

    fn prepare_attach(&self) -> Result<(), PresentationError> {
        let (columns, rows) = crossterm::terminal::size()
            .map_err(|_| PresentationError::TerminalGeometryUnavailable)?;
        self.prepare_attach_with_size(columns, rows)
    }

    fn prepare_attach_with_size(&self, columns: u16, rows: u16) -> Result<(), PresentationError> {
        prepare_attach_window_with_size(&self.paths.session_name, columns, rows, |arguments| {
            self.invoke(None, arguments)
        })
    }

    /// Replaces only the outer provider attachment helper. The managed Codex
    /// runtime remains in its own private tmux server.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux rejects replacement of the exact owned pane.
    pub fn attach_workstream(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<AttachmentStatus, PresentationError> {
        self.with_attachment_claim(|| {
            let status = self.prepare_attachment(workstream_id)?;
            let result = (|| {
                self.retire_utility_for_attachment(workstream_id)?;
                let provider = self.provider_target_for_attachment()?;
                self.set_pane_role(
                    &provider,
                    PresentationPaneRole::Provider,
                    Some(status.workstream_id),
                )?;
                self.invoke(
                    None,
                    self.provider_respawn_arguments(&provider, workstream_id, status.attempt_id),
                )
            })();
            self.finish_attachment_start(status, result)
        })
    }

    /// Replaces only the outer provider pane with the exact private tmux
    /// client for a materialized D17 account shell. The candidate remains
    /// unregistered: this does not create a Workstream, Runtime, attachment
    /// record, or provider effect.
    ///
    /// The caller retains the schema-14 provisional lease through this
    /// transition. The marker, lease, and D17 presentation context are
    /// revalidated immediately before the outer pane changes, so a stale or
    /// foreign candidate can never be attached merely because its paths look
    /// like a Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact D17 marker/lease/context does not
    /// authorize this shell, or when the owned provider pane cannot be
    /// replaced.
    #[allow(
        dead_code,
        reason = "the D17 provisional-shell attachment remains unreachable until the atomic Navigator cutover"
    )]
    pub(crate) fn attach_d17_provisional_shell(
        &self,
        state: &D16State,
        provisional_lease: &ProvisionalLease,
        slot: &ProvisionalSlot,
    ) -> Result<(), PresentationError> {
        self.with_attachment_claim(|| {
            self.validate_d17_provisional_attachment(state, provisional_lease, slot)?;
            self.retire_utility_for_observer_review()?;
            let provider = self.provider_target_for_attachment()?;
            self.set_pane_role(&provider, PresentationPaneRole::Provider, None)?;
            self.invoke(
                None,
                self.provider_respawn_for_command(
                    &provider,
                    Self::d17_provisional_attach_command(slot.runtime_paths()),
                ),
            )?;
            // The provider pane has changed, but no D17 state did. Recheck
            // the held lease before returning so the controller never treats
            // a changed lock as successful shell authority.
            provisional_lease
                .revalidate_for_mutation(state.root())
                .map_err(|_| {
                    PresentationError::ControlRefused(
                        "D17 provisional shell attachment is unavailable",
                    )
                })?;
            self.observer_review_provider_target()?;
            Ok(())
        })
    }

    /// Retires the exact utility pane before a different Workstream can
    /// replace the provider attachment. A shell tagged for the requested
    /// Workstream is retained so same-Workstream reconnects preserve its
    /// launch context.
    ///
    /// This deliberately performs no provider mutation. The topology is
    /// validated before the exact utility pane is killed and again after the
    /// kill, so an ambiguous or unconfirmed cleanup refuses the attachment
    /// before its provider pane is retagged or respawned.
    ///
    /// # Errors
    ///
    /// Returns an error when the owned presentation topology is ambiguous, an
    /// exact utility cleanup is rejected, or the resulting two-pane geometry
    /// cannot be proven.
    fn retire_utility_for_attachment(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<(), PresentationError> {
        self.validate_single_presentation_window()?;
        let topology = self.attachment_topology()?;
        let Some(utility) = topology.utility() else {
            return Ok(());
        };
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        let provider_matches =
            provider.workstream_id.is_none() || provider.workstream_id == Some(workstream_id);
        let utility_matches = utility.workstream_id == Some(workstream_id);
        if !utility.dead && provider_matches && utility_matches {
            return Ok(());
        }

        let utility_id = utility.id.clone();
        self.kill_exact_pane(&utility_id)?;
        self.validate_single_presentation_window()?;
        let topology = self.attachment_topology()?;
        if topology.utility().is_some() {
            return Err(PresentationError::ControlRefused(
                "utility shell cleanup could not be proven",
            ));
        }
        Ok(())
    }

    /// Retires every exact utility pane before the provider is replaced by
    /// observer review. Unlike Workstream attachment, observer review has no
    /// Workstream context that could authorize retaining an existing shell.
    /// The same presentation-wide claim is held by the caller through this
    /// check, kill, and post-respawn topology validation.
    fn retire_utility_for_observer_review(&self) -> Result<(), PresentationError> {
        self.validate_single_presentation_window()?;
        let topology = self.read_topology()?;
        topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        let Some(utility) = topology.utility() else {
            return Ok(());
        };
        let utility_id = utility.id.clone();
        self.kill_exact_pane(&utility_id)?;
        self.validate_single_presentation_window()?;
        let topology = self.read_topology()?;
        if topology.utility().is_some() {
            return Err(PresentationError::ControlRefused(
                "utility shell cleanup could not be proven before observer review",
            ));
        }
        validate_observer_review_topology(&topology).map(|_| ())
    }

    fn observer_review_provider_target(&self) -> Result<String, PresentationError> {
        self.validate_single_presentation_window()?;
        let topology = self.read_topology()?;
        validate_observer_review_topology(&topology)
    }

    /// Replaces the blank provider pane with the local temporary native Codex
    /// observer-review surface. This is not a Workstream attachment and never
    /// records provider output in presentation state. The presentation-wide
    /// claim is held while any utility pane is retired and through the final
    /// two-pane topology check.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned presentation pane cannot be
    /// replaced.
    pub fn start_observer_review(&self) -> Result<(), PresentationError> {
        self.with_attachment_claim(|| {
            self.retire_utility_for_observer_review()?;
            let provider = self.observer_review_provider_target()?;
            self.clear_pane_context(&provider)?;
            self.invoke(
                None,
                self.provider_respawn_for_command(&provider, self.observer_review_command()),
            )?;
            // The presentation-wide claim prevents WSNav's own shell action
            // from splitting concurrently. Re-read the exact topology after
            // respawn as a final guard against an external/stale split.
            self.observer_review_provider_target()?;
            Ok(())
        })
    }

    fn provider_respawn_arguments(
        &self,
        provider: &str,
        workstream_id: WorkstreamId,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        let command = self.provider_attach_command(workstream_id, attempt_id);
        self.provider_respawn_for_command(provider, command)
    }

    fn provider_respawn_for_command(
        &self,
        provider: &str,
        command: Vec<OsString>,
    ) -> Vec<OsString> {
        let mut arguments = vec![
            "respawn-pane".into(),
            "-k".into(),
            "-t".into(),
            self.pane_target(provider).into(),
        ];
        arguments.extend(command);
        arguments
    }

    /// The provisional attach command is deliberately direct argv: no shell,
    /// provider command, or user-derived string crosses into the outer pane.
    /// `env -u TMUX` prevents tmux's nested-server warning path from changing
    /// an attachment to the exact private Runtime socket.
    fn d17_provisional_attach_command(paths: &RuntimePaths) -> Vec<OsString> {
        vec![
            "env".into(),
            "-u".into(),
            "TMUX".into(),
            "tmux".into(),
            "-u".into(),
            "-S".into(),
            paths.socket.clone().into_os_string(),
            "attach-session".into(),
            "-t".into(),
            paths.session_name.clone().into(),
        ]
    }

    /// Gives keyboard focus to the directly interactive provider pane.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned pane cannot be focused.
    pub fn focus_provider(&self) -> Result<(), PresentationError> {
        let provider = self.provider_target()?;
        self.select_owned_pane(&provider)
    }

    /// Gives keyboard focus to the navigator pane without touching a provider
    /// Runtime or its attachment helper.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact owned pane cannot be focused.
    pub fn focus_navigator(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.select_owned_pane(&navigator)
    }

    /// Returns the exact owned role for a pane supplied by tmux's format
    /// expansion. No positional pane index is accepted at this boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the private pane topology is missing, dead, or
    /// ambiguous, or when the source pane is not an exact owned pane.
    pub fn focused_pane_role(
        &self,
        source_pane: &str,
    ) -> Result<PresentationPaneRole, PresentationError> {
        let topology = self.read_topology()?;
        topology
            .pane(source_pane)
            .map(|pane| pane.role)
            .ok_or(PresentationError::InvalidTopology)
    }

    /// Validates that the provider role still names the exact local
    /// attachment represented by the ephemeral status row. This is called
    /// before any shell split or provider literal input.
    ///
    /// # Errors
    ///
    /// Returns an error when the private topology is ambiguous or the tagged
    /// provider context does not exactly match the supplied attachment.
    pub fn validate_provider_context(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let provider = topology
            .provider()
            .ok_or(PresentationError::InvalidTopology)?;
        if provider.workstream_id != Some(workstream_id) {
            return Err(PresentationError::InvalidTopology);
        }
        Ok(())
    }

    /// Focuses the exact utility pane if one is already present. This check
    /// intentionally precedes attachment preflight so a shell keeps its
    /// original launch context when the provider selection later changes.
    ///
    /// # Errors
    ///
    /// Returns an error when the source or topology is ambiguous, or when an
    /// existing utility pane cannot be focused.
    pub fn focus_existing_utility_if_present(
        &self,
        source_pane: &str,
    ) -> Result<bool, PresentationError> {
        let topology = match self.read_topology() {
            Ok(topology) => topology,
            Err(PresentationError::InvalidTopology) if self.shell_claim_present()? => {
                // A competing helper may have the one bounded claim while its
                // new pane is between split and role tagging. Let the caller
                // perform authoritative preflight and enter the same bounded
                // create/focus retry loop instead of treating that transient
                // evidence as a foreign topology.
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        let Some(utility) = topology.utility() else {
            return Ok(false);
        };
        self.select_owned_pane(&utility.id)?;
        Ok(true)
    }

    /// Arms one exact newly-created utility pane before its shell barrier
    /// replaces itself. The pane must already belong to this private
    /// presentation window; no positional pane target is accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when the pane identity is malformed, belongs to a
    /// different session/window, or cannot be switched to non-retaining mode.
    pub fn prepare_utility_pane(&self, pane: &str) -> Result<(), PresentationError> {
        let pane = parse_pane_id(pane).ok_or(PresentationError::InvalidTopology)?;
        let evidence = self.invoke_capture(
            None,
            vec![
                "display-message".into(),
                "-p".into(),
                "-t".into(),
                pane.clone().into(),
                "#{session_name}\t#{window_name}\t#{pane_id}".into(),
            ],
        )?;
        let expected = format!("{}\t{}\t{pane}", self.paths.session_name, NAVIGATOR_WINDOW);
        if evidence.trim() != expected {
            return Err(PresentationError::InvalidTopology);
        }
        self.set_pane_remain_on_exit(&pane, false)
    }

    /// Creates one local shell below the exact provider, or focuses the
    /// existing utility shell. The caller must complete authoritative state
    /// preflight before invoking this method.
    ///
    /// # Errors
    ///
    /// Returns an error when the shell path, project root, role topology, or
    /// bounded tmux mutation is not exact. A shell that exits before tagging
    /// is treated as normal cleanup.
    pub fn create_or_focus_shell(
        &self,
        source_pane: &str,
        workstream_id: WorkstreamId,
        cwd: &Path,
        shell: &Path,
    ) -> Result<(), PresentationError> {
        validate_shell_path(shell)?;
        if !cwd.is_dir() {
            return Err(PresentationError::ControlRefused(
                "registered project root is unavailable",
            ));
        }

        let shell_command = vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_presentation_shell".into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--shell".into(),
            shell.to_path_buf().into_os_string(),
            "--cwd".into(),
            cwd.to_path_buf().into_os_string(),
        ];
        self.create_or_focus_shell_command(source_pane, workstream_id, &shell_command)
    }

    fn create_or_focus_shell_command(
        &self,
        source_pane: &str,
        workstream_id: WorkstreamId,
        shell_command: &[OsString],
    ) -> Result<(), PresentationError> {
        for _ in 0..SHELL_CLAIM_ATTEMPTS {
            let topology = match self.read_topology() {
                Ok(topology) => topology,
                Err(PresentationError::InvalidTopology) if self.shell_claim_present()? => {
                    thread::sleep(SHELL_CLAIM_RETRY);
                    continue;
                }
                Err(error) => return Err(error),
            };
            topology
                .pane(source_pane)
                .ok_or(PresentationError::InvalidTopology)?;
            if let Some(utility) = topology.utility() {
                self.select_owned_pane(&utility.id)?;
                return Ok(());
            }
            let provider = topology
                .provider()
                .ok_or(PresentationError::InvalidTopology)?;
            if provider.workstream_id != Some(workstream_id) {
                return Err(PresentationError::InvalidTopology);
            }

            let token = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
            if !self.try_shell_claim(&token)? {
                thread::sleep(SHELL_CLAIM_RETRY);
                continue;
            }
            let result = self.create_shell_after_claim(
                &topology,
                workstream_id,
                provider.id.as_str(),
                shell_command,
            );
            self.release_shell_claim(&token);
            return result;
        }
        Err(PresentationError::ControlRefused(
            "another shell action is in progress",
        ))
    }

    fn create_shell_after_claim(
        &self,
        topology: &PresentationTopology,
        workstream_id: WorkstreamId,
        provider: &str,
        shell_command: &[OsString],
    ) -> Result<(), PresentationError> {
        let mut split_arguments = vec![
            "split-window".into(),
            "-v".into(),
            "-P".into(),
            "-F".into(),
            "#{pane_id}".into(),
            "-t".into(),
            provider.into(),
        ];
        split_arguments.extend(shell_command.iter().cloned());
        let output = self.invoke_capture(None, split_arguments)?;
        let Some(utility_id) = parse_pane_id(output.trim()) else {
            return Err(PresentationError::InvalidTopology);
        };
        if topology.pane(&utility_id).is_some() {
            return Err(PresentationError::InvalidTopology);
        }
        let setup = (|| {
            self.set_pane_remain_on_exit(&utility_id, false)?;
            self.set_pane_role(
                &utility_id,
                PresentationPaneRole::Utility,
                Some(workstream_id),
            )?;
            self.select_owned_pane(&utility_id)?;
            if self.pane_is_dead(&utility_id)? {
                self.kill_exact_pane(&utility_id)?;
            }
            Ok(())
        })();
        match setup {
            Ok(()) => Ok(()),
            Err(error) => {
                let cleanup = self.kill_exact_pane(&utility_id);
                let restored = cleanup.is_ok()
                    && self.read_topology().is_ok_and(|current| {
                        base_topology_preserved(topology, &current, &utility_id)
                    });
                if restored
                    && (pane_disappeared(&error)
                        || matches!(error, PresentationError::InvalidTopology))
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }

    /// Runs a bounded presentation-only action. Provider literal input is
    /// deliberately excluded: the app layer must first preflight the exact
    /// Runtime and use its private tmux socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the action's source pane or owned role topology
    /// is ambiguous, or when the exact private tmux action is rejected.
    pub fn control(
        &self,
        action: PresentationAction,
        source_pane: &str,
    ) -> Result<(), PresentationError> {
        self.control_with_client(action, source_pane, None)
    }

    /// Runs one presentation action with the exact invoking tmux client when
    /// the action needs a client-scoped prompt.  The client identity is
    /// intentionally optional for callers that cannot originate a tmux key
    /// binding (for example, deterministic unit fixtures); utility close
    /// refuses that path instead of guessing a client.
    ///
    /// # Errors
    ///
    /// Returns an error when the action's source pane, client, or owned role
    /// topology is ambiguous, or when the exact private tmux action is
    /// rejected.
    pub fn control_with_client(
        &self,
        action: PresentationAction,
        source_pane: &str,
        client_name: Option<&str>,
    ) -> Result<(), PresentationError> {
        match action {
            PresentationAction::SuppressSplit => {
                self.focused_pane_role(source_pane)?;
                self.show_guidance("Use Ctrl+b \" for the utility shell")
            }
            PresentationAction::CloseShell => self.close_shell(source_pane, client_name),
            PresentationAction::FocusUp
            | PresentationAction::FocusDown
            | PresentationAction::FocusLeft
            | PresentationAction::FocusRight
            | PresentationAction::FocusNext => self.focus_direction(source_pane, action),
            PresentationAction::LiteralCtrlB => {
                let role = self.focused_pane_role(source_pane)?;
                if role == PresentationPaneRole::Provider {
                    return Err(PresentationError::ControlRefused(
                        "provider literal input requires Runtime preflight",
                    ));
                }
                self.send_outer_literal_c_b(source_pane)
            }
            PresentationAction::CreateOrFocusShell => Err(PresentationError::ControlRefused(
                "local shell requires attachment preflight",
            )),
        }
    }

    /// Sends one literal C-b through the outer presentation pane. Provider
    /// panes are rejected here so they cannot accidentally invoke the nested
    /// Runtime prefix table.
    ///
    /// # Errors
    ///
    /// Returns an error when the source pane is not an exact owned non-provider
    /// pane or the private tmux server rejects the literal input.
    pub fn send_outer_literal_c_b(&self, source_pane: &str) -> Result<(), PresentationError> {
        let role = self.focused_pane_role(source_pane)?;
        if role == PresentationPaneRole::Provider {
            return Err(PresentationError::ControlRefused(
                "provider literal input requires Runtime preflight",
            ));
        }
        self.invoke(
            None,
            vec![
                "send-keys".into(),
                "-t".into(),
                source_pane.into(),
                "C-b".into(),
            ],
        )
    }

    fn close_shell(
        &self,
        source_pane: &str,
        client_name: Option<&str>,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let source = topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        if source.role != PresentationPaneRole::Utility {
            return self.show_guidance("Ctrl+b x closes only the utility shell");
        }
        let client_name = client_name.ok_or(PresentationError::ControlRefused(
            "invoking presentation client is unavailable",
        ))?;
        self.validate_presentation_client(client_name)?;
        self.invoke(None, close_shell_arguments(client_name, &source.id))
    }

    fn validate_presentation_client(&self, client_name: &str) -> Result<(), PresentationError> {
        if client_name.is_empty()
            || client_name.len() > 256
            || client_name
                .chars()
                .any(|character| character.is_control() || character == '\t')
        {
            return Err(PresentationError::ControlRefused(
                "invoking presentation client is invalid",
            ));
        }
        let clients = self.invoke_capture(
            None,
            vec![
                "list-clients".into(),
                "-F".into(),
                "#{client_name}\t#{session_name}\t#{window_name}".into(),
            ],
        )?;
        if clients.lines().any(|line| {
            let mut fields = line.split('\t');
            fields.next() == Some(client_name)
                && fields.next() == Some(self.paths.session_name.as_str())
                && fields.next() == Some(NAVIGATOR_WINDOW)
                && fields.next().is_none()
        }) {
            Ok(())
        } else {
            Err(PresentationError::ControlRefused(
                "invoking client is not attached to this presentation",
            ))
        }
    }

    fn focus_direction(
        &self,
        source_pane: &str,
        action: PresentationAction,
    ) -> Result<(), PresentationError> {
        let topology = self.read_topology()?;
        let source = topology
            .pane(source_pane)
            .ok_or(PresentationError::InvalidTopology)?;
        let target = match action {
            PresentationAction::FocusNext => topology.next(source),
            PresentationAction::FocusUp => topology.directional(source, Direction::Up),
            PresentationAction::FocusDown => topology.directional(source, Direction::Down),
            PresentationAction::FocusLeft => topology.directional(source, Direction::Left),
            PresentationAction::FocusRight => topology.directional(source, Direction::Right),
            _ => None,
        };
        let Some(target) = target else {
            return self.show_guidance("No other owned pane in that direction");
        };
        self.select_owned_pane(&target.id)
    }

    fn select_owned_pane(&self, pane: &str) -> Result<(), PresentationError> {
        self.invoke(None, vec!["select-pane".into(), "-t".into(), pane.into()])
    }

    fn kill_exact_pane(&self, pane: &str) -> Result<(), PresentationError> {
        if parse_pane_id(pane).is_none() {
            return Err(PresentationError::InvalidTopology);
        }
        match self.invoke(None, vec!["kill-pane".into(), "-t".into(), pane.into()]) {
            Ok(()) => Ok(()),
            Err(error) if pane_disappeared(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Displays one bounded guidance message in the Navigator pane.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private presentation server rejects
    /// the bounded message action.
    pub fn show_guidance(&self, message: &str) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.invoke(
            None,
            vec![
                "display-message".into(),
                "-t".into(),
                navigator.into(),
                "-d".into(),
                "3000".into(),
                message.into(),
            ],
        )
    }

    fn read_topology(&self) -> Result<PresentationTopology, PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-panes".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_WINDOW}", self.paths.session_name).into(),
                "-F".into(),
                TOPOLOGY_FORMAT.into(),
            ],
        )?;
        parse_topology(&output)
    }

    fn validate_single_presentation_window(&self) -> Result<(), PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-windows".into(),
                "-t".into(),
                self.paths.session_name.clone().into(),
                "-F".into(),
                "#{window_name}\t#{window_id}".into(),
            ],
        )?;
        let mut windows = output.lines();
        let Some(window) = windows.next() else {
            return Err(PresentationError::InvalidTopology);
        };
        let mut fields = window.split('\t');
        if fields.next() != Some(NAVIGATOR_WINDOW)
            || !fields.next().is_some_and(parse_window_id)
            || fields.next().is_some()
            || windows.next().is_some()
        {
            return Err(PresentationError::InvalidTopology);
        }
        Ok(())
    }

    fn read_topology_allow_dead(&self) -> Result<PresentationTopology, PresentationError> {
        let output = self.invoke_capture(
            None,
            vec![
                "list-panes".into(),
                "-t".into(),
                format!("{}:{NAVIGATOR_WINDOW}", self.paths.session_name).into(),
                "-F".into(),
                TOPOLOGY_FORMAT.into(),
            ],
        )?;
        parse_topology_with_dead(&output, true)
    }

    fn set_pane_remain_on_exit(&self, pane: &str, enabled: bool) -> Result<(), PresentationError> {
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-t".into(),
                self.pane_target(pane).into(),
                "remain-on-exit".into(),
                if enabled { "on" } else { "off" }.into(),
            ],
        )
    }

    fn pane_is_dead(&self, pane: &str) -> Result<bool, PresentationError> {
        let value = self.invoke_capture(
            None,
            vec![
                "display-message".into(),
                "-p".into(),
                "-t".into(),
                self.pane_target(pane).into(),
                "#{pane_dead}".into(),
            ],
        )?;
        match value.trim() {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(PresentationError::InvalidTopology),
        }
    }

    fn try_shell_claim(&self, token: &str) -> Result<bool, PresentationError> {
        match self.invoke(
            None,
            vec![
                "set-option".into(),
                "-g".into(),
                "-o".into(),
                SHELL_CLAIM_OPTION.into(),
                token.into(),
            ],
        ) {
            Ok(()) => Ok(true),
            Err(PresentationError::TmuxRejected(message)) if message.contains("already set") => {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn shell_claim_present(&self) -> Result<bool, PresentationError> {
        let value = self.invoke_capture(
            None,
            vec![
                "show-options".into(),
                "-gqv".into(),
                SHELL_CLAIM_OPTION.into(),
            ],
        )?;
        Ok(!value.trim().is_empty())
    }

    fn release_shell_claim(&self, token: &str) {
        let current = self.invoke_capture(
            None,
            vec![
                "show-options".into(),
                "-gqv".into(),
                SHELL_CLAIM_OPTION.into(),
            ],
        );
        if current
            .ok()
            .as_deref()
            .is_some_and(|value| value.trim() == token)
        {
            let _ = self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-g".into(),
                    "-u".into(),
                    SHELL_CLAIM_OPTION.into(),
                ],
            );
        }
    }

    fn with_attachment_claim<T>(
        &self,
        operation: impl FnOnce() -> Result<T, PresentationError>,
    ) -> Result<T, PresentationError> {
        // The shell claim is presentation-global, so holding it through the
        // provider retag/respawn closes the race where a new utility split
        // could appear after retirement but before attachment replacement.
        let token = format!(
            "attachment-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        if !self.try_shell_claim(&token)? {
            return Err(PresentationError::ControlRefused(
                "another presentation shell or attachment action is in progress",
            ));
        }
        let result = operation();
        self.release_shell_claim(&token);
        result
    }

    fn set_pane_role(
        &self,
        pane: &str,
        role: PresentationPaneRole,
        context: Option<WorkstreamId>,
    ) -> Result<(), PresentationError> {
        let role_name = match role {
            PresentationPaneRole::Navigator => "navigator",
            PresentationPaneRole::Provider => "provider",
            PresentationPaneRole::Utility => "utility",
        };
        let target = self.pane_target(pane);
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-t".into(),
                target.clone().into(),
                ROLE_OPTION.into(),
                role_name.into(),
            ],
        )?;
        self.clear_pane_context(pane)?;
        if let Some(workstream_id) = context {
            self.invoke(
                None,
                vec![
                    "set-option".into(),
                    "-p".into(),
                    "-t".into(),
                    target.into(),
                    WORKSTREAM_OPTION.into(),
                    workstream_id.to_string().into(),
                ],
            )?;
        }
        Ok(())
    }

    fn clear_pane_context(&self, pane: &str) -> Result<(), PresentationError> {
        let target = self.pane_target(pane);
        self.invoke(
            None,
            vec![
                "set-option".into(),
                "-p".into(),
                "-u".into(),
                "-t".into(),
                target.into(),
                WORKSTREAM_OPTION.into(),
            ],
        )?;
        Ok(())
    }

    fn pane_target(&self, pane: &str) -> String {
        if pane.starts_with('%') {
            pane.to_owned()
        } else {
            format!("{}:{pane}", self.paths.session_name)
        }
    }

    fn navigator_target(&self) -> Result<String, PresentationError> {
        self.read_topology()?
            .navigator()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    fn provider_target(&self) -> Result<String, PresentationError> {
        self.read_topology()?
            .provider()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    /// Attachment replacement is the one active path that may accept an exact
    /// dead provider helper pane: tmux retains that owned pane specifically so
    /// `respawn-pane -k` can reconnect another live Runtime in place. A dead
    /// navigator remains a hard refusal, and all ordinary topology reads keep
    /// rejecting dead panes.
    fn attachment_topology(&self) -> Result<PresentationTopology, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        if topology.navigator().is_none_or(|pane| pane.dead) || topology.provider().is_none() {
            return Err(PresentationError::InvalidTopology);
        }
        Ok(topology)
    }

    fn provider_target_for_attachment(&self) -> Result<String, PresentationError> {
        self.attachment_topology()?
            .provider()
            .map(|pane| pane.id.clone())
            .ok_or(PresentationError::InvalidTopology)
    }

    fn validate_d17_provisional_attachment(
        &self,
        state: &D16State,
        provisional_lease: &ProvisionalLease,
        slot: &ProvisionalSlot,
    ) -> Result<(), PresentationError> {
        let unavailable =
            || PresentationError::ControlRefused("D17 provisional shell attachment is unavailable");
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| unavailable())?;
        if slot.phase() != ProvisionalPhase::Materialized
            || slot.lease_generation() != provisional_lease.lease_generation()
        {
            return Err(unavailable());
        }
        let context = Self::d17_context_from_directory(state.root(), &self.paths.directory)
            .map_err(|_| unavailable())?;
        if slot.presentation_id() != context.presentation_id()
            || slot.presentation_revision() != context.presentation_revision()
            || slot.seed_cwd() != context.seed_cwd()
        {
            return Err(unavailable());
        }
        if read_marker(state.root(), &self.paths.directory).map_err(|_| unavailable())? != *slot {
            return Err(unavailable());
        }
        provisional_lease
            .revalidate_for_mutation(state.root())
            .map_err(|_| unavailable())
    }

    fn install_control_bindings(&self) -> Result<(), PresentationError> {
        let bindings = [
            ("\"", PresentationAction::CreateOrFocusShell),
            ("%", PresentationAction::SuppressSplit),
            ("x", PresentationAction::CloseShell),
            ("o", PresentationAction::FocusNext),
            ("Up", PresentationAction::FocusUp),
            ("Down", PresentationAction::FocusDown),
            ("Left", PresentationAction::FocusLeft),
            ("Right", PresentationAction::FocusRight),
            ("C-b", PresentationAction::LiteralCtrlB),
        ];
        for (key, action) in bindings {
            // Deliberately omit `-b`: tmux waits for this fixed helper before
            // accepting another key action, which makes create/focus requests
            // serialize without a lock that could outlive a failed helper.
            self.invoke(
                None,
                vec![
                    "bind-key".into(),
                    "-T".into(),
                    "prefix".into(),
                    key.into(),
                    "run-shell".into(),
                    self.control_shell_command(action)?.into(),
                ],
            )?;
        }
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                "prefix".into(),
                "d".into(),
                "detach-client".into(),
            ],
        )?;
        self.invoke(
            None,
            vec![
                "bind-key".into(),
                "-T".into(),
                "prefix".into(),
                "?".into(),
                "display-message".into(),
                "Ctrl+b: \" shell | % blocked | x close shell | o/directions focus | d detach | Ctrl+b literal | ? help".into(),
            ],
        )
    }

    fn control_shell_command(
        &self,
        action: PresentationAction,
    ) -> Result<String, PresentationError> {
        let executable = shell_quote(self.executable.as_os_str())?;
        let state_root = shell_quote(self.state_root.as_os_str())?;
        let socket = shell_quote(self.paths.socket.as_os_str())?;
        let session = shell_quote(self.paths.session_name.as_ref())?;
        Ok(format!(
            "exec {executable} --state-root {state_root} _presentation_control --presentation-socket {socket} --presentation-session {session} --action {} --source-pane '#{{pane_id}}' --client-name #{{q:client_name}}",
            action.as_str()
        ))
    }

    /// Returns the current exact provider attachment attempt. Before its helper
    /// reports `Running`, a dead pane is atomically converted to `Failed` for
    /// an exact same-row retry. Once running, the helper itself reports its
    /// terminal phase, so this method deliberately avoids repeated control
    /// queries against the presentation tmux server.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed private status or ambiguous tmux pane
    /// evidence.
    pub fn attachment_status(&self) -> Result<Option<AttachmentStatus>, PresentationError> {
        let Some(mut status) = self.read_attachment_status()? else {
            return Ok(None);
        };
        if status.phase == AttachmentPhase::Pending && self.provider_pane_is_dead()? {
            status.phase = AttachmentPhase::Failed;
            self.write_attachment_status(&status)?;
        }
        Ok(Some(status))
    }

    /// Detaches clients from this exact presentation so the navigator can
    /// exit without trying to kill its own controlling tmux server. The outer
    /// launcher observes the dead navigator after `attach-session` returns and
    /// removes the already-proven private server and files. Provider Runtimes
    /// live on separate servers and are never targeted.
    ///
    /// # Errors
    ///
    /// Returns an error when stable private ownership cannot be proven or tmux
    /// rejects detaching clients from the exact presentation session.
    pub fn stop_session(&self) -> Result<(), PresentationError> {
        let ownership = read_presentation_ownership(&self.paths)?.ok_or(
            PresentationError::ControlRefused("presentation ownership marker is missing"),
        )?;
        let current =
            read_presentation_ownership(&self.paths)?.ok_or(PresentationError::ControlRefused(
                "presentation ownership disappeared before session stop",
            ))?;
        if current.marker != ownership.marker
            || current.marker_identity != ownership.marker_identity
            || current.socket_identity.is_none()
            || !optional_socket_identity_compatible(
                ownership.socket_identity.as_ref(),
                current.socket_identity.as_ref(),
            )
        {
            return Err(PresentationError::ControlRefused(
                "presentation ownership changed before session stop",
            ));
        }
        self.invoke(
            None,
            vec![
                "detach-client".into(),
                "-s".into(),
                self.paths.session_name.clone().into(),
            ],
        )
    }

    /// Advances only the currently recorded exact attachment attempt.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale attempt, invalid transition, or private
    /// status I/O failure.
    pub fn report_attachment_phase(
        &self,
        attempt_id: uuid::Uuid,
        phase: AttachmentPhase,
    ) -> Result<(), PresentationError> {
        let Some(mut status) = self.read_attachment_status()? else {
            return Err(PresentationError::StaleAttachmentAttempt);
        };
        if status.attempt_id != attempt_id
            || !matches!(
                (status.phase, phase),
                (
                    AttachmentPhase::Pending,
                    AttachmentPhase::Running | AttachmentPhase::Failed
                ) | (
                    AttachmentPhase::Running,
                    AttachmentPhase::Completed | AttachmentPhase::Failed
                )
            )
        {
            return Err(PresentationError::StaleAttachmentAttempt);
        }
        status.phase = phase;
        self.write_attachment_status(&status)
    }

    /// Stops only this private presentation server and removes its exact
    /// private directory. It never targets a provider runtime or default tmux.
    ///
    /// # Errors
    ///
    /// Returns an error when a live private presentation cannot be stopped or
    /// its owned directory cannot be removed.
    pub fn close(&self) -> Result<(), PresentationError> {
        let Some(ownership) = read_presentation_ownership(&self.paths)? else {
            // A fresh owner that was never started has no artifacts to clean.
            // Any existing path without our marker is foreign or malformed;
            // in particular, socket absence is not deletion authority.
            if fs::symlink_metadata(&self.paths.directory).is_ok() {
                return Err(PresentationError::ControlRefused(
                    "presentation ownership marker is missing or invalid",
                ));
            }
            return Ok(());
        };
        let current = read_presentation_ownership(&self.paths)?.ok_or(
            PresentationError::ControlRefused("presentation ownership disappeared before close"),
        )?;
        if current.marker != ownership.marker
            || current.marker_identity != ownership.marker_identity
            || (current.socket_identity.is_some()
                && (ownership.socket_identity.is_none()
                    || !optional_socket_identity_compatible(
                        ownership.socket_identity.as_ref(),
                        current.socket_identity.as_ref(),
                    )))
        {
            return Err(PresentationError::ControlRefused(
                "presentation ownership changed before close",
            ));
        }
        let result = self.invoke(None, vec!["kill-server".into()]);
        if let Err(PresentationError::TmuxRejected(message)) = &result
            && !message.contains("no server running")
            && !message.contains("No such file")
        {
            return Err(PresentationError::TmuxRejected(message.clone()));
        }
        remove_owned_presentation(&self.paths, &ownership)
    }

    fn discover_live(state_root: &Path) -> Result<Vec<Self>, PresentationError> {
        let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
        if !presentation_root.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&presentation_root).map_err(PresentationError::Io)?;
        let mut live = Vec::new();
        for entry in entries {
            let entry = entry.map_err(PresentationError::Io)?;
            if !entry.file_type().map_err(PresentationError::Io)?.is_dir() {
                return Err(PresentationError::InvalidControlPath(entry.path()));
            }
            let directory = entry.path();
            let session_name = presentation_session_name(&directory)
                .ok_or_else(|| PresentationError::InvalidControlPath(directory.clone()))?;
            let presentation =
                Self::from_control(state_root, directory.join("tmux.sock"), session_name)?;
            let session_live = presentation.is_live()?;
            let navigator_pane_dead = session_live && presentation.navigator_pane_is_dead()?;
            if should_reuse_presentation(session_live, navigator_pane_dead) {
                live.push(presentation);
            } else {
                presentation.close()?;
            }
        }
        Ok(live)
    }

    fn is_live(&self) -> Result<bool, PresentationError> {
        let mut command = private_tmux_command();
        command.arg("-S").arg(&self.paths.socket).args([
            "has-session",
            "-t",
            &self.paths.session_name,
        ]);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            return Ok(true);
        }
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        if !self.paths.socket.exists()
            || diagnostic.contains("no server running")
            || diagnostic.contains("No such file")
        {
            return Ok(false);
        }
        Err(PresentationError::TmuxRejected(sanitize_diagnostic(
            &diagnostic,
        )))
    }

    fn navigator_command(&self) -> Vec<OsString> {
        self.navigator_command_for("_navigator")
    }

    fn d17_navigator_command(&self) -> Vec<OsString> {
        self.navigator_command_for("_navigator_d17")
    }

    fn navigator_command_for(&self, pane_command: &str) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            pane_command.into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
        ]
    }

    fn provider_wait_command(&self) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_wait".into(),
        ]
    }

    fn observer_review_command(&self) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_observer_review".into(),
        ]
    }

    fn provider_attach_command(
        &self,
        workstream_id: WorkstreamId,
        attempt_id: uuid::Uuid,
    ) -> Vec<OsString> {
        vec![
            self.executable.clone().into_os_string(),
            "--state-root".into(),
            self.state_root.clone().into_os_string(),
            "_provider_attach".into(),
            workstream_id.to_string().into(),
            "--presentation-socket".into(),
            self.paths.socket.clone().into_os_string(),
            "--presentation-session".into(),
            self.paths.session_name.clone().into(),
            "--attempt-id".into(),
            attempt_id.to_string().into(),
        ]
    }

    fn prepare_attachment(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<AttachmentStatus, PresentationError> {
        let status = AttachmentStatus {
            attempt_id: uuid::Uuid::new_v4(),
            workstream_id,
            phase: AttachmentPhase::Pending,
        };
        self.write_attachment_status(&status)?;
        Ok(status)
    }

    fn finish_attachment_start(
        &self,
        mut status: AttachmentStatus,
        result: Result<(), PresentationError>,
    ) -> Result<AttachmentStatus, PresentationError> {
        if let Err(error) = result {
            status.phase = AttachmentPhase::Failed;
            let _ = self.write_attachment_status(&status);
            return Err(error);
        }
        Ok(status)
    }

    fn capture_ownership_socket_identity(&self) -> Result<(), PresentationError> {
        let Some(mut ownership) = read_presentation_ownership(&self.paths)? else {
            return Err(PresentationError::ControlRefused(
                "presentation ownership marker disappeared",
            ));
        };
        let socket = inspect_private_socket(&self.paths.socket)
            .map_err(map_presentation_ownership_probe)?
            .ok_or(PresentationError::ControlRefused(
                "private presentation socket is missing",
            ))?;
        ownership.marker.socket_identity = Some(socket);
        write_presentation_ownership_marker(
            &self.paths,
            &ownership.marker,
            Some(&ownership.marker_identity),
        )
    }

    fn read_attachment_status(&self) -> Result<Option<AttachmentStatus>, PresentationError> {
        let metadata = match fs::symlink_metadata(&self.paths.attachment_status) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(PresentationError::Io(error)),
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_ATTACHMENT_STATUS_BYTES {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let file = fs::File::open(&self.paths.attachment_status).map_err(PresentationError::Io)?;
        let mut bytes = Vec::new();
        file.take(MAX_ATTACHMENT_STATUS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(PresentationError::Io)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_STATUS_BYTES {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let status: AttachmentStatus = serde_json::from_slice(&bytes)
            .map_err(|_| PresentationError::InvalidAttachmentStatus)?;
        Ok(Some(status))
    }

    fn write_attachment_status(&self, status: &AttachmentStatus) -> Result<(), PresentationError> {
        let bytes =
            serde_json::to_vec(status).map_err(|_| PresentationError::InvalidAttachmentStatus)?;
        if bytes.len() > usize::try_from(MAX_ATTACHMENT_STATUS_BYTES).unwrap_or(usize::MAX) {
            return Err(PresentationError::InvalidAttachmentStatus);
        }
        let temporary = self
            .paths
            .directory
            .join(format!(".attachment-{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(PresentationError::Io)?;
        file.write_all(&bytes).map_err(PresentationError::Io)?;
        file.sync_all().map_err(PresentationError::Io)?;
        set_mode(&temporary, 0o600)?;
        fs::rename(&temporary, &self.paths.attachment_status).map_err(PresentationError::Io)
    }

    fn provider_pane_is_dead(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        topology
            .provider()
            .map(|pane| pane.dead)
            .ok_or(PresentationError::InvalidTopology)
    }

    fn navigator_pane_is_dead(&self) -> Result<bool, PresentationError> {
        let topology = self.read_topology_allow_dead()?;
        topology
            .navigator()
            .map(|pane| pane.dead)
            .ok_or(PresentationError::InvalidTopology)
    }

    #[cfg(test)]
    fn pane_dead_arguments(&self, pane: &str) -> Vec<OsString> {
        vec![
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            self.pane_target(pane).into(),
            "#{pane_dead}".into(),
        ]
    }

    /// Keeps the narrow navigator at its deliberate default width, leaving
    /// all remaining terminal columns to the native provider pane.
    /// Reapplies the compact navigator layout after tmux adopts a controlling
    /// client's terminal size.
    ///
    /// # Errors
    ///
    /// Returns an error when the exact private tmux server rejects the resize.
    pub fn set_default_navigator_width(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        self.invoke(
            None,
            self.default_navigator_resize_arguments_for(&navigator),
        )
    }

    fn default_navigator_resize_arguments_for(&self, navigator: &str) -> Vec<OsString> {
        vec![
            "resize-pane".into(),
            "-t".into(),
            self.pane_target(navigator).into(),
            "-x".into(),
            DEFAULT_NAVIGATOR_PANE_WIDTH.to_string().into(),
        ]
    }

    /// Keeps the compact split invariant at the private tmux event boundary.
    /// A detached server starts at its configured default size; when the first
    /// real client attaches, tmux otherwise expands both panes proportionally
    /// before the Navigator can receive a terminal resize event.
    fn install_navigator_width_hooks(&self) -> Result<(), PresentationError> {
        let navigator = self.navigator_target()?;
        for hook in NAVIGATOR_WIDTH_HOOKS {
            self.invoke(
                None,
                self.navigator_width_hook_arguments_for(hook, &navigator),
            )?;
        }
        Ok(())
    }

    fn navigator_width_hook_arguments_for(&self, hook: &str, navigator: &str) -> Vec<OsString> {
        vec![
            "set-hook".into(),
            "-t".into(),
            self.paths.session_name.clone().into(),
            hook.into(),
            format!(
                "resize-pane -t {} -x {DEFAULT_NAVIGATOR_PANE_WIDTH}",
                self.pane_target(navigator)
            )
            .into(),
        ]
    }

    fn invoke(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<(), PresentationError> {
        self.invoke_capture(config, arguments).map(|_| ())
    }

    fn invoke_capture(
        &self,
        config: Option<&Path>,
        arguments: Vec<OsString>,
    ) -> Result<String, PresentationError> {
        let mut command = private_tmux_command();
        if let Some(config) = config {
            command.arg("-f").arg(config);
        }
        command.arg("-S").arg(&self.paths.socket).args(arguments);
        let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
            .map_err(PresentationError::from_bounded_tmux)?;
        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|_| {
                PresentationError::TmuxRejected(
                    "private presentation tmux output was not UTF-8".to_owned(),
                )
            })
        } else {
            Err(PresentationError::TmuxRejected(sanitize_diagnostic(
                &String::from_utf8_lossy(&output.stderr),
            )))
        }
    }
}

/// Classifies legacy presentation ownership without opening host state or
/// invoking any mutating presentation helper.  This is intentionally separate
/// from [`Presentation::open_or_create`]: a cutover launcher must be
/// able to inspect and prove the old presentation before it is allowed to
/// acquire a transition lease or present confirmation.
///
/// The only live process inspected here is the navigator pane itself.  No
/// signal, provider Runtime socket, tmux kill, or directory removal is ever
/// attempted.  A second presentation is always refused, even when its tmux
/// server is already dead.
///
/// # Errors
///
/// Returns [`PresentationError::AmbiguousLegacyPresentations`] when more than
/// one exact presentation directory is present.  Other unsafe evidence is
/// returned as a typed [`LegacyPresentationState`] so the launcher can provide
/// bounded drain/recovery guidance without guessing.
#[allow(
    clippy::too_many_lines,
    reason = "The classifier keeps its fail-closed root inventory and ambiguity gate in one auditable boundary."
)]
pub fn classify_legacy_presentations(
    state_root: &Path,
) -> Result<LegacyPresentationAssessment, PresentationError> {
    let state_metadata = match fs::symlink_metadata(state_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LegacyPresentationAssessment::none());
        }
        Err(error) => {
            return Ok(LegacyPresentationAssessment::classified(classify_fs_error(
                &error,
            )));
        }
    };
    if state_metadata.file_type().is_symlink() {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Foreign,
        ));
    }
    if !state_metadata.is_dir() {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Malformed,
        ));
    }
    if !is_private_owner_directory(&state_metadata) {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Foreign,
        ));
    }
    let Some(presentation_root) = inspect_private_presentation_root(state_root)? else {
        return Ok(LegacyPresentationAssessment::none());
    };
    let presentation_metadata = match fs::symlink_metadata(&presentation_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LegacyPresentationAssessment::none());
        }
        Err(error) => {
            return Ok(LegacyPresentationAssessment::classified(classify_fs_error(
                &error,
            )));
        }
    };
    if presentation_metadata.file_type().is_symlink() {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Foreign,
        ));
    }
    if !presentation_metadata.is_dir() {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Malformed,
        ));
    }
    if !is_private_owner_directory(&presentation_metadata) {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Foreign,
        ));
    }
    let entries = match fs::read_dir(&presentation_root) {
        Ok(entries) => entries,
        Err(error) => {
            return Ok(LegacyPresentationAssessment::classified(classify_fs_error(
                &error,
            )));
        }
    };
    let mut candidate_directories = Vec::new();
    let mut unknown_entry = false;
    for entry in bounded_directory_entries(entries) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return Ok(LegacyPresentationAssessment::classified(classify_fs_error(
                    &error,
                )));
            }
        };
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(LegacyPresentationAssessment::classified(classify_fs_error(
                    &error,
                )));
            }
        };
        if metadata.file_type().is_symlink() {
            return Ok(LegacyPresentationAssessment::classified(
                LegacyPresentationState::Foreign,
            ));
        }
        if !metadata.is_dir() {
            unknown_entry = true;
            continue;
        }
        if presentation_session_name(&path).is_some() {
            candidate_directories.push(path);
        } else {
            unknown_entry = true;
        }
    }

    if candidate_directories.len() > 1 {
        return Err(PresentationError::AmbiguousLegacyPresentations);
    }
    let Some(directory) = candidate_directories.pop() else {
        return Ok(if unknown_entry {
            LegacyPresentationAssessment::classified(LegacyPresentationState::Malformed)
        } else {
            LegacyPresentationAssessment::none()
        });
    };
    if unknown_entry {
        return Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Malformed,
        ));
    }

    if retirement_marker_is_present(&directory) {
        return Ok(classify_retirement_marker(state_root, &directory));
    }

    match inspect_legacy_presentation(&directory) {
        Ok(evidence) => Ok(classify_legacy_evidence_internal(
            &directory, state_root, &evidence,
        )),
        Err(LegacyProbeFailure::Malformed) => Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Malformed,
        )),
        Err(LegacyProbeFailure::Foreign) => Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Foreign,
        )),
        Err(LegacyProbeFailure::Inaccessible) => Ok(LegacyPresentationAssessment::classified(
            LegacyPresentationState::Inaccessible,
        )),
    }
}

/// Classifies an already captured evidence set without filesystem, process,
/// or tmux access.  It is public so deterministic launcher tests can
/// inject process/tmux evidence without needing a live tmux server.
#[must_use]
pub fn classify_legacy_evidence(
    directory: &Path,
    state_root: &Path,
    evidence: LegacyPresentationEvidenceForTest,
) -> LegacyPresentationAssessment {
    let evidence = evidence.into_internal();
    classify_legacy_evidence_internal(directory, state_root, &evidence)
}

/// Reclassifies one supplied proof and requires byte-for-byte equality with
/// fresh bounded evidence.  This is the only proof comparison authority used
/// by the D16 cutover mutation helpers.
///
/// # Errors
///
/// Returns [`PresentationError::LegacyProofChanged`] when the presentation
/// disappeared, changed identity, or can no longer be proven exactly.
pub fn revalidate_legacy_presentation(
    state_root: &Path,
    expected: &LegacyPresentationProof,
) -> Result<LegacyPresentationAssessment, PresentationError> {
    let fresh = classify_legacy_presentations(state_root)?;
    if fresh.proof().is_some_and(|proof| proof == expected) {
        Ok(fresh)
    } else {
        Err(PresentationError::LegacyProofChanged)
    }
}

/// Attaches to an already-running exact private presentation for drain-only
/// review.  No host registry, provider Runtime socket, or process-control
/// path is opened.  The attached/utility/observer surface must also carry a
/// fully proven navigator/controller pair; an attached presentation whose
/// pane evidence is incomplete is refused without invoking tmux attach.
///
/// # Errors
///
/// Returns [`PresentationError::LegacyProofChanged`] when the supplied proof
/// no longer matches, or [`PresentationError::LegacyMutationRefused`] when
/// the presentation is not an eligible, fully proven drain surface.
pub fn drain_attach_legacy_presentation(
    state_root: &Path,
    expected: &LegacyPresentationProof,
) -> Result<(), PresentationError> {
    let root = state_root.to_path_buf();
    drain_attach_legacy_presentation_with(
        expected,
        || classify_legacy_presentations(&root),
        |proof| legacy_tmux_attach(&root, proof),
    )
}

/// Retires one freshly revalidated detached legacy presentation.  Retirement
/// targets only the exact private tmux socket recorded in the proof, waits for
/// the old server/navigator to disappear, then performs strict known-artifact
/// cleanup and independently proves the root is empty.
///
/// # Errors
///
/// Returns [`PresentationError::LegacyProofChanged`] before any kill/remove
/// effect when the supplied proof is stale or changed.
pub fn retire_legacy_presentation(
    state_root: &Path,
    expected: &LegacyPresentationProof,
    lease: &TransitionLease,
) -> Result<(), PresentationError> {
    ensure_transition_lease(state_root, lease)?;
    let root = state_root.to_path_buf();
    retire_legacy_presentation_with(
        expected,
        || classify_legacy_presentations(&root),
        |proof| legacy_tmux_kill_server(&root, proof),
        |dead_proof| remove_dead_legacy_presentation(&root, dead_proof, lease),
        legacy_tmux_server_is_live,
    )
}

/// Removes only exact, freshly revalidated dead-owned presentation artifacts.
/// The operation never recursively removes a directory: it validates the
/// bounded allowlist, removes only `attachment.json`, `tmux.conf`, and
/// `tmux.sock` when their recorded identities still match, then removes the
/// exact presentation directory only when empty.
///
/// A second call after complete or partial known-artifact disappearance is
/// idempotent; an unknown/new entry, symlink, changed identity, or live server
/// refuses before it is removed.
///
/// # Errors
///
/// Returns [`PresentationError::LegacyProofChanged`] when the proof or exact
/// private artifacts no longer match, and a typed refusal for live,
/// ambiguous, malformed, or inaccessible evidence.
pub fn remove_dead_legacy_presentation(
    state_root: &Path,
    expected: &LegacyPresentationProof,
    lease: &TransitionLease,
) -> Result<(), PresentationError> {
    ensure_transition_lease(state_root, lease)?;
    let root = state_root.to_path_buf();
    let fresh = classify_legacy_presentations(&root)?;
    if fresh.state() == LegacyPresentationState::None {
        return Ok(());
    }
    let marker = if fresh.state() == LegacyPresentationState::DeadOwned {
        let actual = fresh.proof().ok_or(PresentationError::LegacyProofChanged)?;
        if !dead_cleanup_proof_matches(expected, actual) {
            return Err(PresentationError::LegacyProofChanged);
        }
        ensure_retirement_marker(&root, expected)?
    } else {
        read_retirement_marker(&root, expected)?.ok_or(PresentationError::LegacyProofChanged)?
    };
    if legacy_tmux_server_is_live(expected)? {
        return Err(PresentationError::LegacyMutationRefused(
            "dead-owned cleanup found a live private tmux server",
        ));
    }
    remove_exact_legacy_artifacts(&root, expected, &marker)?;
    let final_assessment = classify_legacy_presentations(&root)?;
    if final_assessment.state() == LegacyPresentationState::None
        && final_assessment.proof().is_none()
    {
        Ok(())
    } else {
        Err(PresentationError::LegacyNotRetired)
    }
}

fn ensure_transition_lease(
    state_root: &Path,
    lease: &TransitionLease,
) -> Result<(), PresentationError> {
    lease
        .revalidate_for_mutation(state_root)
        .map_err(|error| match error {
            crate::state::StateError::TransitionLeaseRootMismatch => {
                PresentationError::LegacyMutationRefused(
                    "transition lease root does not match presentation root",
                )
            }
            _ => PresentationError::LegacyMutationRefused(
                "transition lease is no longer valid for presentation mutation",
            ),
        })
}

fn drain_attach_legacy_presentation_with<F, A>(
    expected: &LegacyPresentationProof,
    mut fresh: F,
    mut attach: A,
) -> Result<(), PresentationError>
where
    F: FnMut() -> Result<LegacyPresentationAssessment, PresentationError>,
    A: FnMut(&LegacyPresentationProof) -> Result<(), PresentationError>,
{
    let assessment = fresh()?;
    let proof = exact_revalidated_proof(expected, &assessment)?;
    if !matches!(
        assessment.state(),
        LegacyPresentationState::Attached
            | LegacyPresentationState::UtilityShell
            | LegacyPresentationState::ObserverReview
    ) {
        return Err(PresentationError::LegacyMutationRefused(
            "presentation is not a drain-only surface",
        ));
    }
    if !proof.controller_proven() {
        return Err(PresentationError::LegacyMutationRefused(
            "navigator/controller evidence is incomplete",
        ));
    }
    attach(proof)
}

fn retire_legacy_presentation_with<F, K, C, L>(
    expected: &LegacyPresentationProof,
    mut fresh: F,
    mut kill: K,
    mut cleanup: C,
    mut server_live: L,
) -> Result<(), PresentationError>
where
    F: FnMut() -> Result<LegacyPresentationAssessment, PresentationError>,
    K: FnMut(&LegacyPresentationProof) -> Result<(), PresentationError>,
    C: FnMut(&LegacyPresentationProof) -> Result<(), PresentationError>,
    L: FnMut(&LegacyPresentationProof) -> Result<bool, PresentationError>,
{
    let assessment = fresh()?;
    let proof = exact_revalidated_proof(expected, &assessment)?;
    if assessment.state() != LegacyPresentationState::DetachedOrdinary {
        return Err(PresentationError::LegacyMutationRefused(
            "only a detached ordinary presentation may be retired",
        ));
    }
    kill(proof)?;

    for _ in 0..MAX_LEGACY_RETIREMENT_ATTEMPTS {
        let after = fresh()?;
        if server_live(expected)? {
            thread::sleep(SHELL_CLAIM_RETRY);
            continue;
        }
        match after.state() {
            LegacyPresentationState::None => return Ok(()),
            LegacyPresentationState::DeadOwned => {
                let dead_proof = after.proof().ok_or(PresentationError::LegacyProofChanged)?;
                cleanup(dead_proof)?;
                let final_assessment = fresh()?;
                if final_assessment.state() == LegacyPresentationState::None
                    && final_assessment.proof().is_none()
                {
                    return Ok(());
                }
                return Err(PresentationError::LegacyNotRetired);
            }
            _ => thread::sleep(SHELL_CLAIM_RETRY),
        }
    }
    Err(PresentationError::LegacyNotRetired)
}

fn exact_revalidated_proof<'a>(
    expected: &LegacyPresentationProof,
    assessment: &'a LegacyPresentationAssessment,
) -> Result<&'a LegacyPresentationProof, PresentationError> {
    assessment
        .proof()
        .filter(|proof| *proof == expected)
        .ok_or(PresentationError::LegacyProofChanged)
}

fn dead_cleanup_proof_matches(
    expected: &LegacyPresentationProof,
    actual: &LegacyPresentationProof,
) -> bool {
    expected.directory == actual.directory
        && directory_identity_compatible(&expected.directory_identity, &actual.directory_identity)
        && expected.socket == actual.socket
        && expected.session_name == actual.session_name
        && expected.config_identity == actual.config_identity
        && optional_socket_identity_compatible(
            expected.socket_identity.as_ref(),
            actual.socket_identity.as_ref(),
        )
        && optional_identity_compatible(
            expected.attachment_identity.as_ref(),
            actual.attachment_identity.as_ref(),
        )
        && optional_status_compatible(
            expected.attachment_status.as_ref(),
            actual.attachment_status.as_ref(),
        )
}

fn directory_identity_compatible(
    expected: &LegacyFileIdentity,
    actual: &LegacyFileIdentity,
) -> bool {
    // Directory size is allowed to change as known artifacts disappear; the
    // owner/mode and device/inode identity must remain exact.
    expected.mode == actual.mode
        && expected.device == actual.device
        && expected.inode == actual.inode
}

fn optional_identity_compatible(
    expected: Option<&LegacyFileIdentity>,
    actual: Option<&LegacyFileIdentity>,
) -> bool {
    actual.is_none() || actual == expected
}

fn socket_identity_compatible(expected: &LegacyFileIdentity, actual: &LegacyFileIdentity) -> bool {
    private_socket_mode(expected.mode)
        && private_socket_mode(actual.mode)
        && expected.size == actual.size
        && expected.device == actual.device
        && expected.inode == actual.inode
        && expected.digest == actual.digest
}

fn optional_socket_identity_compatible(
    expected: Option<&LegacyFileIdentity>,
    actual: Option<&LegacyFileIdentity>,
) -> bool {
    match (expected, actual) {
        (_, None) => true,
        (Some(expected), Some(actual)) => socket_identity_compatible(expected, actual),
        (None, Some(_)) => false,
    }
}

fn socket_identity_options_match(
    left: Option<&LegacyFileIdentity>,
    right: Option<&LegacyFileIdentity>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => socket_identity_compatible(left, right),
        _ => false,
    }
}

fn optional_status_compatible(
    expected: Option<&LegacyAttachmentStatus>,
    actual: Option<&LegacyAttachmentStatus>,
) -> bool {
    actual.is_none() || actual == expected
}

fn exact_legacy_paths(
    state_root: &Path,
    proof: &LegacyPresentationProof,
) -> Result<PresentationPaths, PresentationError> {
    let paths = PresentationPaths::from_control(
        state_root,
        proof.socket.clone(),
        proof.session_name.clone(),
    )
    .map_err(|_| PresentationError::LegacyMutationRefused("presentation path is not exact"))?;
    if paths.directory != proof.directory || paths.socket != proof.socket {
        return Err(PresentationError::LegacyProofChanged);
    }
    let metadata = fs::symlink_metadata(&paths.directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PresentationError::LegacyProofChanged
        } else {
            PresentationError::Io(error)
        }
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(PresentationError::LegacyMutationRefused(
            "presentation directory is not private and regular",
        ));
    }
    let actual_identity = legacy_file_identity(&metadata, None);
    if !directory_identity_compatible(&proof.directory_identity, &actual_identity) {
        return Err(PresentationError::LegacyProofChanged);
    }
    Ok(paths)
}

fn validate_exact_socket(
    paths: &PresentationPaths,
    expected: Option<&LegacyFileIdentity>,
) -> Result<Option<LegacyFileIdentity>, PresentationError> {
    let actual = inspect_private_socket(&paths.socket).map_err(map_cleanup_probe_failure)?;
    if !optional_socket_identity_compatible(expected, actual.as_ref()) {
        return Err(PresentationError::LegacyProofChanged);
    }
    Ok(actual)
}

fn legacy_tmux_attach(
    state_root: &Path,
    proof: &LegacyPresentationProof,
) -> Result<(), PresentationError> {
    let paths = exact_legacy_paths(state_root, proof)?;
    let actual = validate_exact_socket(&paths, proof.socket_identity.as_ref())?;
    if actual.is_none() {
        return Err(PresentationError::LegacyMutationRefused(
            "presentation socket disappeared before drain attach",
        ));
    }
    let status = private_tmux_command()
        .args(["-f", "/dev/null", "-S"])
        .arg(&paths.socket)
        .args(["attach-session", "-t", paths.session_name.as_str()])
        .status()
        .map_err(PresentationError::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(PresentationError::TmuxRejected(
            "legacy presentation drain attach failed".to_owned(),
        ))
    }
}

fn legacy_tmux_kill_server(
    state_root: &Path,
    proof: &LegacyPresentationProof,
) -> Result<(), PresentationError> {
    let paths = exact_legacy_paths(state_root, proof)?;
    let config = inspect_regular_file(&paths.config, true, MAX_LEGACY_CONFIG_BYTES)
        .map_err(map_cleanup_probe_failure)?
        .ok_or(PresentationError::LegacyProofChanged)?;
    if config != proof.config_identity {
        return Err(PresentationError::LegacyProofChanged);
    }
    let actual = validate_exact_socket(&paths, proof.socket_identity.as_ref())?;
    if actual.is_none() {
        return Ok(());
    }
    let mut command = private_tmux_command();
    command
        .args(["-f", "/dev/null", "-S"])
        .arg(&paths.socket)
        .arg("kill-server");
    let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
        .map_err(PresentationError::from_bounded_tmux)?;
    if output.status.success() {
        return Ok(());
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    if diagnostic.contains("no server running")
        || diagnostic.contains("No such file")
        || diagnostic.contains("no sessions")
    {
        Ok(())
    } else {
        Err(PresentationError::TmuxRejected(sanitize_diagnostic(
            &diagnostic,
        )))
    }
}

fn legacy_tmux_server_is_live(proof: &LegacyPresentationProof) -> Result<bool, PresentationError> {
    let actual = inspect_private_socket(&proof.socket).map_err(map_cleanup_probe_failure)?;
    if !optional_socket_identity_compatible(proof.socket_identity.as_ref(), actual.as_ref()) {
        return Err(PresentationError::LegacyProofChanged);
    }
    if actual.is_none() {
        return Ok(false);
    }
    let mut command = private_tmux_command();
    command
        .args(["-f", "/dev/null", "-S"])
        .arg(&proof.socket)
        .args(["has-session", "-t", proof.session_name.as_str()]);
    let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
        .map_err(PresentationError::from_bounded_tmux)?;
    if output.status.success() {
        return Ok(true);
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    if diagnostic.contains("no server running")
        || diagnostic.contains("No such file")
        || diagnostic.contains("no sessions")
    {
        Ok(false)
    } else if diagnostic.contains("can't find session") {
        legacy_tmux_server_has_any_session(&proof.socket)
    } else {
        Err(PresentationError::TmuxRejected(sanitize_diagnostic(
            &diagnostic,
        )))
    }
}

fn legacy_tmux_server_has_any_session(socket: &Path) -> Result<bool, PresentationError> {
    let mut command = private_tmux_command();
    command.args(["-f", "/dev/null", "-S"]).arg(socket).args([
        "list-sessions",
        "-F",
        "#{session_name}",
    ]);
    let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
        .map_err(PresentationError::from_bounded_tmux)?;
    if output.status.success() {
        return Ok(!output.stdout.is_empty());
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    if diagnostic.contains("no server running")
        || diagnostic.contains("No such file")
        || diagnostic.contains("no sessions")
    {
        Ok(false)
    } else {
        Err(PresentationError::TmuxRejected(sanitize_diagnostic(
            &diagnostic,
        )))
    }
}

fn retirement_marker_is_present(directory: &Path) -> bool {
    fs::symlink_metadata(directory.join(LEGACY_RETIREMENT_MARKER_FILE)).is_ok()
}

fn classify_retirement_marker(state_root: &Path, directory: &Path) -> LegacyPresentationAssessment {
    let marker = match read_retirement_marker_for_discovery(directory) {
        Ok(Some(marker)) => marker,
        Ok(None) => {
            return LegacyPresentationAssessment::classified(LegacyPresentationState::Malformed);
        }
        Err(failure) => return assessment_for_legacy_probe_failure(failure),
    };
    let Some(expected_session) = presentation_session_name(directory) else {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    };
    if marker.version != 1
        || marker.directory != directory
        || marker.socket != directory.join("tmux.sock")
        || marker.session_name != expected_session
        || marker.config_identity.mode != 0o600
        || !config_content_matches(&marker.config_identity)
    {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let directory_metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) => return assessment_for_fs_error(&error),
    };
    if directory_metadata.file_type().is_symlink() {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    if !directory_metadata.is_dir() || !is_private_owner_directory(&directory_metadata) {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let directory_identity = legacy_file_identity(&directory_metadata, None);
    if !directory_identity_compatible(&marker.directory_identity, &directory_identity) {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let paths = match PresentationPaths::from_control(
        state_root,
        marker.socket.clone(),
        marker.session_name.clone(),
    ) {
        Ok(paths) if paths.directory == directory => paths,
        _ => return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign),
    };
    if let Err(error) = validate_legacy_artifact_entries(&paths.directory, true) {
        return assessment_for_cleanup_error(error);
    }
    let config = match inspect_regular_file(&paths.config, false, MAX_LEGACY_CONFIG_BYTES) {
        Ok(config) => config,
        Err(failure) => return assessment_for_legacy_probe_failure(failure),
    };
    if config.is_some() && config != Some(marker.config_identity) {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let attachment = match inspect_regular_file(
        &paths.attachment_status,
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    ) {
        Ok(attachment) => attachment,
        Err(failure) => return assessment_for_legacy_probe_failure(failure),
    };
    if !optional_identity_compatible(marker.attachment_identity.as_ref(), attachment.as_ref()) {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let socket = match inspect_private_socket(&paths.socket) {
        Ok(socket) => socket,
        Err(failure) => return assessment_for_legacy_probe_failure(failure),
    };
    if !optional_socket_identity_compatible(marker.socket_identity.as_ref(), socket.as_ref()) {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let proof = legacy_proof_from_retirement_marker(&marker);
    match legacy_tmux_server_is_live(&proof) {
        Ok(false) => LegacyPresentationAssessment {
            state: LegacyPresentationState::DeadOwned,
            proof: Some(proof),
        },
        Ok(true) => LegacyPresentationAssessment::classified(LegacyPresentationState::Malformed),
        Err(
            PresentationError::LegacyProofChanged | PresentationError::LegacyMutationRefused(_),
        ) => LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign),
        Err(_) => LegacyPresentationAssessment::classified(LegacyPresentationState::Inaccessible),
    }
}

fn assessment_for_cleanup_error(error: PresentationError) -> LegacyPresentationAssessment {
    match error {
        PresentationError::LegacyMutationRefused(_) => {
            LegacyPresentationAssessment::classified(LegacyPresentationState::Malformed)
        }
        PresentationError::LegacyProofChanged => {
            LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign)
        }
        PresentationError::Io(error) => assessment_for_fs_error(&error),
        _ => LegacyPresentationAssessment::classified(LegacyPresentationState::Inaccessible),
    }
}

fn assessment_for_legacy_probe_failure(
    failure: LegacyProbeFailure,
) -> LegacyPresentationAssessment {
    LegacyPresentationAssessment::classified(match failure {
        LegacyProbeFailure::Malformed => LegacyPresentationState::Malformed,
        LegacyProbeFailure::Foreign => LegacyPresentationState::Foreign,
        LegacyProbeFailure::Inaccessible => LegacyPresentationState::Inaccessible,
    })
}

fn assessment_for_fs_error(error: &std::io::Error) -> LegacyPresentationAssessment {
    LegacyPresentationAssessment::classified(classify_fs_error(error))
}

fn legacy_proof_from_retirement_marker(marker: &LegacyRetirementMarker) -> LegacyPresentationProof {
    LegacyPresentationProof {
        directory: marker.directory.clone(),
        directory_identity: marker.directory_identity,
        socket: marker.socket.clone(),
        socket_identity: marker.socket_identity,
        config_identity: marker.config_identity,
        attachment_identity: marker.attachment_identity,
        session_name: marker.session_name.clone(),
        session_id: None,
        window_id: None,
        navigator: None,
        provider: None,
        utility: None,
        clients: Vec::new(),
        shell_claim_present: false,
        legacy_executable: None,
        attachment_status: None,
    }
}

fn read_retirement_marker_for_discovery(
    directory: &Path,
) -> Result<Option<LegacyRetirementMarker>, LegacyProbeFailure> {
    let marker_path = directory.join(LEGACY_RETIREMENT_MARKER_FILE);
    let Some(identity) =
        inspect_regular_file(&marker_path, false, MAX_LEGACY_RETIREMENT_MARKER_BYTES)?
    else {
        return Ok(None);
    };
    let bytes = read_private_file(&marker_path, MAX_LEGACY_RETIREMENT_MARKER_BYTES)?
        .ok_or(LegacyProbeFailure::Inaccessible)?;
    let marker = serde_json::from_slice::<LegacyRetirementMarker>(&bytes)
        .map_err(|_| LegacyProbeFailure::Malformed)?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    let expected_digest: [u8; 32] = digest.finalize().into();
    if identity.size != bytes.len() as u64 || identity.digest != Some(expected_digest) {
        return Err(LegacyProbeFailure::Foreign);
    }
    Ok(Some(marker))
}

fn retirement_marker_for(proof: &LegacyPresentationProof) -> LegacyRetirementMarker {
    LegacyRetirementMarker {
        version: 1,
        directory: proof.directory.clone(),
        directory_identity: proof.directory_identity,
        socket: proof.socket.clone(),
        session_name: proof.session_name.clone(),
        config_identity: proof.config_identity,
        socket_identity: proof.socket_identity,
        attachment_identity: proof.attachment_identity,
    }
}

fn marker_matches_proof(marker: &LegacyRetirementMarker, proof: &LegacyPresentationProof) -> bool {
    marker.version == 1
        && marker.directory == proof.directory
        && directory_identity_compatible(&marker.directory_identity, &proof.directory_identity)
        && marker.socket == proof.socket
        && marker.session_name == proof.session_name
        && marker.config_identity == proof.config_identity
        && socket_identity_options_match(
            marker.socket_identity.as_ref(),
            proof.socket_identity.as_ref(),
        )
        && marker.attachment_identity == proof.attachment_identity
}

fn ensure_retirement_marker(
    state_root: &Path,
    proof: &LegacyPresentationProof,
) -> Result<LegacyRetirementMarker, PresentationError> {
    let paths = exact_legacy_paths(state_root, proof)?;
    let marker_path = paths.directory.join(LEGACY_RETIREMENT_MARKER_FILE);
    if marker_path.exists() {
        return read_retirement_marker(state_root, proof)?.ok_or(
            PresentationError::LegacyMutationRefused("retirement marker disappeared"),
        );
    }
    validate_legacy_artifact_entries(&paths.directory, false)?;
    let marker = retirement_marker_for(proof);
    let bytes = serde_json::to_vec(&marker).map_err(|_| {
        PresentationError::LegacyMutationRefused("retirement marker could not be encoded")
    })?;
    if bytes.len() > MAX_LEGACY_RETIREMENT_MARKER_BYTES {
        return Err(PresentationError::LegacyMutationRefused(
            "retirement marker exceeded bound",
        ));
    }
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_retirement_marker(state_root, proof)?.ok_or(
                PresentationError::LegacyMutationRefused("retirement marker is invalid"),
            );
        }
        Err(error) => return Err(PresentationError::Io(error)),
    };
    set_mode(&marker_path, 0o600)?;
    file.write_all(&bytes).map_err(PresentationError::Io)?;
    file.sync_all().map_err(PresentationError::Io)?;
    sync_directory(&paths.directory)?;
    Ok(marker)
}

fn read_retirement_marker(
    state_root: &Path,
    proof: &LegacyPresentationProof,
) -> Result<Option<LegacyRetirementMarker>, PresentationError> {
    let paths = exact_legacy_paths(state_root, proof)?;
    let marker_path = paths.directory.join(LEGACY_RETIREMENT_MARKER_FILE);
    let Some(identity) =
        inspect_regular_file(&marker_path, false, MAX_LEGACY_RETIREMENT_MARKER_BYTES)
            .map_err(map_cleanup_probe_failure)?
    else {
        return Ok(None);
    };
    let bytes = read_private_file(&marker_path, MAX_LEGACY_RETIREMENT_MARKER_BYTES)
        .map_err(map_cleanup_probe_failure)?
        .ok_or(PresentationError::LegacyMutationRefused(
            "retirement marker disappeared",
        ))?;
    let marker = serde_json::from_slice::<LegacyRetirementMarker>(&bytes)
        .map_err(|_| PresentationError::LegacyMutationRefused("retirement marker is malformed"))?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    let expected_digest: [u8; 32] = digest.finalize().into();
    if !marker_matches_proof(&marker, proof)
        || identity.size != bytes.len() as u64
        || identity.digest != Some(expected_digest)
    {
        return Err(PresentationError::LegacyProofChanged);
    }
    Ok(Some(marker))
}

fn remove_exact_legacy_artifacts(
    state_root: &Path,
    proof: &LegacyPresentationProof,
    marker: &LegacyRetirementMarker,
) -> Result<(), PresentationError> {
    remove_exact_legacy_artifacts_with(state_root, proof, marker, |_| Ok(()))
}

fn remove_exact_legacy_artifacts_with<F>(
    state_root: &Path,
    proof: &LegacyPresentationProof,
    marker: &LegacyRetirementMarker,
    mut after_remove: F,
) -> Result<(), PresentationError>
where
    F: FnMut(&Path) -> Result<(), PresentationError>,
{
    let paths = exact_legacy_paths(state_root, proof)?;
    if !marker_matches_proof(marker, proof) {
        return Err(PresentationError::LegacyProofChanged);
    }
    let current_marker =
        read_retirement_marker(state_root, proof)?.ok_or(PresentationError::LegacyProofChanged)?;
    if current_marker != *marker {
        return Err(PresentationError::LegacyProofChanged);
    }
    let _ = validate_exact_socket(&paths, proof.socket_identity.as_ref())?;
    validate_legacy_artifact_entries(&paths.directory, true)?;

    let config = inspect_regular_file(&paths.config, false, MAX_LEGACY_CONFIG_BYTES)
        .map_err(map_cleanup_probe_failure)?;
    if config.is_some() && config != Some(proof.config_identity) {
        return Err(PresentationError::LegacyProofChanged);
    }
    let attachment = inspect_regular_file(
        &paths.attachment_status,
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    )
    .map_err(map_cleanup_probe_failure)?;
    if !optional_identity_compatible(proof.attachment_identity.as_ref(), attachment.as_ref()) {
        return Err(PresentationError::LegacyProofChanged);
    }
    let socket = inspect_private_socket(&paths.socket).map_err(map_cleanup_probe_failure)?;
    if !optional_socket_identity_compatible(proof.socket_identity.as_ref(), socket.as_ref()) {
        return Err(PresentationError::LegacyProofChanged);
    }

    // Recheck the bounded name allowlist immediately before mutation so a
    // newly appearing sibling is refused rather than removed around.
    validate_legacy_artifact_entries(&paths.directory, true)?;
    remove_exact_regular_artifact(
        &paths.attachment_status,
        proof.attachment_identity.as_ref(),
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
        &mut after_remove,
    )?;
    remove_exact_regular_artifact(
        &paths.config,
        Some(&proof.config_identity),
        MAX_LEGACY_CONFIG_BYTES,
        &mut after_remove,
    )?;
    remove_exact_socket_artifact(
        &paths.socket,
        proof.socket_identity.as_ref(),
        &mut after_remove,
    )?;
    let marker_path = paths.directory.join(LEGACY_RETIREMENT_MARKER_FILE);
    let current_marker =
        read_retirement_marker(state_root, proof)?.ok_or(PresentationError::LegacyProofChanged)?;
    if current_marker != *marker {
        return Err(PresentationError::LegacyProofChanged);
    }
    match fs::remove_file(marker_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(PresentationError::Io(error)),
    }
    sync_directory(&paths.directory)?;
    match fs::remove_dir(&paths.directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            return Err(PresentationError::LegacyMutationRefused(
                "presentation directory gained an entry during cleanup",
            ));
        }
        Err(error) => return Err(PresentationError::Io(error)),
    }
    if let Some(parent) = paths.directory.parent() {
        sync_directory(parent)?;
        if let Some(root) = parent.parent() {
            sync_directory(root)?;
        }
    }
    Ok(())
}

fn remove_exact_regular_artifact<F>(
    path: &Path,
    expected: Option<&LegacyFileIdentity>,
    max_bytes: usize,
    after_remove: &mut F,
) -> Result<(), PresentationError>
where
    F: FnMut(&Path) -> Result<(), PresentationError>,
{
    // This is deliberately repeated for each unlink.  An interruption hook
    // or another actor may replace a later artifact after an earlier unlink;
    // the replacement must fail identity validation before it is touched.
    let actual = inspect_regular_file(path, false, max_bytes).map_err(map_cleanup_probe_failure)?;
    if !optional_socket_identity_compatible(expected, actual.as_ref()) {
        return Err(PresentationError::LegacyProofChanged);
    }
    if actual.is_none() {
        return Ok(());
    }
    match fs::remove_file(path) {
        Ok(()) => after_remove(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PresentationError::Io(error)),
    }
}

fn remove_exact_socket_artifact<F>(
    path: &Path,
    expected: Option<&LegacyFileIdentity>,
    after_remove: &mut F,
) -> Result<(), PresentationError>
where
    F: FnMut(&Path) -> Result<(), PresentationError>,
{
    let actual = inspect_private_socket(path).map_err(map_cleanup_probe_failure)?;
    if !optional_identity_compatible(expected, actual.as_ref()) {
        return Err(PresentationError::LegacyProofChanged);
    }
    if actual.is_none() {
        return Ok(());
    }
    match fs::remove_file(path) {
        Ok(()) => after_remove(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(PresentationError::Io(error)),
    }
}

fn validate_legacy_artifact_entries(
    directory: &Path,
    allow_marker: bool,
) -> Result<(), PresentationError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PresentationError::LegacyProofChanged
        } else {
            PresentationError::Io(error)
        }
    })?;
    let mut count = 0;
    for entry in entries.take(MAX_LEGACY_PRESENTATION_ENTRIES + 1) {
        count += 1;
        if count > MAX_LEGACY_PRESENTATION_ENTRIES {
            return Err(PresentationError::LegacyMutationRefused(
                "presentation artifact count exceeded bound",
            ));
        }
        let entry = entry.map_err(PresentationError::Io)?;
        let name = entry.file_name();
        if name != ATTACHMENT_STATUS_FILE
            && name != "tmux.conf"
            && name != "tmux.sock"
            && (!allow_marker || name != LEGACY_RETIREMENT_MARKER_FILE)
        {
            return Err(PresentationError::LegacyMutationRefused(
                "unknown presentation artifact",
            ));
        }
    }
    Ok(())
}

fn map_cleanup_probe_failure(failure: LegacyProbeFailure) -> PresentationError {
    match failure {
        LegacyProbeFailure::Inaccessible => PresentationError::LegacyMutationRefused(
            "presentation artifact could not be inspected safely",
        ),
        LegacyProbeFailure::Foreign => PresentationError::LegacyMutationRefused(
            "presentation artifact is foreign or symlinked",
        ),
        LegacyProbeFailure::Malformed => {
            PresentationError::LegacyMutationRefused("presentation artifact is malformed")
        }
    }
}

fn sync_directory(path: &Path) -> Result<(), PresentationError> {
    let directory = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(PresentationError::Io)?;
    directory.sync_all().map_err(PresentationError::Io)
}

/// A small deterministic evidence seam for focused presentation-proof tests.
/// It deliberately contains only bounded tmux/process identity, never pane
/// output.  Production code uses the private collector below.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPresentationEvidenceForTest {
    pub executable_path: PathBuf,
    pub config_identity: Option<LegacyFileIdentity>,
    pub session_id: Option<String>,
    pub window_id: Option<String>,
    pub panes: Vec<LegacyPresentationPaneEvidenceForTest>,
    pub clients: Vec<String>,
    pub shell_claim_present: bool,
    pub attachment_status: Option<LegacyAttachmentStatusForTest>,
}

/// Schema-12 attachment metadata accepted by the deterministic legacy proof
/// fixture. It is separate from the active host-local [`AttachmentStatus`].
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyAttachmentStatusForTest {
    pub attempt_id: uuid::Uuid,
    pub host_alias: String,
    pub workstream_id: WorkstreamId,
    pub phase: AttachmentPhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPresentationPaneEvidenceForTest {
    pub id: String,
    pub role: PresentationPaneRole,
    pub dead: bool,
    pub pid: Option<u32>,
    pub process_pid: Option<u32>,
    pub birth: Option<u64>,
    pub process_stable: bool,
    pub executable_path: Option<PathBuf>,
    pub executable_identity: Option<LegacyFileIdentity>,
    pub arguments: Vec<String>,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    pub window_width: u16,
    pub window_height: u16,
}

impl LegacyPresentationEvidenceForTest {
    fn into_internal(self) -> LegacyPresentationEvidence {
        let panes = self
            .panes
            .into_iter()
            .map(|pane| {
                let process =
                    pane.process_pid
                        .or(pane.pid)
                        .zip(pane.birth)
                        .and_then(|(pid, birth)| {
                            pane.executable_path.zip(pane.executable_identity).map(
                                |(path, identity)| LegacyProcessProof {
                                    pid,
                                    birth,
                                    executable: LegacyExecutableProof {
                                        path: normalize_deleted_executable_path(path),
                                        identity,
                                    },
                                    arguments: pane.arguments.clone(),
                                },
                            )
                        });
                LegacyPaneEvidence {
                    pane: LegacyOwnedPane {
                        id: pane.id,
                        role: pane.role,
                        host_alias: None,
                        workstream_id: None,
                        dead: pane.dead,
                        left: pane.left,
                        top: pane.top,
                        width: pane.width,
                        height: pane.height,
                    },
                    pid: pane.pid,
                    current_command: String::new(),
                    start_command: String::new(),
                    process,
                    process_stable: pane.process_stable,
                }
            })
            .collect();
        LegacyPresentationEvidence {
            directory: LegacyFileIdentity {
                size: 0,
                mode: 0o700,
                device: 0,
                inode: 0,
                digest: None,
            },
            socket: Some(LegacyFileIdentity {
                size: 0,
                mode: 0o600,
                device: 0,
                inode: 0,
                digest: None,
            }),
            config: self
                .config_identity
                .unwrap_or_else(expected_presentation_config_identity),
            attachment: self.attachment_status.as_ref().map(|_| LegacyFileIdentity {
                size: 0,
                mode: 0o600,
                device: 0,
                inode: 0,
                digest: None,
            }),
            attachment_status: self.attachment_status.map(|status| LegacyAttachmentStatus {
                attempt_id: status.attempt_id,
                host_alias: status.host_alias,
                workstream_id: status.workstream_id,
                phase: status.phase,
            }),
            session_id: self.session_id,
            window_id: self.window_id,
            panes,
            clients: self
                .clients
                .into_iter()
                .map(|name| LegacyClientProof {
                    name,
                    window_name: NAVIGATOR_WINDOW.to_owned(),
                })
                .collect(),
            shell_claim_present: self.shell_claim_present,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyProbeFailure {
    Malformed,
    Foreign,
    Inaccessible,
}

fn classify_fs_error(error: &std::io::Error) -> LegacyPresentationState {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied
        | std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::TimedOut => LegacyPresentationState::Inaccessible,
        _ => LegacyPresentationState::Malformed,
    }
}

fn inspect_private_presentation_root(
    state_root: &Path,
) -> Result<Option<PathBuf>, PresentationError> {
    let metadata = match fs::symlink_metadata(state_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PresentationError::Io(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(Some(state_root.join(PRESENTATION_DIRECTORY)));
    }
    let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
    let metadata = match fs::symlink_metadata(&presentation_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PresentationError::Io(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(Some(presentation_root));
    }
    if !is_private_owner_directory(&metadata) {
        return Ok(Some(presentation_root));
    }
    Ok(Some(presentation_root))
}

fn bounded_directory_entries(
    entries: fs::ReadDir,
) -> impl Iterator<Item = Result<fs::DirEntry, std::io::Error>> {
    entries.take(MAX_LEGACY_PRESENTATION_ENTRIES + 1)
}

#[allow(
    clippy::too_many_lines,
    reason = "The filesystem/tmux collector keeps the exact allowlist and bounded query sequence together."
)]
fn inspect_legacy_presentation(
    directory: &Path,
) -> Result<LegacyPresentationEvidence, LegacyProbeFailure> {
    let directory_metadata = fs::symlink_metadata(directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            LegacyProbeFailure::Inaccessible
        } else {
            LegacyProbeFailure::Foreign
        }
    })?;
    if directory_metadata.file_type().is_symlink() {
        return Err(LegacyProbeFailure::Foreign);
    }
    if !directory_metadata.is_dir() {
        return Err(LegacyProbeFailure::Malformed);
    }
    if !is_private_owner_directory(&directory_metadata) {
        return Err(LegacyProbeFailure::Foreign);
    }

    let mut names = BTreeSet::new();
    let entries =
        fs::read_dir(directory).map_err(|error| classify_fs_error(&error).into_probe())?;
    for entry in bounded_directory_entries(entries) {
        let entry = entry.map_err(|error| classify_fs_error(&error).into_probe())?;
        if !names.insert(entry.file_name()) {
            return Err(LegacyProbeFailure::Malformed);
        }
    }
    if names.len() > MAX_LEGACY_PRESENTATION_ENTRIES {
        return Err(LegacyProbeFailure::Malformed);
    }
    for name in &names {
        let known = name == "tmux.sock" || name == "tmux.conf" || name == ATTACHMENT_STATUS_FILE;
        if !known {
            return Err(LegacyProbeFailure::Malformed);
        }
    }

    let config = inspect_regular_file(&directory.join("tmux.conf"), true, MAX_LEGACY_CONFIG_BYTES)?
        .ok_or(LegacyProbeFailure::Malformed)?;
    let socket = inspect_private_socket(&directory.join("tmux.sock"))?;
    let (attachment, attachment_status) = match inspect_regular_file(
        &directory.join(ATTACHMENT_STATUS_FILE),
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    )? {
        Some(file) => {
            let bytes = read_private_file(
                &directory.join(ATTACHMENT_STATUS_FILE),
                MAX_ATTACHMENT_STATUS_BYTES_USIZE,
            )?
            .ok_or(LegacyProbeFailure::Inaccessible)?;
            let status = serde_json::from_slice::<LegacyAttachmentStatus>(&bytes)
                .map_err(|_| LegacyProbeFailure::Malformed)?;
            validate_legacy_host_alias(&status.host_alias)
                .map_err(|_| LegacyProbeFailure::Malformed)?;
            (Some(file), Some(status))
        }
        None => (None, None),
    };

    let mut evidence = LegacyPresentationEvidence {
        directory: legacy_file_identity(&directory_metadata, None),
        socket,
        config,
        attachment,
        attachment_status,
        session_id: None,
        window_id: None,
        panes: Vec::new(),
        clients: Vec::new(),
        shell_claim_present: false,
    };
    if evidence.socket.is_none() {
        return Ok(evidence);
    }
    let socket_path = directory.join("tmux.sock");
    let Some(session_output) = legacy_tmux_query(
        &socket_path,
        ["list-sessions", "-F", "#{session_name}\t#{session_id}"],
    )?
    else {
        return Ok(evidence);
    };
    let sessions = parse_session_rows(&session_output)?;
    if sessions.is_empty() {
        return Ok(evidence);
    }
    if sessions.len() != 1 {
        return Err(LegacyProbeFailure::Malformed);
    }
    let (session_name, session_id) = &sessions[0];
    let expected_session = presentation_session_name(directory)
        .ok_or(LegacyProbeFailure::Malformed)?
        .clone();
    if session_name != &expected_session {
        return Err(LegacyProbeFailure::Foreign);
    }
    evidence.session_id = Some(session_id.clone());

    // An attached client is the strongest refusal signal.  Capture it before
    // probing pane topology so a dead/malformed pane cannot turn an attached
    // presentation into cleanup-eligible evidence.  The client query is
    // exact: every row must name this session and the navigator window.
    let clients = legacy_tmux_query(
        &socket_path,
        [
            "list-clients",
            "-F",
            "#{client_name}\t#{session_name}\t#{window_name}",
        ],
    )?
    .ok_or(LegacyProbeFailure::Inaccessible)?;
    evidence.clients = parse_client_rows(&clients, session_name)?;
    let attached = !evidence.clients.is_empty();

    let windows_output = match legacy_tmux_query(
        &socket_path,
        [
            "list-windows",
            "-t",
            session_name.as_str(),
            "-F",
            "#{window_name}\t#{window_id}",
        ],
    ) {
        Ok(Some(output)) => output,
        Ok(None) | Err(_) if attached => return Ok(evidence),
        Ok(None) => return Err(LegacyProbeFailure::Inaccessible),
        Err(error) => return Err(error),
    };
    let windows = match parse_window_rows(&windows_output) {
        Ok(windows) => windows,
        Err(_) if attached => return Ok(evidence),
        Err(error) => return Err(error),
    };
    if windows.len() != 1 {
        if attached {
            return Ok(evidence);
        }
        return Err(LegacyProbeFailure::Malformed);
    }
    let (window_name, window_id) = &windows[0];
    if window_name != NAVIGATOR_WINDOW {
        if attached {
            return Ok(evidence);
        }
        return Err(LegacyProbeFailure::Foreign);
    }
    evidence.window_id = Some(window_id.clone());

    let panes_output = match legacy_tmux_query(
        &socket_path,
        [
            "list-panes",
            "-t",
            format!("{session_name}:{NAVIGATOR_WINDOW}").as_str(),
            "-F",
            LEGACY_PROOF_TOPOLOGY_FORMAT,
        ],
    ) {
        Ok(Some(output)) => output,
        Ok(None) | Err(_) if attached => return Ok(evidence),
        Ok(None) => return Err(LegacyProbeFailure::Inaccessible),
        Err(error) => return Err(error),
    };
    evidence.panes = match parse_legacy_panes(&panes_output) {
        Ok(panes) => panes,
        Err(_) if attached => return Ok(evidence),
        Err(error) => return Err(error),
    };
    if !(2..=MAX_LEGACY_PANES).contains(&evidence.panes.len()) {
        if attached {
            return Ok(evidence);
        }
        return Err(LegacyProbeFailure::Malformed);
    }
    let legacy_topology_panes = evidence
        .panes
        .iter()
        .map(|pane| pane.pane.clone())
        .collect::<Vec<_>>();
    let (window_width, window_height) = legacy_topology_dimensions(&legacy_topology_panes);
    let topology_panes = legacy_topology_panes
        .iter()
        .map(|pane| OwnedPane {
            id: pane.id.clone(),
            role: pane.role,
            workstream_id: pane.workstream_id,
            dead: pane.dead,
            left: pane.left,
            top: pane.top,
            width: pane.width,
            height: pane.height,
        })
        .collect::<Vec<_>>();
    let topology = PresentationTopology {
        panes: topology_panes,
        window_width,
        window_height,
    };
    if validate_topology_shape(&topology).is_err() {
        if attached {
            return Ok(evidence);
        }
        return Err(LegacyProbeFailure::Malformed);
    }

    let claim = match legacy_tmux_query(&socket_path, ["show-options", "-gqv", SHELL_CLAIM_OPTION])
    {
        Ok(Some(claim)) => claim,
        Ok(None) => String::new(),
        Err(_) if attached => return Ok(evidence),
        Err(error) => return Err(error),
    };
    evidence.shell_claim_present = !claim.trim().is_empty();
    Ok(evidence)
}

fn inspect_regular_file(
    path: &Path,
    required: bool,
    max_bytes: usize,
) -> Result<Option<LegacyFileIdentity>, LegacyProbeFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => return Ok(None),
        Err(error) => return Err(classify_fs_error(&error).into_probe()),
    };
    if metadata.file_type().is_symlink() {
        return Err(LegacyProbeFailure::Foreign);
    }
    if !metadata.is_file() {
        return Err(LegacyProbeFailure::Malformed);
    }
    if !is_private_owner_file(&metadata) {
        return Err(LegacyProbeFailure::Foreign);
    }
    let bytes = read_private_file(path, max_bytes)?.ok_or(LegacyProbeFailure::Inaccessible)?;
    Ok(Some(legacy_file_identity(&metadata, Some(&bytes))))
}

fn inspect_private_socket(path: &Path) -> Result<Option<LegacyFileIdentity>, LegacyProbeFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(classify_fs_error(&error).into_probe()),
    };
    if metadata.file_type().is_symlink() {
        return Err(LegacyProbeFailure::Foreign);
    }
    if !file_type_is_socket(&metadata) {
        return Err(LegacyProbeFailure::Malformed);
    }
    if !is_private_owner_socket(&metadata) {
        return Err(LegacyProbeFailure::Foreign);
    }
    Ok(Some(legacy_file_identity(&metadata, None)))
}

fn read_private_file(path: &Path, maximum: usize) -> Result<Option<Vec<u8>>, LegacyProbeFailure> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(classify_fs_error(&error).into_probe()),
    };
    let mut bytes = Vec::new();
    file.take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| classify_fs_error(&error).into_probe())?;
    if bytes.len() > maximum {
        return Err(LegacyProbeFailure::Malformed);
    }
    Ok(Some(bytes))
}

fn legacy_file_identity(metadata: &fs::Metadata, bytes: Option<&[u8]>) -> LegacyFileIdentity {
    LegacyFileIdentity {
        size: metadata.len(),
        mode: file_mode(metadata),
        device: file_device(metadata),
        inode: file_inode(metadata),
        digest: bytes.map(|bytes| {
            let mut digest = Sha256::new();
            digest.update(bytes);
            digest.finalize().into()
        }),
    }
}

fn is_private_owner_directory(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.mode() & 0o777) == 0o700 && metadata.uid() == nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn file_type_is_socket(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        metadata.file_type().is_socket()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn is_private_owner_file(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.mode() & 0o777) == 0o600 && metadata.uid() == nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn private_socket_mode(mode: u32) -> bool {
    matches!(mode, 0o600 | 0o700)
}

fn is_private_owner_socket(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        private_socket_mode(metadata.mode() & 0o777)
            && metadata.uid() == nix::unistd::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn file_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn file_device(metadata: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.dev()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn file_inode(metadata: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn executable_matches(left: &LegacyExecutableProof, right: &LegacyExecutableProof) -> bool {
    // The inode/device identity is authoritative.  The `/proc/<pid>/exe`
    // readlink is only a display hint and may differ after an upgrade has
    // unlinked the old executable.
    left.identity == right.identity
}

fn normalize_deleted_executable_path(path: PathBuf) -> PathBuf {
    const DELETED_SUFFIX: &str = " (deleted)";
    let Some(value) = path.to_str() else {
        return path;
    };
    value
        .strip_suffix(DELETED_SUFFIX)
        .map_or_else(|| path.clone(), PathBuf::from)
}

fn process_executable_proof(
    proc_executable: &Path,
    display_path: PathBuf,
) -> Result<LegacyExecutableProof, LegacyProcessFailure> {
    let metadata = fs::metadata(proc_executable).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            LegacyProcessFailure::Gone
        } else {
            LegacyProcessFailure::Inaccessible
        }
    })?;
    if !metadata.is_file() {
        return Err(LegacyProcessFailure::Malformed);
    }
    Ok(LegacyExecutableProof {
        path: normalize_deleted_executable_path(display_path),
        identity: legacy_file_identity(&metadata, None),
    })
}

fn legacy_tmux_query<const N: usize>(
    socket: &Path,
    arguments: [&str; N],
) -> Result<Option<String>, LegacyProbeFailure> {
    let mut command = private_tmux_command();
    command
        .args(["-f", "/dev/null", "-S"])
        .arg(socket)
        .args(arguments);
    let output = output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
        .map_err(|_| LegacyProbeFailure::Inaccessible)?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|_| LegacyProbeFailure::Malformed);
    }
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    if diagnostic.contains("no server running")
        || diagnostic.contains("No such file")
        || diagnostic.contains("no sessions")
    {
        return Ok(None);
    }
    Err(LegacyProbeFailure::Inaccessible)
}

fn parse_session_rows(output: &str) -> Result<Vec<(String, String)>, LegacyProbeFailure> {
    let mut rows = Vec::new();
    for line in output.lines() {
        if line.is_empty() || rows.len() >= 2 {
            return Err(LegacyProbeFailure::Malformed);
        }
        let mut fields = line.split('\t');
        let Some(name) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        let Some(id) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        if fields.next().is_some()
            || name.is_empty()
            || name.chars().any(char::is_control)
            || !parse_session_id(id)
        {
            return Err(LegacyProbeFailure::Malformed);
        }
        rows.push((name.to_owned(), id.to_owned()));
    }
    Ok(rows)
}

fn parse_window_rows(output: &str) -> Result<Vec<(String, String)>, LegacyProbeFailure> {
    let mut rows = Vec::new();
    for line in output.lines() {
        if line.is_empty() || rows.len() >= 2 {
            return Err(LegacyProbeFailure::Malformed);
        }
        let mut fields = line.split('\t');
        let Some(name) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        let Some(id) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        if fields.next().is_some() || name.is_empty() || !parse_window_id(id) {
            return Err(LegacyProbeFailure::Malformed);
        }
        rows.push((name.to_owned(), id.to_owned()));
    }
    Ok(rows)
}

fn parse_client_rows(
    output: &str,
    expected_session: &str,
) -> Result<Vec<LegacyClientProof>, LegacyProbeFailure> {
    let mut clients = Vec::new();
    for line in output.lines() {
        if line.is_empty() || clients.len() >= MAX_LEGACY_CLIENTS {
            return Err(LegacyProbeFailure::Malformed);
        }
        let mut fields = line.split('\t');
        let Some(name) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        let Some(session) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        let Some(window_name) = fields.next() else {
            return Err(LegacyProbeFailure::Malformed);
        };
        if fields.next().is_some()
            || name.is_empty()
            || session != expected_session
            || window_name != NAVIGATOR_WINDOW
        {
            return Err(LegacyProbeFailure::Malformed);
        }
        clients.push(LegacyClientProof {
            name: name.to_owned(),
            window_name: window_name.to_owned(),
        });
    }
    clients.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(clients)
}

fn parse_session_id(value: &str) -> bool {
    value.strip_prefix('$').is_some_and(|digits| {
        !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
    })
}

fn parse_legacy_panes(output: &str) -> Result<Vec<LegacyPaneEvidence>, LegacyProbeFailure> {
    let mut panes = Vec::new();
    let mut window_size = None;
    for line in output.lines() {
        if line.is_empty() || panes.len() >= MAX_LEGACY_PANES {
            return Err(LegacyProbeFailure::Malformed);
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 15 {
            return Err(LegacyProbeFailure::Malformed);
        }
        let base_line = [
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[9], fields[10],
            fields[11], fields[12], fields[13], fields[14],
        ]
        .join("\t");
        let existing_panes = panes
            .iter()
            .map(|pane: &LegacyPaneEvidence| pane.pane.clone())
            .collect::<Vec<_>>();
        let pane = parse_legacy_topology_line(&base_line, true, &mut window_size, &existing_panes)
            .map_err(|_| LegacyProbeFailure::Malformed)?;
        if fields[5] == "0" {
            return Err(LegacyProbeFailure::Malformed);
        }
        let pid = fields[5]
            .parse::<u32>()
            .map_err(|_| LegacyProbeFailure::Malformed)?;
        for value in [fields[6], fields[7], fields[8]] {
            if value.chars().any(char::is_control) || value.len() > 1024 {
                return Err(LegacyProbeFailure::Malformed);
            }
        }
        let process = if pane.dead {
            None
        } else {
            stable_process_proof(pid).map_err(|error| match error {
                LegacyProcessFailure::Gone | LegacyProcessFailure::Malformed => {
                    LegacyProbeFailure::Malformed
                }
                LegacyProcessFailure::Inaccessible => LegacyProbeFailure::Inaccessible,
            })?
        };
        panes.push(LegacyPaneEvidence {
            pane,
            pid: Some(pid),
            current_command: fields[6].to_owned(),
            start_command: fields[7].to_owned(),
            process,
            process_stable: true,
        });
    }
    if panes.is_empty() {
        return Err(LegacyProbeFailure::Malformed);
    }
    if panes
        .iter()
        .filter(|pane| pane.pane.role == PresentationPaneRole::Navigator)
        .count()
        != 1
        || panes
            .iter()
            .filter(|pane| pane.pane.role == PresentationPaneRole::Provider)
            .count()
            != 1
        || panes
            .iter()
            .filter(|pane| pane.pane.role == PresentationPaneRole::Utility)
            .count()
            > 1
    {
        return Err(LegacyProbeFailure::Malformed);
    }
    Ok(panes)
}

fn legacy_topology_dimensions(panes: &[LegacyOwnedPane]) -> (u16, u16) {
    let window_width = panes
        .iter()
        .map(|pane| pane.left.saturating_add(pane.width))
        .max()
        .unwrap_or(0);
    let window_height = panes
        .iter()
        .map(|pane| pane.top.saturating_add(pane.height))
        .max()
        .unwrap_or(0);
    (window_width, window_height)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyProcessFailure {
    Gone,
    Inaccessible,
    Malformed,
}

fn stable_process_proof(pid: u32) -> Result<Option<LegacyProcessProof>, LegacyProcessFailure> {
    let first = read_process_proof(pid)?;
    let Some(first) = first else {
        return Ok(None);
    };
    let second = read_process_proof(pid)?.ok_or(LegacyProcessFailure::Gone)?;
    if first != second {
        return Err(LegacyProcessFailure::Gone);
    }
    Ok(Some(first))
}

fn read_process_proof(pid: u32) -> Result<Option<LegacyProcessProof>, LegacyProcessFailure> {
    #[cfg(target_os = "linux")]
    {
        let proc_root = Path::new("/proc");
        let process_root = proc_root.join(pid.to_string());
        let stat = match fs::read_to_string(process_root.join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(LegacyProcessFailure::Inaccessible),
        };
        let birth = parse_process_birth(&stat)?;
        let proc_executable = process_root.join("exe");
        let executable_link = match fs::read_link(&proc_executable) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(LegacyProcessFailure::Inaccessible),
        };
        let executable = process_executable_proof(&proc_executable, executable_link)?;
        let bytes = match fs::read(process_root.join("cmdline")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(LegacyProcessFailure::Inaccessible),
        };
        if bytes.len() > MAX_LEGACY_PROCESS_ARGUMENT_BYTES {
            return Err(LegacyProcessFailure::Malformed);
        }
        let mut arguments = Vec::new();
        for argument in bytes
            .split(|byte| *byte == 0)
            .filter(|argument| !argument.is_empty())
        {
            let argument =
                std::str::from_utf8(argument).map_err(|_| LegacyProcessFailure::Malformed)?;
            if argument.chars().any(char::is_control) {
                return Err(LegacyProcessFailure::Malformed);
            }
            arguments.push(argument.to_owned());
        }
        if arguments.is_empty() {
            return Err(LegacyProcessFailure::Malformed);
        }
        Ok(Some(LegacyProcessProof {
            pid,
            birth,
            executable,
            arguments,
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err(LegacyProcessFailure::Inaccessible)
    }
}

fn parse_process_birth(stat: &str) -> Result<u64, LegacyProcessFailure> {
    let Some(close_paren) = stat.rfind(')') else {
        return Err(LegacyProcessFailure::Malformed);
    };
    let fields = stat
        .get(close_paren + 2..)
        .ok_or(LegacyProcessFailure::Malformed)?
        .split_whitespace()
        .collect::<Vec<_>>();
    fields
        .get(19)
        .ok_or(LegacyProcessFailure::Malformed)?
        .parse::<u64>()
        .map_err(|_| LegacyProcessFailure::Malformed)
}

#[allow(
    clippy::too_many_lines,
    reason = "The pure classifier orders each independent fail-closed proof check before deriving the launcher state."
)]
fn classify_legacy_evidence_internal(
    directory: &Path,
    state_root: &Path,
    evidence: &LegacyPresentationEvidence,
) -> LegacyPresentationAssessment {
    let socket = directory.join("tmux.sock");
    let session_name = presentation_session_name(directory).unwrap_or_default();
    let mut proof = LegacyPresentationProof {
        directory: directory.to_path_buf(),
        directory_identity: evidence.directory,
        socket,
        socket_identity: evidence.socket,
        config_identity: evidence.config,
        attachment_identity: evidence.attachment,
        session_name: session_name.clone(),
        session_id: evidence.session_id.clone(),
        window_id: evidence.window_id.clone(),
        navigator: None,
        provider: None,
        utility: None,
        clients: evidence.clients.clone(),
        shell_claim_present: evidence.shell_claim_present,
        legacy_executable: None,
        attachment_status: evidence.attachment_status.clone(),
    };

    if !config_content_matches(&evidence.config) {
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    if evidence.socket.is_none() || evidence.session_id.is_none() {
        return LegacyPresentationAssessment {
            state: LegacyPresentationState::DeadOwned,
            proof: Some(proof),
        };
    }
    // Exact attached-client evidence outranks every pane-local failure.  A
    // client may remain attached while a pane is dead or its private options
    // are no longer readable; that presentation must never be mistaken for
    // detached cleanup-eligible ownership.  When all pane evidence is
    // available we still prove the navigator/controller so a later drain
    // attach can require the strongest evidence without downgrading this
    // refusal when a pane is malformed.
    let attached = !proof.clients.is_empty();
    if evidence.shell_claim_present && !attached {
        return LegacyPresentationAssessment {
            state: LegacyPresentationState::Malformed,
            proof: None,
        };
    }
    let Some(navigator) = evidence
        .panes
        .iter()
        .find(|pane| pane.pane.role == PresentationPaneRole::Navigator)
    else {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Malformed);
    };
    let Some(provider) = evidence
        .panes
        .iter()
        .find(|pane| pane.pane.role == PresentationPaneRole::Provider)
    else {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Malformed);
    };
    if navigator.pane.dead || provider.pane.dead {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment {
            state: LegacyPresentationState::DeadOwned,
            proof: Some(proof),
        };
    }
    let Some(navigator_process) = &navigator.process else {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Inaccessible);
    };
    let expected_navigator_arguments = [
        "--state-root",
        state_root.to_str().unwrap_or_default(),
        "_navigator",
        "--presentation-socket",
        proof.socket.to_str().unwrap_or_default(),
        "--presentation-session",
        session_name.as_str(),
    ];
    if !navigator.process_stable
        || navigator_process.pid != navigator.pid.unwrap_or_default()
        || !arguments_after_executable_match(
            &navigator_process.arguments,
            &expected_navigator_arguments,
        )
    {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    proof.navigator = Some(LegacyPaneProof {
        id: navigator.pane.id.clone(),
        role: navigator.pane.role,
        dead: navigator.pane.dead,
        process: Some(navigator_process.clone()),
        command: LegacyPaneCommand::Navigator,
    });
    // The exact, stable navigator process establishes the legacy controller
    // executable.  This intentionally does not compare it with the current
    // D16 executable: an upgrade may leave the old inode running in place.
    let legacy_executable = navigator_process.executable.clone();
    proof.legacy_executable = Some(legacy_executable.clone());

    let Some(provider_process) = &provider.process else {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    };
    if !provider.process_stable
        || provider_process.pid != provider.pid.unwrap_or_default()
        || !executable_matches(&provider_process.executable, &legacy_executable)
    {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    let provider_command = classify_provider_command(
        provider_process,
        state_root,
        &proof.socket,
        &session_name,
        &legacy_executable,
    );
    if matches!(provider_command, LegacyPaneCommand::Other) {
        if attached {
            return attached_assessment(proof);
        }
        return LegacyPresentationAssessment::classified(LegacyPresentationState::Foreign);
    }
    proof.provider = Some(LegacyPaneProof {
        id: provider.pane.id.clone(),
        role: provider.pane.role,
        dead: provider.pane.dead,
        process: provider.process.clone(),
        command: provider_command,
    });
    if let Some(utility) = evidence
        .panes
        .iter()
        .find(|pane| pane.pane.role == PresentationPaneRole::Utility)
    {
        if utility.pane.dead {
            if attached {
                return attached_assessment(proof);
            }
            return LegacyPresentationAssessment {
                state: LegacyPresentationState::DeadOwned,
                proof: Some(proof),
            };
        }
        proof.utility = Some(LegacyPaneProof {
            id: utility.pane.id.clone(),
            role: utility.pane.role,
            dead: utility.pane.dead,
            process: utility.process.clone(),
            command: classify_utility_command(utility.process.as_ref()),
        });
    }

    let state = if attached {
        LegacyPresentationState::Attached
    } else if proof.utility.is_some() {
        LegacyPresentationState::UtilityShell
    } else if provider_command == LegacyPaneCommand::ObserverReview {
        LegacyPresentationState::ObserverReview
    } else {
        LegacyPresentationState::DetachedOrdinary
    };
    LegacyPresentationAssessment {
        state,
        proof: Some(proof),
    }
}

fn attached_assessment(proof: LegacyPresentationProof) -> LegacyPresentationAssessment {
    LegacyPresentationAssessment {
        state: LegacyPresentationState::Attached,
        proof: Some(proof),
    }
}

fn arguments_after_executable_match(arguments: &[String], expected: &[&str]) -> bool {
    arguments.len() == expected.len() + 1
        && arguments.get(1..).is_some_and(|actual| {
            actual
                .iter()
                .map(String::as_str)
                .eq(expected.iter().copied())
        })
}

fn classify_provider_command(
    process: &LegacyProcessProof,
    state_root: &Path,
    socket: &Path,
    session_name: &str,
    executable: &LegacyExecutableProof,
) -> LegacyPaneCommand {
    let (Some(expected_root), Some(expected_socket)) = (state_root.to_str(), socket.to_str())
    else {
        return LegacyPaneCommand::Other;
    };
    let same_executable = executable_matches(&process.executable, executable);
    let helper = |name: &str| {
        same_executable
            && arguments_after_executable_match(
                &process.arguments,
                &["--state-root", expected_root, name],
            )
    };
    let legacy_remote_observer = same_executable
        && process.arguments.len() == 5
        && process.arguments.get(1).map(String::as_str) == Some("--state-root")
        && process.arguments.get(2).map(String::as_str) == Some(expected_root)
        && process.arguments.get(3).map(String::as_str) == Some("_provider_remote_observer_review")
        && process
            .arguments
            .get(4)
            .is_some_and(|alias| !alias.is_empty() && !alias.chars().any(char::is_control));
    if helper("_observer_review") || legacy_remote_observer {
        LegacyPaneCommand::ObserverReview
    } else if helper("_provider_wait") {
        LegacyPaneCommand::ProviderWait
    } else if exact_provider_attach_arguments(
        process,
        expected_root,
        expected_socket,
        session_name,
        same_executable,
    ) {
        LegacyPaneCommand::ProviderAttach
    } else {
        LegacyPaneCommand::Other
    }
}

fn exact_provider_attach_arguments(
    process: &LegacyProcessProof,
    expected_root: &str,
    expected_socket: &str,
    expected_session: &str,
    same_executable: bool,
) -> bool {
    let arguments = &process.arguments;
    same_executable
        && arguments.len() == 11
        && arguments.get(1).map(String::as_str) == Some("--state-root")
        && arguments.get(2).map(String::as_str) == Some(expected_root)
        && arguments.get(3).map(String::as_str) == Some("_provider_attach")
        && arguments
            .get(4)
            .is_some_and(|value| WorkstreamId::from_str(value).is_ok())
        && arguments.get(5).map(String::as_str) == Some("--presentation-socket")
        && arguments.get(6).map(String::as_str) == Some(expected_socket)
        && arguments.get(7).map(String::as_str) == Some("--presentation-session")
        && arguments.get(8).map(String::as_str) == Some(expected_session)
        && arguments.get(9).map(String::as_str) == Some("--attempt-id")
        && arguments
            .get(10)
            .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok())
}

fn classify_utility_command(process: Option<&LegacyProcessProof>) -> LegacyPaneCommand {
    let Some(process) = process else {
        return LegacyPaneCommand::Other;
    };
    if process
        .arguments
        .iter()
        .any(|argument| argument == "_presentation_shell")
    {
        LegacyPaneCommand::PresentationShell
    } else {
        LegacyPaneCommand::Other
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OwnedPane {
    id: String,
    role: PresentationPaneRole,
    workstream_id: Option<WorkstreamId>,
    dead: bool,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresentationTopology {
    panes: Vec<OwnedPane>,
    window_width: u16,
    window_height: u16,
}

impl PresentationTopology {
    fn pane(&self, id: &str) -> Option<&OwnedPane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    fn navigator(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Navigator)
    }

    fn provider(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Provider)
    }

    fn utility(&self) -> Option<&OwnedPane> {
        self.panes
            .iter()
            .find(|pane| pane.role == PresentationPaneRole::Utility)
    }

    fn next(&self, source: &OwnedPane) -> Option<&OwnedPane> {
        let mut panes: Vec<&OwnedPane> = self.panes.iter().collect();
        panes.sort_by_key(|pane| (pane.top, pane.left, pane.id.as_str()));
        let index = panes.iter().position(|pane| pane.id == source.id)?;
        panes.get((index + 1) % panes.len()).copied()
    }

    fn directional(&self, source: &OwnedPane, direction: Direction) -> Option<&OwnedPane> {
        let source_x = i32::from(source.left) + i32::from(source.width) / 2;
        let source_y = i32::from(source.top) + i32::from(source.height) / 2;
        let mut candidates: Vec<(&OwnedPane, (i32, i32))> = self
            .panes
            .iter()
            .filter(|pane| pane.id != source.id)
            .filter_map(|pane| {
                let pane_x = i32::from(pane.left) + i32::from(pane.width) / 2;
                let pane_y = i32::from(pane.top) + i32::from(pane.height) / 2;
                let (primary, secondary) = match direction {
                    Direction::Up if pane_y < source_y => {
                        (source_y - pane_y, (source_x - pane_x).abs())
                    }
                    Direction::Down if pane_y > source_y => {
                        (pane_y - source_y, (source_x - pane_x).abs())
                    }
                    Direction::Left if pane_x < source_x => {
                        (source_x - pane_x, (source_y - pane_y).abs())
                    }
                    Direction::Right if pane_x > source_x => {
                        (pane_x - source_x, (source_y - pane_y).abs())
                    }
                    _ => return None,
                };
                Some((pane, (primary, secondary)))
            })
            .collect();
        candidates.sort_by_key(|(pane, distance)| (*distance, pane.id.as_str()));
        candidates.first().map(|(pane, _)| *pane)
    }
}

fn parse_topology(output: &str) -> Result<PresentationTopology, PresentationError> {
    parse_topology_with_dead(output, false)
}

fn parse_topology_with_dead(
    output: &str,
    allow_dead: bool,
) -> Result<PresentationTopology, PresentationError> {
    let mut panes = Vec::new();
    let mut window_size = None;
    for line in output.lines() {
        panes.push(parse_topology_line(
            line,
            allow_dead,
            &mut window_size,
            &panes,
        )?);
    }
    if !(2..=3).contains(&panes.len()) {
        return Err(PresentationError::InvalidTopology);
    }
    let (window_width, window_height) = window_size.ok_or(PresentationError::InvalidTopology)?;
    let topology = PresentationTopology {
        panes,
        window_width,
        window_height,
    };
    validate_topology_shape(&topology)?;
    Ok(topology)
}

fn parse_topology_line(
    line: &str,
    allow_dead: bool,
    window_size: &mut Option<(u16, u16)>,
    panes: &[OwnedPane],
) -> Result<OwnedPane, PresentationError> {
    if line.is_empty() {
        return Err(PresentationError::InvalidTopology);
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 10 {
        return Err(PresentationError::InvalidTopology);
    }
    let id = parse_pane_id(fields[0]).ok_or(PresentationError::InvalidTopology)?;
    if panes.iter().any(|pane| pane.id == id) {
        return Err(PresentationError::InvalidTopology);
    }
    let role = match fields[1] {
        "navigator" => PresentationPaneRole::Navigator,
        "provider" => PresentationPaneRole::Provider,
        "utility" => PresentationPaneRole::Utility,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let workstream_id = if fields[2].is_empty() {
        None
    } else {
        Some(
            fields[2]
                .parse()
                .map_err(|_| PresentationError::InvalidTopology)?,
        )
    };
    if (role == PresentationPaneRole::Navigator && workstream_id.is_some())
        || (role == PresentationPaneRole::Utility && workstream_id.is_none())
    {
        return Err(PresentationError::InvalidTopology);
    }
    let dead = match fields[3] {
        "0" => false,
        "1" if allow_dead => true,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let window_width = topology_dimension(fields[8])?;
    let window_height = topology_dimension(fields[9])?;
    if window_width == 0 || window_height == 0 {
        return Err(PresentationError::InvalidTopology);
    }
    if let Some((expected_width, expected_height)) = window_size {
        if (*expected_width, *expected_height) != (window_width, window_height) {
            return Err(PresentationError::InvalidTopology);
        }
    } else {
        *window_size = Some((window_width, window_height));
    }
    let left = topology_dimension(fields[4])?;
    let top = topology_dimension(fields[5])?;
    let width = topology_dimension(fields[6])?;
    let height = topology_dimension(fields[7])?;
    if width == 0
        || height == 0
        || u32::from(left) + u32::from(width) > u32::from(window_width)
        || u32::from(top) + u32::from(height) > u32::from(window_height)
    {
        return Err(PresentationError::InvalidTopology);
    }
    Ok(OwnedPane {
        id,
        role,
        workstream_id,
        dead,
        left,
        top,
        width,
        height,
    })
}

fn topology_dimension(value: &str) -> Result<u16, PresentationError> {
    value
        .parse::<u16>()
        .map_err(|_| PresentationError::InvalidTopology)
}

/// Parses the schema-12 topology format used only by the explicit cutover
/// proof collector. Host identity is retained in this private shape and is
/// never exposed through the active [`OwnedPane`] topology.
fn parse_legacy_topology_line(
    line: &str,
    allow_dead: bool,
    window_size: &mut Option<(u16, u16)>,
    panes: &[LegacyOwnedPane],
) -> Result<LegacyOwnedPane, PresentationError> {
    if line.is_empty() {
        return Err(PresentationError::InvalidTopology);
    }
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 11 {
        return Err(PresentationError::InvalidTopology);
    }
    let id = parse_pane_id(fields[0]).ok_or(PresentationError::InvalidTopology)?;
    if panes.iter().any(|pane| pane.id == id) {
        return Err(PresentationError::InvalidTopology);
    }
    let role = match fields[1] {
        "navigator" => PresentationPaneRole::Navigator,
        "provider" => PresentationPaneRole::Provider,
        "utility" => PresentationPaneRole::Utility,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let dead = match fields[4] {
        "0" => false,
        "1" if allow_dead => true,
        _ => return Err(PresentationError::InvalidTopology),
    };
    let host_alias = if fields[2].is_empty() {
        None
    } else {
        validate_legacy_host_alias(fields[2])?;
        Some(fields[2].to_owned())
    };
    let workstream_id = if fields[3].is_empty() {
        None
    } else {
        Some(
            fields[3]
                .parse()
                .map_err(|_| PresentationError::InvalidTopology)?,
        )
    };
    if (role == PresentationPaneRole::Navigator
        && (host_alias.is_some() || workstream_id.is_some()))
        || (role == PresentationPaneRole::Utility
            && (host_alias.is_none() || workstream_id.is_none()))
        || (role == PresentationPaneRole::Provider
            && host_alias.is_some() != workstream_id.is_some())
    {
        return Err(PresentationError::InvalidTopology);
    }
    let window_width = topology_dimension(fields[9])?;
    let window_height = topology_dimension(fields[10])?;
    if window_width == 0 || window_height == 0 {
        return Err(PresentationError::InvalidTopology);
    }
    if let Some((expected_width, expected_height)) = window_size {
        if (*expected_width, *expected_height) != (window_width, window_height) {
            return Err(PresentationError::InvalidTopology);
        }
    } else {
        *window_size = Some((window_width, window_height));
    }
    let left = topology_dimension(fields[5])?;
    let top = topology_dimension(fields[6])?;
    let width = topology_dimension(fields[7])?;
    let height = topology_dimension(fields[8])?;
    if width == 0
        || height == 0
        || u32::from(left) + u32::from(width) > u32::from(window_width)
        || u32::from(top) + u32::from(height) > u32::from(window_height)
    {
        return Err(PresentationError::InvalidTopology);
    }
    Ok(LegacyOwnedPane {
        id,
        role,
        host_alias,
        workstream_id,
        dead,
        left,
        top,
        width,
        height,
    })
}

fn prepare_attach_window_with_size<F>(
    session_name: &str,
    columns: u16,
    rows: u16,
    mut invoke: F,
) -> Result<(), PresentationError>
where
    F: FnMut(Vec<OsString>) -> Result<(), PresentationError>,
{
    if columns == 0 || rows == 0 {
        return Err(PresentationError::InvalidTerminalGeometry);
    }
    let target = format!("{session_name}:{NAVIGATOR_WINDOW}");
    invoke(vec![
        "resize-window".into(),
        "-t".into(),
        target.clone().into(),
        "-x".into(),
        columns.to_string().into(),
        "-y".into(),
        rows.to_string().into(),
    ])?;
    invoke(vec![
        "set-window-option".into(),
        "-t".into(),
        target.into(),
        "window-size".into(),
        "latest".into(),
    ])
}

fn validate_topology_shape(topology: &PresentationTopology) -> Result<(), PresentationError> {
    if topology.navigator().is_none()
        || topology.provider().is_none()
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Navigator)
            .count()
            != 1
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Provider)
            .count()
            != 1
        || topology
            .panes
            .iter()
            .filter(|pane| pane.role == PresentationPaneRole::Utility)
            .count()
            > 1
    {
        return Err(PresentationError::InvalidTopology);
    }
    let navigator = topology
        .navigator()
        .ok_or(PresentationError::InvalidTopology)?;
    let provider = topology
        .provider()
        .ok_or(PresentationError::InvalidTopology)?;
    if navigator.left != 0
        || navigator.top != 0
        || navigator.height != topology.window_height
        || provider.top != 0
        || provider.left
            != navigator
                .left
                .saturating_add(navigator.width)
                .saturating_add(1)
        || provider.left <= navigator.left
        || u32::from(provider.left) + u32::from(provider.width) != u32::from(topology.window_width)
    {
        return Err(PresentationError::InvalidTopology);
    }
    match topology.utility() {
        None if provider.height == topology.window_height => {}
        Some(utility)
            if provider.height < topology.window_height
                && utility.left == provider.left
                && utility.width == provider.width
                && u32::from(utility.top)
                    == u32::from(provider.top) + u32::from(provider.height) + 1
                && u32::from(utility.top) + u32::from(utility.height)
                    == u32::from(topology.window_height) => {}
        _ => return Err(PresentationError::InvalidTopology),
    }
    Ok(())
}

fn validate_observer_review_topology(
    topology: &PresentationTopology,
) -> Result<String, PresentationError> {
    validate_topology_shape(topology)?;
    if topology.utility().is_some() {
        return Err(PresentationError::ControlRefused(
            "observer review requires an exact two-pane presentation",
        ));
    }
    topology
        .provider()
        .map(|pane| pane.id.clone())
        .ok_or(PresentationError::InvalidTopology)
}

fn parse_pane_id(value: &str) -> Option<String> {
    value
        .strip_prefix('%')
        .filter(|digits| {
            !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
        })
        .map(|_| value.to_owned())
}

fn parse_window_id(value: &str) -> bool {
    value.strip_prefix('@').is_some_and(|digits| {
        !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
    })
}

fn validate_shell_path(path: &Path) -> Result<(), PresentationError> {
    let value = path
        .to_str()
        .ok_or_else(|| PresentationError::InvalidControlPath(path.to_path_buf()))?;
    if !path.is_absolute()
        || value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(PresentationError::ControlRefused(
            "ordinary shell path is invalid",
        ));
    }
    Ok(())
}

fn shell_quote(value: &std::ffi::OsStr) -> Result<String, PresentationError> {
    let value = value
        .to_str()
        .ok_or_else(|| PresentationError::InvalidControlPath(PathBuf::from("non-UTF-8")))?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(PresentationError::ControlRefused(
            "presentation control path contains an invalid character",
        ));
    }
    // tmux expands format directives before invoking the shell used by
    // `run-shell`; POSIX quoting alone does not protect `#{...}` or `#(...)`.
    // A doubled hash is tmux's literal-hash escape. The source pane format is
    // intentionally emitted separately below and remains the only live
    // expansion.
    let value = value.replace('#', "##");
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn close_shell_arguments(client_name: &str, utility_pane: &str) -> Vec<OsString> {
    vec![
        "confirm-before".into(),
        "-t".into(),
        client_name.into(),
        "-p".into(),
        "Close utility shell? (y/n)".into(),
        format!("kill-pane -t {utility_pane}").into(),
    ]
}

fn presentation_session_name(directory: &Path) -> Option<String> {
    let identifier = directory
        .file_name()?
        .to_str()?
        .strip_prefix("presentation-")?;
    if identifier.len() != 12
        || !identifier
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!("{PRESENTATION_PREFIX}{identifier}"))
}

fn sanitize_diagnostic(diagnostic: &str) -> String {
    diagnostic
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(256)
        .collect()
}

fn pane_disappeared(error: &PresentationError) -> bool {
    matches!(
        error,
        PresentationError::TmuxRejected(message)
            if message.contains("no such pane")
                || message.contains("pane not found")
                || message.contains("can't find pane")
    )
}

fn base_topology_preserved(
    before: &PresentationTopology,
    after: &PresentationTopology,
    removed_utility: &str,
) -> bool {
    if after.panes.len() != 2 || after.utility().is_some() || after.pane(removed_utility).is_some()
    {
        return false;
    }
    let (
        Some(before_navigator),
        Some(before_provider),
        Some(after_navigator),
        Some(after_provider),
    ) = (
        before.navigator(),
        before.provider(),
        after.navigator(),
        after.provider(),
    )
    else {
        return false;
    };
    before_navigator.id == after_navigator.id
        && before_navigator.workstream_id == after_navigator.workstream_id
        && before_provider.id == after_provider.id
        && before_provider.workstream_id == after_provider.workstream_id
}

fn validate_legacy_host_alias(host_alias: &str) -> Result<(), PresentationError> {
    if host_alias.is_empty() || host_alias.len() > 128 || host_alias.chars().any(char::is_control) {
        return Err(PresentationError::InvalidAttachmentStatus);
    }
    Ok(())
}

fn canonical_d17_inventory_root(
    state_root: &Path,
) -> Result<PathBuf, D17ProvisionalInventoryError> {
    let metadata =
        fs::symlink_metadata(state_root).map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(D17ProvisionalInventoryError::Ambiguous);
    }
    let state_root =
        fs::canonicalize(state_root).map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
    let metadata =
        fs::symlink_metadata(&state_root).map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(D17ProvisionalInventoryError::Ambiguous);
    }
    Ok(state_root)
}

fn match_materialized_slot_operation(
    slot: &ProvisionalSlot,
    operations: &[D17OnboardingOperationInventory],
    matched_operations: &mut BTreeSet<uuid::Uuid>,
) -> Result<(), D17ProvisionalInventoryError> {
    let matches = operations
        .iter()
        .filter(|operation| operation.runtime_id == slot.candidate_runtime_id())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(()),
        [operation] if operation.phase == OnboardingPhase::CapabilityIssued => {
            matched_operations.insert(operation.operation_id.as_uuid());
            Ok(())
        }
        _ => Err(D17ProvisionalInventoryError::Ambiguous),
    }
}

fn match_slot_operation(
    slot: &ProvisionalSlot,
    operations: &BTreeMap<uuid::Uuid, &D17OnboardingOperationInventory>,
    matched_operations: &mut BTreeSet<uuid::Uuid>,
    allowed_phases: &[OnboardingPhase],
) -> Result<(), D17ProvisionalInventoryError> {
    let request = slot
        .handoff_request()
        .ok_or(D17ProvisionalInventoryError::Ambiguous)?;
    let operation = operations
        .get(&request)
        .ok_or(D17ProvisionalInventoryError::Ambiguous)?;
    if operation.runtime_id != slot.candidate_runtime_id()
        || !allowed_phases.contains(&operation.phase)
        || !matched_operations.insert(request)
    {
        return Err(D17ProvisionalInventoryError::Ambiguous);
    }
    Ok(())
}

fn require_registered_runtime_path(
    slot: &ProvisionalSlot,
    registered_runtime_paths: &[RuntimePaths],
) -> Result<(), D17ProvisionalInventoryError> {
    registered_runtime_paths
        .iter()
        .any(|paths| paths == slot.runtime_paths())
        .then_some(())
        .ok_or(D17ProvisionalInventoryError::Ambiguous)
}

fn classify_d17_runtime_namespace(
    state_root: &Path,
    allowed_runtime_directories: &BTreeSet<PathBuf>,
) -> Result<(), D17ProvisionalInventoryError> {
    let runtime_root = state_root.join("run");
    let metadata = match fs::symlink_metadata(&runtime_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(D17ProvisionalInventoryError::Unavailable),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(D17ProvisionalInventoryError::Ambiguous);
    }
    let entries =
        fs::read_dir(&runtime_root).map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
    for (count, entry) in entries.enumerate() {
        if count >= MAX_D17_PROVISIONAL_INVENTORY_ENTRIES {
            return Err(D17ProvisionalInventoryError::Ambiguous);
        }
        let entry = entry.map_err(|_| D17ProvisionalInventoryError::Unavailable)?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("runtime-"))
            && !allowed_runtime_directories.contains(&entry.path())
        {
            return Err(D17ProvisionalInventoryError::Ambiguous);
        }
    }
    Ok(())
}

fn canonical_d17_seed_cwd(seed_cwd: &Path) -> Result<PathBuf, PresentationError> {
    let seed_cwd = fs::canonicalize(seed_cwd).map_err(|_| PresentationError::D17SeedUnavailable)?;
    if !seed_cwd.is_dir() {
        return Err(PresentationError::D17SeedUnavailable);
    }
    Ok(seed_cwd)
}

fn d17_context_from_marker(
    marker: &D17PresentationMarker,
) -> Result<D17PresentationContext, PresentationError> {
    if marker.version != D17_PRESENTATION_CONTEXT_VERSION
        || marker.presentation_id.is_nil()
        || marker.presentation_revision.value() < Revision::INITIAL.value()
    {
        return Err(PresentationError::D17ContextInvalid);
    }
    let seed_cwd = canonical_d17_seed_cwd(&marker.seed_cwd)?;
    if seed_cwd != marker.seed_cwd {
        return Err(PresentationError::D17ContextInvalid);
    }
    Ok(D17PresentationContext {
        presentation_id: marker.presentation_id,
        presentation_revision: marker.presentation_revision,
        seed_cwd,
    })
}

fn create_paths(paths: &PresentationPaths) -> Result<(), PresentationError> {
    let parent = paths
        .directory
        .parent()
        .ok_or_else(|| PresentationError::InvalidControlPath(paths.directory.clone()))?;
    fs::create_dir_all(parent).map_err(PresentationError::Io)?;
    set_mode(parent, 0o700)?;
    fs::create_dir(&paths.directory).map_err(PresentationError::Io)?;
    set_mode(&paths.directory, 0o700)?;
    let config = presentation_tmux_config();
    fs::write(&paths.config, &config).map_err(PresentationError::Io)?;
    set_mode(&paths.config, 0o600)?;

    let directory_metadata =
        fs::symlink_metadata(&paths.directory).map_err(PresentationError::Io)?;
    let config_metadata = fs::symlink_metadata(&paths.config).map_err(PresentationError::Io)?;
    let marker = PresentationOwnershipMarker {
        version: 1,
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
        session_name: paths.session_name.clone(),
        directory_identity: legacy_file_identity(&directory_metadata, None),
        config_identity: legacy_file_identity(&config_metadata, Some(config.as_bytes())),
        socket_identity: None,
        d17: None,
    };
    write_presentation_ownership_marker(paths, &marker, None)
}

fn presentation_ownership_marker_path(paths: &PresentationPaths) -> PathBuf {
    paths.directory.join(PRESENTATION_OWNERSHIP_MARKER_FILE)
}

fn write_presentation_ownership_marker(
    paths: &PresentationPaths,
    marker: &PresentationOwnershipMarker,
    expected_identity: Option<&LegacyFileIdentity>,
) -> Result<(), PresentationError> {
    let bytes = serde_json::to_vec(marker).map_err(|_| {
        PresentationError::ControlRefused("presentation ownership marker could not be encoded")
    })?;
    if bytes.len() > MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker exceeded its bound",
        ));
    }
    let marker_path = presentation_ownership_marker_path(paths);
    let Some(expected_identity) = expected_identity else {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&marker_path).map_err(PresentationError::Io)?;
        file.write_all(&bytes).map_err(PresentationError::Io)?;
        file.sync_all().map_err(PresentationError::Io)?;
        set_mode(&marker_path, 0o600)?;
        return sync_directory(&paths.directory);
    };
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(&marker_path).map_err(PresentationError::Io)?;
    let opened = file.metadata().map_err(PresentationError::Io)?;
    let mut before_bytes = Vec::new();
    (&mut file)
        .take((MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES as u64).saturating_add(1))
        .read_to_end(&mut before_bytes)
        .map_err(PresentationError::Io)?;
    if before_bytes.len() > MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES
        || legacy_file_identity(&opened, Some(&before_bytes)) != *expected_identity
        || !opened.is_file()
        || !is_private_owner_file(&opened)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership changed before marker update",
        ));
    }
    file.set_len(0).map_err(PresentationError::Io)?;
    file.seek(SeekFrom::Start(0))
        .map_err(PresentationError::Io)?;
    file.write_all(&bytes).map_err(PresentationError::Io)?;
    file.sync_all().map_err(PresentationError::Io)?;
    let after = fs::symlink_metadata(&marker_path).map_err(PresentationError::Io)?;
    if file_device(&after) != expected_identity.device
        || file_inode(&after) != expected_identity.inode
        || !after.is_file()
        || !is_private_owner_file(&after)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership changed during marker update",
        ));
    }
    sync_directory(&paths.directory)
}

fn read_presentation_ownership(
    paths: &PresentationPaths,
) -> Result<Option<PresentationOwnershipProof>, PresentationError> {
    read_presentation_ownership_with_artifacts(paths, PresentationArtifactSet::D16)
}

/// Reads an owned D17 presentation after a provisional marker may exist. The
/// ordinary D16 ownership reader deliberately continues to reject that marker
/// so legacy close/cleanup cannot adopt a partially materialized D17 shell.
fn read_d17_presentation_ownership(
    paths: &PresentationPaths,
) -> Result<Option<PresentationOwnershipProof>, PresentationError> {
    read_presentation_ownership_with_artifacts(paths, PresentationArtifactSet::D17)
}

#[derive(Clone, Copy)]
enum PresentationArtifactSet {
    D16,
    D17,
}

fn read_presentation_ownership_with_artifacts(
    paths: &PresentationPaths,
    artifacts: PresentationArtifactSet,
) -> Result<Option<PresentationOwnershipProof>, PresentationError> {
    let directory_metadata = match fs::symlink_metadata(&paths.directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PresentationError::Io(error)),
    };
    if directory_metadata.file_type().is_symlink()
        || !directory_metadata.is_dir()
        || !is_private_owner_directory(&directory_metadata)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership directory is foreign or malformed",
        ));
    }
    validate_presentation_artifact_entries(&paths.directory, artifacts)?;
    let marker_path = presentation_ownership_marker_path(paths);
    let marker_metadata = match fs::symlink_metadata(&marker_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PresentationError::Io(error)),
    };
    if marker_metadata.file_type().is_symlink()
        || !marker_metadata.is_file()
        || !is_private_owner_file(&marker_metadata)
        || marker_metadata.len() > MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES as u64
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker is foreign or malformed",
        ));
    }
    let bytes = read_private_file(&marker_path, MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES)
        .map_err(map_presentation_ownership_probe)?
        .ok_or(PresentationError::ControlRefused(
            "presentation ownership marker disappeared",
        ))?;
    let marker_after = fs::symlink_metadata(&marker_path).map_err(PresentationError::Io)?;
    if marker_after.file_type().is_symlink()
        || !marker_after.is_file()
        || !is_private_owner_file(&marker_after)
        || marker_after.len() != marker_metadata.len()
        || file_device(&marker_after) != file_device(&marker_metadata)
        || file_inode(&marker_after) != file_inode(&marker_metadata)
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker changed during inspection",
        ));
    }
    let marker: PresentationOwnershipMarker = serde_json::from_slice(&bytes).map_err(|_| {
        PresentationError::ControlRefused("presentation ownership marker is malformed")
    })?;
    if marker.version != 1
        || marker.directory != paths.directory
        || marker.socket != paths.socket
        || marker.session_name != paths.session_name
        || marker.config_identity.mode != 0o600
        || marker.directory_identity.mode != 0o700
        || !directory_identity_compatible(
            &marker.directory_identity,
            &legacy_file_identity(&directory_metadata, None),
        )
    {
        return Err(PresentationError::ControlRefused(
            "presentation ownership marker does not prove this directory",
        ));
    }
    let config = inspect_regular_file(&paths.config, true, MAX_LEGACY_CONFIG_BYTES)
        .map_err(map_presentation_ownership_probe)?
        .ok_or(PresentationError::ControlRefused(
            "presentation configuration is missing",
        ))?;
    if config != marker.config_identity || !config_content_matches(&config) {
        return Err(PresentationError::ControlRefused(
            "presentation configuration is foreign or modified",
        ));
    }
    let socket = inspect_private_socket(&paths.socket).map_err(map_presentation_ownership_probe)?;
    if socket.is_some()
        && marker.socket_identity.is_some()
        && !optional_socket_identity_compatible(marker.socket_identity.as_ref(), socket.as_ref())
    {
        return Err(PresentationError::ControlRefused(
            "presentation socket identity changed",
        ));
    }
    if let Some(attachment) = inspect_regular_file(
        &paths.attachment_status,
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    )
    .map_err(map_presentation_ownership_probe)?
    {
        let _ = attachment;
    }
    let marker_identity = legacy_file_identity(&marker_after, Some(&bytes));
    Ok(Some(PresentationOwnershipProof {
        marker,
        marker_identity,
        socket_identity: socket,
    }))
}

fn map_presentation_ownership_probe(failure: LegacyProbeFailure) -> PresentationError {
    match failure {
        LegacyProbeFailure::Inaccessible => PresentationError::ControlRefused(
            "presentation ownership artifact could not be inspected safely",
        ),
        LegacyProbeFailure::Foreign => PresentationError::ControlRefused(
            "presentation ownership artifact is foreign or symlinked",
        ),
        LegacyProbeFailure::Malformed => {
            PresentationError::ControlRefused("presentation ownership artifact is malformed")
        }
    }
}

fn validate_presentation_artifact_entries(
    directory: &Path,
    artifacts: PresentationArtifactSet,
) -> Result<(), PresentationError> {
    let entries = fs::read_dir(directory).map_err(PresentationError::Io)?;
    for (count, entry) in entries
        .take(MAX_LEGACY_PRESENTATION_ENTRIES + 1)
        .enumerate()
    {
        if count >= MAX_LEGACY_PRESENTATION_ENTRIES {
            return Err(PresentationError::ControlRefused(
                "presentation directory contains too many artifacts",
            ));
        }
        let entry = entry.map_err(PresentationError::Io)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(PresentationError::Io)?;
        if metadata.file_type().is_symlink() {
            return Err(PresentationError::ControlRefused(
                "presentation directory contains a symlink",
            ));
        }
        let name = entry.file_name();
        if name != PRESENTATION_OWNERSHIP_MARKER_FILE
            && name != ATTACHMENT_STATUS_FILE
            && name != "tmux.conf"
            && name != "tmux.sock"
            && !(matches!(artifacts, PresentationArtifactSet::D17)
                && name == crate::provisional::PROVISIONAL_MARKER_FILE)
        {
            return Err(PresentationError::ControlRefused(
                "presentation directory contains an unknown artifact",
            ));
        }
    }
    Ok(())
}

fn remove_owned_presentation(
    paths: &PresentationPaths,
    expected: &PresentationOwnershipProof,
) -> Result<(), PresentationError> {
    let actual = read_presentation_ownership(paths)?.ok_or(PresentationError::ControlRefused(
        "presentation ownership disappeared",
    ))?;
    if actual.marker != expected.marker || actual.marker_identity != expected.marker_identity {
        return Err(PresentationError::ControlRefused(
            "presentation ownership changed during close",
        ));
    }

    // Recheck the bounded allowlist before each unlink. This never recursively
    // removes the directory, and an unknown/sentinel entry therefore remains
    // untouched even if it appears during cleanup.
    validate_presentation_artifact_entries(&paths.directory, PresentationArtifactSet::D16)?;
    let attachment = inspect_regular_file(
        &paths.attachment_status,
        false,
        MAX_ATTACHMENT_STATUS_BYTES_USIZE,
    )
    .map_err(map_presentation_ownership_probe)?;
    if let Some(identity) = attachment.as_ref() {
        remove_exact_regular_artifact(
            &paths.attachment_status,
            Some(identity),
            MAX_ATTACHMENT_STATUS_BYTES_USIZE,
            &mut |_| Ok(()),
        )?;
    }

    validate_presentation_artifact_entries(&paths.directory, PresentationArtifactSet::D16)?;
    remove_exact_regular_artifact(
        &paths.config,
        Some(&expected.marker.config_identity),
        MAX_LEGACY_CONFIG_BYTES,
        &mut |_| Ok(()),
    )?;

    validate_presentation_artifact_entries(&paths.directory, PresentationArtifactSet::D16)?;
    let socket = inspect_private_socket(&paths.socket).map_err(map_presentation_ownership_probe)?;
    if socket.is_some()
        && !optional_socket_identity_compatible(expected.socket_identity.as_ref(), socket.as_ref())
    {
        return Err(PresentationError::ControlRefused(
            "presentation socket identity changed during close",
        ));
    }
    if let Some(identity) = socket.as_ref() {
        remove_exact_socket_artifact(&paths.socket, Some(identity), &mut |_| Ok(()))?;
    }

    validate_presentation_artifact_entries(&paths.directory, PresentationArtifactSet::D16)?;
    remove_exact_regular_artifact(
        &presentation_ownership_marker_path(paths),
        Some(&expected.marker_identity),
        MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES,
        &mut |_| Ok(()),
    )?;
    match fs::remove_dir(&paths.directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
            Err(PresentationError::ControlRefused(
                "presentation directory gained an entry during close",
            ))
        }
        Err(error) => Err(PresentationError::Io(error)),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), PresentationError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(PresentationError::Io)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), PresentationError> {
    Ok(())
}

fn stopped_owned_presentation(presentation_live: bool) -> bool {
    !presentation_live
}

fn should_reuse_presentation(session_live: bool, navigator_pane_dead: bool) -> bool {
    session_live && !navigator_pane_dead
}

/// Presentation ownership failures; no provider content is retained in their
/// diagnostics.
#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("multiple private navigator presentations are live; close one before reconnecting")]
    AmbiguousPresentations,
    #[error("multiple legacy navigator presentations require cutover refusal")]
    AmbiguousLegacyPresentations,
    #[error("legacy presentation proof changed during revalidation")]
    LegacyProofChanged,
    #[error("legacy presentation mutation refused: {0}")]
    LegacyMutationRefused(&'static str),
    #[error("legacy presentation did not disappear after bounded retirement")]
    LegacyNotRetired,
    #[error("invalid private presentation control path {0}")]
    InvalidControlPath(PathBuf),
    #[error("invalid private presentation control action")]
    InvalidControlAction,
    #[error("private presentation pane topology is ambiguous")]
    InvalidTopology,
    #[error("private presentation startup failed during {stage}: {source}")]
    StartupFailed {
        stage: &'static str,
        #[source]
        source: Box<PresentationError>,
    },
    #[error("presentation control refused: {0}")]
    ControlRefused(&'static str),
    #[error("invalid private provider attachment status")]
    InvalidAttachmentStatus,
    #[error("provider attachment attempt is stale or already complete")]
    StaleAttachmentAttempt,
    #[error("the D17 presentation context is unavailable")]
    D17ContextUnavailable,
    #[error("the D17 presentation context is already initialized")]
    D17ContextAlreadyInitialized,
    #[error("the D17 presentation context is invalid")]
    D17ContextInvalid,
    #[error("the D17 presentation seed cwd is unavailable")]
    D17SeedUnavailable,
    #[error("invoking terminal geometry is unavailable")]
    TerminalGeometryUnavailable,
    #[error("invoking terminal geometry is invalid")]
    InvalidTerminalGeometry,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("private tmux output exceeded the diagnostic limit")]
    OutputTooLarge,
    #[error("private presentation tmux action failed: {0}")]
    TmuxRejected(String),
    #[error("could not execute bounded private presentation tmux command")]
    TmuxOutput(#[source] BoundedProcessError),
}

impl PresentationError {
    fn from_bounded_tmux(source: BoundedProcessError) -> Self {
        match source {
            BoundedProcessError::OutputTooLarge => Self::OutputTooLarge,
            other => Self::TmuxOutput(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_private_tmux_command_forces_utf8_format_semantics() {
        let command = private_tmux_command();
        assert_eq!(command.get_args().next(), Some(std::ffi::OsStr::new("-u")));
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "TMUX" && value.is_none())
        );
    }

    #[test]
    fn tmux_socket_identity_accepts_only_its_owner_only_live_mode_transition() {
        let detached = LegacyFileIdentity {
            size: 0,
            mode: 0o600,
            device: 7,
            inode: 11,
            digest: None,
        };
        let mut attached = detached;
        attached.mode = 0o700;
        assert!(socket_identity_compatible(&detached, &attached));

        let mut group_accessible = attached;
        group_accessible.mode = 0o770;
        assert!(!socket_identity_compatible(&detached, &group_accessible));

        let mut replacement = attached;
        replacement.inode += 1;
        assert!(!socket_identity_compatible(&detached, &replacement));
    }

    struct DisposableTmuxServerGuard {
        socket: PathBuf,
        directory: Option<PathBuf>,
    }

    impl DisposableTmuxServerGuard {
        fn new(socket: PathBuf, directory: Option<PathBuf>) -> Self {
            Self { socket, directory }
        }
    }

    impl Drop for DisposableTmuxServerGuard {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .env_remove("TMUX")
                .args(["-f", "/dev/null", "-S"])
                .arg(&self.socket)
                .arg("kill-server")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = fs::remove_file(&self.socket);
            if let Some(directory) = &self.directory {
                let _ = fs::remove_dir_all(directory);
            }
        }
    }

    struct DisposableChildGuard {
        child: Option<std::process::Child>,
    }

    impl DisposableChildGuard {
        fn new(child: std::process::Child) -> Self {
            Self { child: Some(child) }
        }
    }

    impl Drop for DisposableChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[test]
    fn failed_outer_attach_accepts_a_stopped_owned_presentation() {
        assert!(stopped_owned_presentation(false));
    }

    #[test]
    fn failed_outer_attach_rejects_a_live_owned_presentation() {
        assert!(!stopped_owned_presentation(true));
    }

    #[test]
    fn attach_geometry_targets_exact_window_and_restores_latest() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        prepare_attach_window_with_size(&paths.session_name, 150, 40, |arguments| {
            calls.borrow_mut().push(arguments);
            Ok(())
        })
        .unwrap();

        let calls = calls.into_inner();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            vec![
                OsString::from("resize-window"),
                OsString::from("-t"),
                OsString::from(format!("{}:navigator", paths.session_name)),
                OsString::from("-x"),
                OsString::from("150"),
                OsString::from("-y"),
                OsString::from("40"),
            ]
        );
        assert_eq!(
            calls[1],
            vec![
                OsString::from("set-window-option"),
                OsString::from("-t"),
                OsString::from(format!("{}:navigator", paths.session_name)),
                OsString::from("window-size"),
                OsString::from("latest"),
            ]
        );
    }

    #[test]
    fn attach_geometry_rejection_stops_before_restore() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        let result = prepare_attach_window_with_size(&paths.session_name, 150, 40, |arguments| {
            calls.borrow_mut().push(arguments);
            Err(PresentationError::TmuxRejected(
                "resize rejected".to_owned(),
            ))
        });

        assert!(matches!(
            result,
            Err(PresentationError::TmuxRejected(message)) if message == "resize rejected"
        ));
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn attach_geometry_latest_rejection_stops_before_native_attach() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        let result = prepare_attach_window_with_size(&paths.session_name, 150, 40, |arguments| {
            let call_number = calls.borrow().len();
            calls.borrow_mut().push(arguments);
            if call_number == 0 {
                Ok(())
            } else {
                Err(PresentationError::TmuxRejected(
                    "latest rejected".to_owned(),
                ))
            }
        });

        assert!(matches!(
            result,
            Err(PresentationError::TmuxRejected(message)) if message == "latest rejected"
        ));
        assert_eq!(calls.borrow().len(), 2);
    }

    #[test]
    fn attach_geometry_rejects_zero_dimensions_without_tmux_access() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let calls = std::cell::RefCell::new(Vec::new());

        assert!(matches!(
            prepare_attach_window_with_size(&paths.session_name, 0, 40, |arguments| {
                calls.borrow_mut().push(arguments);
                Ok(())
            }),
            Err(PresentationError::InvalidTerminalGeometry)
        ));
        assert!(calls.borrow().is_empty());
    }

    #[test]
    #[cfg(unix)]
    #[allow(clippy::too_many_lines)]
    fn detached_nested_private_windows_keep_geometry_and_copy_mode_profile() {
        use std::{process::Stdio, time::Instant};

        if Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
            || Command::new("script")
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_err()
        {
            eprintln!("skipped: tmux and script are required");
            return;
        }

        let temporary = tempfile::tempdir().unwrap();
        let fixture = temporary.path().join("fixture");
        fs::write(&fixture, "#!/bin/sh\nexec /usr/bin/sleep 60\n").unwrap();
        set_mode(&fixture, 0o700).unwrap();

        let tmux = |socket: &Path| {
            let mut command = Command::new("tmux");
            command.env_remove("TMUX").arg("-S").arg(socket);
            command
        };
        let output = |socket: &Path, arguments: &[&str]| -> String {
            let mut command = tmux(socket);
            command.args(arguments);
            let output =
                output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES).unwrap();
            assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
            String::from_utf8(output.stdout).unwrap()
        };
        let wait_for_clients = |socket: &Path| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut command = tmux(socket);
                command.args(["list-clients", "-F", "#{client_name}"]);
                if output_bounded(&mut command, MAX_TMUX_OUTPUT_BYTES, MAX_TMUX_OUTPUT_BYTES)
                    .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
                {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("private tmux client did not attach");
        };
        let assert_geometry = |socket: &Path, target: &str, geometry: &str| {
            assert_eq!(
                output(
                    socket,
                    [
                        "display-message",
                        "-p",
                        "-t",
                        target,
                        "#{window_width}x#{window_height}",
                    ]
                    .as_slice(),
                )
                .trim(),
                geometry
            );
        };
        let assert_window = |socket: &Path, target: &str, geometry: &str| {
            assert_geometry(socket, target, geometry);
            assert_eq!(
                output(
                    socket,
                    ["show-window-options", "-v", "-t", target, "window-size"].as_slice(),
                )
                .trim(),
                "latest"
            );
        };
        let assert_copy_mode_scroll = |socket: &Path, repeat_count: &str| {
            for (table, key, direction) in [
                ("copy-mode", "WheelUpPane", "scroll-up"),
                ("copy-mode", "WheelDownPane", "scroll-down"),
                ("copy-mode-vi", "WheelUpPane", "scroll-up"),
                ("copy-mode-vi", "WheelDownPane", "scroll-down"),
            ] {
                let expected = format!(
                    "bind-key -T {table} {key} select-pane \\; send-keys -X -N {repeat_count} {direction}"
                );
                let bindings = output(socket, ["list-keys", "-T", table].as_slice());
                assert!(
                    bindings
                        .lines()
                        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
                        .any(|line| line == expected),
                    "missing copy-mode binding: {expected}\n{bindings}"
                );
            }
        };
        let set_copy_mode_scroll_repeat = |socket: &Path, repeat_count: &str| {
            for (table, key, direction) in [
                ("copy-mode", "WheelUpPane", "scroll-up"),
                ("copy-mode", "WheelDownPane", "scroll-down"),
                ("copy-mode-vi", "WheelUpPane", "scroll-up"),
                ("copy-mode-vi", "WheelDownPane", "scroll-down"),
            ] {
                assert!(
                    tmux(socket)
                        .args([
                            "bind-key",
                            "-T",
                            table,
                            key,
                            "select-pane",
                            "\\;",
                            "send-keys",
                            "-X",
                            "-N",
                            repeat_count,
                            direction,
                        ])
                        .status()
                        .unwrap()
                        .success()
                );
            }
        };

        let presentation = Presentation::fresh_with_executable(temporary.path(), fixture);
        presentation.start().unwrap();
        let _presentation_guard = DisposableTmuxServerGuard::new(
            presentation.paths().socket.clone(),
            Some(presentation.paths().directory.clone()),
        );
        let presentation_socket = presentation.paths().socket.clone();
        let presentation_target = format!("{}:navigator", presentation.paths().session_name);
        assert_copy_mode_scroll(&presentation_socket, "1");

        let tmux_client = crate::runtime::SystemTmux::default();
        let process_probe = crate::runtime::LinuxProcessProbe;
        let runtime = crate::runtime::PrivateRuntime::new(
            &tmux_client,
            &process_probe,
            crate::runtime::RuntimePaths::for_runtime(
                temporary.path(),
                crate::domain::RuntimeId::new(),
            ),
        );
        runtime
            .start(&crate::runtime::NativeLaunch {
                cwd: temporary.path().to_path_buf(),
                program: vec![
                    OsString::from("/bin/sh"),
                    OsString::from("-c"),
                    OsString::from("sleep 60"),
                ],
                environment: std::collections::BTreeMap::new(),
            })
            .unwrap();
        let _runtime_guard = DisposableTmuxServerGuard::new(
            runtime.paths().socket.clone(),
            Some(runtime.paths().directory.clone()),
        );
        let runtime_target = format!("{}:provider", runtime.paths().session_name);
        assert_copy_mode_scroll(&runtime.paths().socket, "1");
        // Simulate a long-lived Runtime created before this profile existed.
        // The attach preparation below must converge these exact bindings
        // without restarting the sleeping provider pane.
        set_copy_mode_scroll_repeat(&runtime.paths().socket, "5");
        assert_copy_mode_scroll(&runtime.paths().socket, "5");
        assert_geometry(&presentation_socket, &presentation_target, "80x24");
        assert_geometry(&runtime.paths().socket, &runtime_target, "80x24");

        // This is the final outer PTY geometry used by the disposable nested
        // client. Both private windows are still detached at this point.
        let final_columns = 150;
        let final_rows = 40;
        presentation
            .prepare_attach_with_size(final_columns, final_rows)
            .unwrap();
        assert_window(&presentation_socket, &presentation_target, "150x40");

        let outer_socket = temporary.path().join("outer.sock");
        let outer_session = format!("outer-{}", uuid::Uuid::new_v4().simple());
        let status = tmux(&outer_socket)
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                &outer_session,
                "/usr/bin/sleep",
                "60",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let _outer_guard = DisposableTmuxServerGuard::new(outer_socket.clone(), None);
        assert!(
            tmux(&outer_socket)
                .args(["set-option", "-g", "status", "off"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            tmux(&outer_socket)
                .args(["resize-window", "-t", &format!("{outer_session}:0")])
                .args(["-x", "150", "-y", "40"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            tmux(&outer_socket)
                .args([
                    "set-window-option",
                    "-t",
                    &format!("{outer_session}:0"),
                    "window-size",
                    "latest",
                ])
                .status()
                .unwrap()
                .success()
        );
        let nested_presentation_attach = format!(
            "env -u TMUX tmux -S {} attach-session -t {}",
            shell_quote(presentation_socket.as_os_str()).unwrap(),
            shell_quote(Path::new(&presentation.paths().session_name).as_os_str()).unwrap(),
        );
        assert!(
            tmux(&outer_socket)
                .args(["respawn-pane", "-k", "-t", &format!("{outer_session}:0.0")])
                .arg(nested_presentation_attach)
                .status()
                .unwrap()
                .success()
        );

        let outer_attach = format!(
            "stty rows 40 cols 150; exec env -u TMUX tmux -S {} attach-session -t {}",
            shell_quote(outer_socket.as_os_str()).unwrap(),
            shell_quote(Path::new(&outer_session).as_os_str()).unwrap(),
        );
        let outer_client = Command::new("script")
            .env("TERM", "xterm-256color")
            .args(["-qefc", &outer_attach, "/dev/null"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _outer_client_guard = DisposableChildGuard::new(outer_client);
        wait_for_clients(&presentation_socket);
        assert_window(&presentation_socket, &presentation_target, "150x40");

        let panes = output(
            &presentation_socket,
            [
                "list-panes",
                "-t",
                &presentation_target,
                "-F",
                "#{pane_id}\t#{@wsnav_role}\t#{pane_width}\t#{pane_height}",
            ]
            .as_slice(),
        );
        let provider = panes
            .lines()
            .find_map(|line| {
                let mut fields = line.split('\t');
                let pane_id = fields.next()?;
                let role = fields.next()?;
                let columns = fields.next()?.parse::<u16>().ok()?;
                let rows = fields.next()?.parse::<u16>().ok()?;
                (role == "provider").then_some((pane_id.to_owned(), columns, rows))
            })
            .expect("provider pane geometry");
        assert!(provider.1 > 0 && provider.2 > 0);

        runtime
            .prepare_attach_with_size(provider.1, provider.2)
            .unwrap();
        assert_copy_mode_scroll(&presentation_socket, "1");
        assert_copy_mode_scroll(&runtime.paths().socket, "1");
        let runtime_geometry = format!("{}x{}", provider.1, provider.2);
        assert_window(&runtime.paths().socket, &runtime_target, &runtime_geometry);

        let nested_runtime_attach = format!(
            "env -u TMUX tmux -u -S {} attach-session -t {}",
            shell_quote(runtime.paths().socket.as_os_str()).unwrap(),
            shell_quote(Path::new(&runtime.paths().session_name).as_os_str()).unwrap(),
        );
        assert!(
            tmux(&presentation_socket)
                .args(["respawn-pane", "-k", "-t", &provider.0])
                .arg(nested_runtime_attach)
                .status()
                .unwrap()
                .success()
        );
        wait_for_clients(&runtime.paths().socket);
        assert_window(&runtime.paths().socket, &runtime_target, &runtime_geometry);
    }

    #[test]
    fn dead_navigator_pane_is_not_reused_even_when_the_session_is_live() {
        assert!(should_reuse_presentation(true, false));
        assert!(!should_reuse_presentation(true, true));
        assert!(!should_reuse_presentation(false, false));
    }

    #[test]
    fn close_refuses_a_path_shaped_directory_without_our_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        set_mode(&paths.directory, 0o700).unwrap();
        let sentinel = paths.directory.join("foreign-sentinel");
        fs::write(&sentinel, b"leave me alone").unwrap();
        let presentation = Presentation {
            paths: paths.clone(),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let result = presentation.close();
        assert!(matches!(
            result,
            Err(PresentationError::ControlRefused(message))
                if message.contains("ownership marker") || message.contains("unknown artifact")
        ));
        assert!(paths.directory.exists());
        assert!(sentinel.exists());
    }

    #[test]
    fn close_removes_only_a_directory_with_our_ownership_proof() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();
        let presentation = Presentation {
            paths: paths.clone(),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        presentation.close().unwrap();
        assert!(!paths.directory.exists());
    }

    #[test]
    fn d17_context_is_marker_bound_and_uses_only_the_canonical_seed() {
        let temporary = tempfile::tempdir().unwrap();
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();
        let presentation = Presentation {
            paths: paths.clone(),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let presentation_id = uuid::Uuid::from_u128(71);
        let context = presentation
            .initialize_d17_context(presentation_id, &seed)
            .unwrap();
        assert_eq!(context.presentation_id(), presentation_id);
        assert_eq!(context.presentation_revision(), Revision::INITIAL);
        assert_eq!(context.seed_cwd(), seed.canonicalize().unwrap());
        assert_eq!(presentation.d17_context().unwrap(), context);
        assert!(matches!(
            presentation.initialize_d17_context(uuid::Uuid::from_u128(72), &seed),
            Err(PresentationError::D17ContextAlreadyInitialized)
        ));

        let marker = fs::read(paths.directory.join(PRESENTATION_OWNERSHIP_MARKER_FILE)).unwrap();
        assert!(String::from_utf8(marker).unwrap().contains("\"d17\":{"));
    }

    #[test]
    fn d17_context_reopens_only_the_exact_owned_directory_after_slot_materialization() {
        let temporary = tempfile::tempdir().unwrap();
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();
        let presentation = Presentation {
            paths: paths.clone(),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let presentation_id = uuid::Uuid::from_u128(74);
        let context = presentation
            .initialize_d17_context(presentation_id, &seed)
            .unwrap();
        let slot = crate::provisional::ProvisionalSlot::materializing(
            temporary.path(),
            presentation_id,
            context.presentation_revision(),
            1,
            crate::domain::RuntimeId::new(),
            crate::provisional::SlotGeneration::new(uuid::Uuid::from_u128(75)),
            &seed,
        )
        .unwrap();
        crate::provisional::write_new_marker(temporary.path(), &paths.directory, &slot).unwrap();

        assert_eq!(
            Presentation::d17_context_from_directory(temporary.path(), &paths.directory).unwrap(),
            context
        );
        assert!(matches!(
            Presentation::d17_context_from_directory(temporary.path(), &seed),
            Err(PresentationError::D17ContextUnavailable)
        ));
    }

    #[test]
    fn d17_provisional_inventory_allows_one_marker_but_refuses_stale_journal_or_runtime_artifact() {
        let temporary = tempfile::tempdir().unwrap();
        set_mode(temporary.path(), 0o700).unwrap();
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();
        let presentation = Presentation {
            paths: paths.clone(),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let presentation_id = uuid::Uuid::from_u128(76);
        let context = presentation
            .initialize_d17_context(presentation_id, &seed)
            .unwrap();
        assert_eq!(
            Presentation::d17_context_from_directory(temporary.path(), &paths.directory).unwrap(),
            context
        );

        assert_eq!(
            classify_d17_provisional_inventory(temporary.path(), &[], &[]).unwrap(),
            D17ProvisionalInventory::Vacant
        );
        let candidate_runtime_id = crate::domain::RuntimeId::from(uuid::Uuid::from_u128(77));
        let slot = crate::provisional::ProvisionalSlot::materializing(
            temporary.path(),
            presentation_id,
            context.presentation_revision(),
            1,
            candidate_runtime_id,
            crate::provisional::SlotGeneration::new(uuid::Uuid::from_u128(78)),
            &seed,
        )
        .unwrap();
        crate::provisional::write_new_marker(temporary.path(), &paths.directory, &slot).unwrap();
        assert_eq!(
            classify_d17_provisional_inventory(temporary.path(), &[], &[]).unwrap(),
            D17ProvisionalInventory::Occupied
        );
        let stale = D17OnboardingOperationInventory {
            operation_id: crate::domain::OperationId::from(uuid::Uuid::from_u128(79)),
            workstream_id: WorkstreamId::from(uuid::Uuid::from_u128(80)),
            runtime_id: candidate_runtime_id,
            phase: OnboardingPhase::CapabilityIssued,
        };
        assert_eq!(
            classify_d17_provisional_inventory(temporary.path(), &[], &[stale]),
            Err(D17ProvisionalInventoryError::Ambiguous)
        );

        let other = tempfile::tempdir().unwrap();
        set_mode(other.path(), 0o700).unwrap();
        let run = other.path().join("run");
        fs::create_dir(&run).unwrap();
        set_mode(&run, 0o700).unwrap();
        fs::create_dir(run.join("runtime-foreign")).unwrap();
        assert_eq!(
            classify_d17_provisional_inventory(other.path(), &[], &[]),
            Err(D17ProvisionalInventoryError::Ambiguous)
        );
    }

    #[test]
    fn d17_host_materialization_validation_requires_exact_vacant_context_and_lease() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        drop(crate::state::fresh_create(&state_path, &crate::domain::RandomIdGenerator).unwrap());

        let root = crate::state::StateRoot::select(&state_path);
        let transition_lock = state_path.join(crate::state::TRANSITION_LOCK_FILE);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&transition_lock)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&transition_lock, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let transition = crate::state::acquire_transition_lease(&state_path).unwrap();
        let mut migrating = crate::state::open_cutover_transition(&root, &transition).unwrap();
        migrating.migrate_schema13_to14(&transition).unwrap();
        drop(migrating);
        drop(transition);
        fs::remove_file(&transition_lock).unwrap();

        let presentation =
            Presentation::fresh_with_executable(&state_path, PathBuf::from("/workspace/wsnav"));
        create_paths(presentation.paths()).unwrap();
        let presentation_id = uuid::Uuid::from_u128(81);
        let context = presentation
            .initialize_d17_context(presentation_id, &seed)
            .unwrap();
        let mut state = crate::state::open_d17_current_only(&root).unwrap();
        let provisional_lease = state.acquire_d17_provisional_lease().unwrap();
        let slot = crate::provisional::ProvisionalSlot::materializing(
            &state_path,
            presentation_id,
            context.presentation_revision(),
            provisional_lease.lease_generation(),
            crate::domain::RuntimeId::from(uuid::Uuid::from_u128(82)),
            crate::provisional::SlotGeneration::new(uuid::Uuid::from_u128(83)),
            &seed,
        )
        .unwrap();

        assert!(
            crate::provisional::validate_fresh_host_materialization(
                &state,
                &provisional_lease,
                &presentation.paths().directory,
                &slot,
            )
            .is_ok()
        );

        let mismatched_lease_slot = crate::provisional::ProvisionalSlot::materializing(
            &state_path,
            presentation_id,
            context.presentation_revision(),
            provisional_lease.lease_generation() + 1,
            crate::domain::RuntimeId::from(uuid::Uuid::from_u128(84)),
            crate::provisional::SlotGeneration::new(uuid::Uuid::from_u128(85)),
            &seed,
        )
        .unwrap();
        assert!(matches!(
            crate::provisional::validate_fresh_host_materialization(
                &state,
                &provisional_lease,
                &presentation.paths().directory,
                &mismatched_lease_slot,
            ),
            Err(crate::provisional::HostMaterializationError::Lease)
        ));

        crate::provisional::write_new_marker(&state_path, &presentation.paths().directory, &slot)
            .unwrap();
        assert!(matches!(
            crate::provisional::validate_fresh_host_materialization(
                &state,
                &provisional_lease,
                &presentation.paths().directory,
                &slot,
            ),
            Err(crate::provisional::HostMaterializationError::Occupied)
        ));
    }

    #[test]
    fn ordinary_presentation_marker_omits_the_dormant_d17_shape() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();

        let marker = fs::read(paths.directory.join(PRESENTATION_OWNERSHIP_MARKER_FILE)).unwrap();
        assert!(!marker.windows(5).any(|window| window == b"\"d17\""));
    }

    #[test]
    fn d17_context_refuses_a_deleted_seed_without_fallback() {
        let temporary = tempfile::tempdir().unwrap();
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        presentation
            .initialize_d17_context(uuid::Uuid::from_u128(73), &seed)
            .unwrap();
        fs::remove_dir(&seed).unwrap();

        assert!(matches!(
            presentation.d17_context(),
            Err(PresentationError::D17SeedUnavailable)
        ));
    }

    #[test]
    fn close_leaves_unknown_artifacts_when_owned_directory_is_tampered() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();
        let sentinel = paths.directory.join("foreign-sentinel");
        fs::write(&sentinel, b"leave me alone").unwrap();
        let presentation = Presentation {
            paths: paths.clone(),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let result = presentation.close();
        assert!(matches!(
            result,
            Err(PresentationError::ControlRefused(message))
                if message.contains("unknown artifact")
        ));
        assert!(paths.directory.exists());
        assert!(sentinel.exists());
        assert!(paths.config.exists());
    }

    #[test]
    fn navigator_liveness_probe_targets_only_the_exact_owned_pane() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let arguments = presentation.pane_dead_arguments("%42");

        assert_eq!(arguments[0], "display-message");
        assert_eq!(arguments[2], "-t");
        assert_eq!(arguments[3], std::ffi::OsString::from("%42"));
        assert_eq!(arguments[4], "#{pane_dead}");
        assert!(arguments.iter().all(|argument| argument != "0.1"));
    }

    #[test]
    fn presentation_config_selects_the_clicked_pane_on_mouse_release() {
        let config = presentation_tmux_config();
        assert!(config.contains("set -g mouse on"));
        assert!(config.contains("bind-key -T root MouseUp1Pane select-pane -t = \\; send-keys -M"));
        assert!(config.contains("WheelUpPane"));
        assert!(!config.contains("MouseDown3Pane"));
    }

    #[test]
    fn presentation_config_rebuilds_bounded_d12_allowlists() {
        assert_eq!(
            presentation_tmux_config(),
            concat!(
                "set -g status off\n",
                "set -g mouse on\n",
                "set -g remain-on-exit on\n",
                "set -g prefix C-b\n",
                "set -g prefix2 None\n",
                "unbind-key -a -T prefix\n",
                "unbind-key -a -T root\n",
                "set -g default-terminal tmux-256color\n",
                "set-environment -g COLORTERM truecolor\n",
                "set -g extended-keys always\n",
                "set -q -g extended-keys-format csi-u\n",
                "set -as terminal-features ',xterm-ghostty:RGB:extkeys'\n",
                "set -as terminal-features ',tmux-256color:RGB:extkeys'\n",
                "bind-key -T copy-mode WheelUpPane select-pane \\; send-keys -X -N 1 scroll-up\n",
                "bind-key -T copy-mode WheelDownPane select-pane \\; send-keys -X -N 1 scroll-down\n",
                "bind-key -T copy-mode-vi WheelUpPane select-pane \\; send-keys -X -N 1 scroll-up\n",
                "bind-key -T copy-mode-vi WheelDownPane select-pane \\; send-keys -X -N 1 scroll-down\n",
                "bind-key -T root MouseDown1Pane select-pane -t = \\; send-keys -M\n",
                "bind-key -T root MouseUp1Pane select-pane -t = \\; send-keys -M\n",
                "bind-key -T root MouseDrag1Pane if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -M\"\n",
                "bind-key -T root WheelUpPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"copy-mode -e\"\n",
                "bind-key -T root WheelDownPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" \"send-keys -M\" \"send-keys -M\"\n",
            )
        );
    }

    #[test]
    fn topology_parser_rejects_dead_duplicate_and_unknown_roles() {
        let valid = concat!(
            "%0\tnavigator\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t24\t128\t24\n",
        );
        assert!(parse_topology(valid).is_ok());
        assert!(matches!(
            parse_topology(&valid.replace("\t0\t0\t0\t32", "\t1\t0\t0\t32")),
            Err(PresentationError::InvalidTopology)
        ));
        let duplicate = valid.replace("%1\tprovider", "%0\tprovider");
        assert!(matches!(
            parse_topology(&duplicate),
            Err(PresentationError::InvalidTopology)
        ));
        let unknown = valid.replace("provider", "unknown");
        assert!(matches!(
            parse_topology(&unknown),
            Err(PresentationError::InvalidTopology)
        ));
        assert!(
            parse_topology_with_dead(&valid.replace("\t0\t0\t0\t32", "\t1\t0\t0\t32"), true)
                .is_ok()
        );
    }

    #[test]
    fn tmux_window_id_requires_an_at_sign_and_decimal_digits() {
        assert!(parse_window_id("@0"));
        assert!(parse_window_id("@123"));
        assert!(!parse_window_id("@"));
        assert!(!parse_window_id("@window"));
        assert!(!parse_window_id("%0"));
    }

    #[test]
    fn topology_parser_rejects_unsupported_geometry() {
        let valid = concat!(
            "%0\tnavigator\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t24\t128\t24\n",
        );
        assert!(parse_topology(valid).is_ok());
        assert!(parse_topology(&valid.replace("\t33\t0\t95\t24", "\t34\t0\t94\t24")).is_err());
        assert!(
            parse_topology(&valid.replace("\t0\t0\t32\t24\t128", "\t1\t0\t32\t24\t128")).is_err()
        );
        assert!(parse_topology(&valid.replace("\t128\t24", "\t127\t24")).is_err());

        let three_pane = concat!(
            "%0\tnavigator\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t11\t128\t24\n",
            "%2\tutility\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t12\t95\t12\t128\t24\n",
        );
        assert!(parse_topology(three_pane).is_ok());
        assert!(
            parse_topology(&three_pane.replace("\t12\t95\t12\t128", "\t11\t95\t13\t128")).is_err()
        );
        assert!(
            parse_topology(&three_pane.replace("\t33\t12\t95\t12", "\t34\t12\t94\t12")).is_err()
        );
    }

    #[test]
    fn observer_review_topology_retires_utility_and_rejects_external_splits() {
        let two_pane = concat!(
            "%0\tnavigator\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t24\t128\t24\n",
        );
        let topology = parse_topology(two_pane).unwrap();
        assert_eq!(validate_observer_review_topology(&topology).unwrap(), "%1");

        let utility = concat!(
            "%0\tnavigator\t\t0\t0\t0\t32\t24\t128\t24\n",
            "%1\tprovider\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t0\t95\t11\t128\t24\n",
            "%2\tutility\t01234567-89ab-cdef-0123-456789abcdef\t0\t33\t12\t95\t12\t128\t24\n",
        );
        let topology = parse_topology(utility).unwrap();
        assert!(matches!(
            validate_observer_review_topology(&topology),
            Err(PresentationError::ControlRefused(message))
                if message.contains("two-pane")
        ));

        let external = utility.replace("utility", "external");
        assert!(matches!(
            parse_topology(&external),
            Err(PresentationError::InvalidTopology)
        ));
    }

    #[test]
    fn control_binding_uses_fixed_shell_quoting_and_tmux_format_source() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state's root/#{danger}/#(marker)");
        let presentation = Presentation {
            paths: PresentationPaths::fresh(&state_root),
            executable: PathBuf::from("/tmp/wsnav's executable/#{danger}/#(marker)"),
            state_root,
        };

        let command = presentation
            .control_shell_command(PresentationAction::SuppressSplit)
            .unwrap();

        assert!(command.contains("'/tmp/wsnav'\\''s executable/##{danger}/##(marker)'"));
        assert!(command.contains("##{danger}"));
        assert!(command.contains("##(marker)"));
        let source_only = command.replace("##{danger}", "").replace("##(marker)", "");
        assert_eq!(source_only.matches("#{").count(), 2);
        assert!(!source_only.contains("#("));
        assert!(command.contains("--action suppress-split"));
        assert!(command.contains("--source-pane '#{pane_id}'"));
        assert!(command.contains("--client-name #{q:client_name}"));
        assert!(!command.contains("; tmux"));
        assert!(!command.contains("split-window"));
    }

    #[test]
    fn close_shell_targets_the_invoking_client_and_exact_utility_pane() {
        let arguments = close_shell_arguments("/dev/pts/9", "%7");
        let values = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                "confirm-before",
                "-t",
                "/dev/pts/9",
                "-p",
                "Close utility shell? (y/n)",
                "kill-pane -t %7",
            ]
        );
        assert_ne!(values[2], values[5]);
    }

    #[test]
    fn navigator_default_width_is_exactly_32_cells() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let arguments = presentation.default_navigator_resize_arguments_for("%0");

        assert_eq!(DEFAULT_NAVIGATOR_PANE_WIDTH, 32);
        assert_eq!(arguments[0], "resize-pane");
        assert_eq!(arguments[1], "-t");
        assert_eq!(arguments[3], "-x");
        assert_eq!(arguments[4], "32");
    }

    #[test]
    fn d17_navigator_command_cannot_fall_back_to_the_d16_pane() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let command = presentation
            .d17_navigator_command()
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command[3], "_navigator_d17");
        assert_ne!(command[3], "_navigator");
    }

    #[test]
    fn navigator_width_hooks_target_only_the_exact_private_pane() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let exact_target = "%0".to_owned();

        for hook in NAVIGATOR_WIDTH_HOOKS {
            let arguments = presentation.navigator_width_hook_arguments_for(hook, "%0");
            assert_eq!(arguments[0], "set-hook");
            assert_eq!(arguments[1], "-t");
            assert_eq!(
                arguments[2],
                OsString::from(&presentation.paths.session_name)
            );
            assert_eq!(arguments[3], hook);
            assert_eq!(
                arguments[4],
                OsString::from(format!(
                    "resize-pane -t {exact_target} -x {DEFAULT_NAVIGATOR_PANE_WIDTH}"
                ))
            );
            assert!(arguments.iter().all(|argument| argument != "run-shell"));
            assert!(arguments.iter().all(|argument| argument != PROVIDER_PANE));
        }
    }

    #[test]
    fn attachment_status_advances_only_the_exact_ephemeral_attempt() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let workstream_id = WorkstreamId::new();
        let pending = presentation.prepare_attachment(workstream_id).unwrap();

        assert_eq!(
            presentation.read_attachment_status().unwrap(),
            Some(pending.clone())
        );
        assert!(matches!(
            presentation.report_attachment_phase(uuid::Uuid::new_v4(), AttachmentPhase::Running),
            Err(PresentationError::StaleAttachmentAttempt)
        ));

        presentation
            .report_attachment_phase(pending.attempt_id, AttachmentPhase::Running)
            .unwrap();
        presentation
            .report_attachment_phase(pending.attempt_id, AttachmentPhase::Failed)
            .unwrap();
        let failed = presentation.read_attachment_status().unwrap().unwrap();
        assert_eq!(failed.phase, AttachmentPhase::Failed);
        assert_eq!(failed.workstream_id, workstream_id);
        assert!(matches!(
            presentation.report_attachment_phase(pending.attempt_id, AttachmentPhase::Running),
            Err(PresentationError::StaleAttachmentAttempt)
        ));
    }

    #[test]
    fn running_attachment_status_does_not_probe_the_presentation_tmux_server() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let pending = presentation
            .prepare_attachment(WorkstreamId::new())
            .unwrap();
        presentation
            .report_attachment_phase(pending.attempt_id, AttachmentPhase::Running)
            .unwrap();

        let status = presentation.attachment_status().unwrap().unwrap();

        assert_eq!(status.attempt_id, pending.attempt_id);
        assert_eq!(status.phase, AttachmentPhase::Running);
    }

    #[test]
    #[cfg(unix)]
    fn attachment_status_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir_all(&paths.directory).unwrap();
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        presentation
            .prepare_attachment(WorkstreamId::new())
            .unwrap();

        assert_eq!(
            fs::metadata(&presentation.paths.attachment_status)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn provider_attachment_uses_direct_arguments_not_a_shell() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let command =
            presentation.provider_attach_command(WorkstreamId::new(), uuid::Uuid::new_v4());
        assert!(
            command
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
        assert!(
            command
                .iter()
                .any(|argument| argument == "_provider_attach")
        );
        assert_eq!(command.len(), 11);
    }

    #[test]
    fn observer_review_uses_only_the_owned_provider_pane_and_direct_arguments() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let command = presentation.observer_review_command();

        assert_eq!(command[0], "/workspace/wsnav");
        assert_eq!(command[1], "--state-root");
        assert_eq!(command[3], "_observer_review");
        assert!(
            command
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
    }

    #[test]
    fn provider_respawn_forwards_the_complete_direct_attachment_command() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let workstream_id = WorkstreamId::new();
        let arguments = presentation.provider_respawn_arguments(
            PROVIDER_PANE,
            workstream_id,
            uuid::Uuid::new_v4(),
        );

        assert_eq!(arguments.len(), 15);
        assert_eq!(arguments[0], "respawn-pane");
        assert_eq!(arguments[4], "/workspace/wsnav");
        assert_eq!(arguments[7], "_provider_attach");
        assert_eq!(arguments[8], OsString::from(workstream_id.to_string()));
        assert_eq!(arguments[9], "--presentation-socket");
        assert_eq!(arguments[13], "--attempt-id");
    }

    #[test]
    fn d17_provisional_attachment_uses_only_the_exact_private_tmux_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = RuntimePaths::for_runtime(
            temporary.path(),
            crate::domain::RuntimeId::from(uuid::Uuid::from_u128(87)),
        );

        let command = Presentation::d17_provisional_attach_command(&runtime);

        assert_eq!(command[0], "env");
        assert_eq!(command[1], "-u");
        assert_eq!(command[2], "TMUX");
        assert_eq!(command[3], "tmux");
        assert_eq!(command[4], "-u");
        assert_eq!(command[5], "-S");
        assert_eq!(command[6], runtime.socket.into_os_string());
        assert_eq!(command[7], "attach-session");
        assert_eq!(command[8], "-t");
        assert_eq!(command[9], OsString::from(runtime.session_name));
        assert!(
            command
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
    }

    #[test]
    fn control_path_rejects_the_default_tmux_socket() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(
            PresentationPaths::from_control(
                temporary.path(),
                PathBuf::from("/tmp/tmux-default"),
                "wsnav-presentation-example".to_owned(),
            )
            .is_err()
        );
    }

    #[test]
    fn control_path_requires_the_exact_owned_session_name() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        assert!(
            PresentationPaths::from_control(
                temporary.path(),
                paths.socket,
                "wsnav-presentation-other".to_owned(),
            )
            .is_err()
        );
    }

    fn proof_identity() -> LegacyFileIdentity {
        LegacyFileIdentity {
            size: 42,
            mode: 0o755,
            device: 1,
            inode: 2,
            digest: None,
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the fixture spells every independently falsifiable pane proof field"
    )]
    fn proof_pane(
        paths: &PresentationPaths,
        state_root: &Path,
        role: PresentationPaneRole,
        id: &str,
        pid: Option<u32>,
        process_pid: Option<u32>,
        birth: Option<u64>,
        process_stable: bool,
        arguments: &[&str],
        left: u16,
        top: u16,
        width: u16,
        height: u16,
    ) -> LegacyPresentationPaneEvidenceForTest {
        let _ = (paths, state_root);
        LegacyPresentationPaneEvidenceForTest {
            id: id.to_owned(),
            role,
            dead: false,
            pid,
            process_pid,
            birth,
            process_stable,
            executable_path: Some(PathBuf::from("/workspace/wsnav")),
            executable_identity: Some(proof_identity()),
            arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
            left,
            top,
            width,
            height,
            window_width: 128,
            window_height: 24,
        }
    }

    fn proof_evidence(
        temporary: &tempfile::TempDir,
        provider_arguments: &[&str],
        utility: bool,
        clients: &[&str],
    ) -> (PresentationPaths, LegacyPresentationEvidenceForTest) {
        let paths = PresentationPaths::fresh(temporary.path());
        let root = temporary.path();
        let navigator_arguments = [
            "/workspace/wsnav",
            "--state-root",
            root.to_str().unwrap(),
            "_navigator",
            "--presentation-socket",
            paths.socket.to_str().unwrap(),
            "--presentation-session",
            paths.session_name.as_str(),
        ];
        let navigator = proof_pane(
            &paths,
            root,
            PresentationPaneRole::Navigator,
            "%0",
            Some(101),
            None,
            Some(11),
            true,
            &navigator_arguments,
            0,
            0,
            32,
            24,
        );
        let provider_height = if utility { 11 } else { 24 };
        let provider = proof_pane(
            &paths,
            root,
            PresentationPaneRole::Provider,
            "%1",
            Some(102),
            None,
            Some(12),
            true,
            provider_arguments,
            33,
            0,
            95,
            provider_height,
        );
        let utility_pane = utility.then(|| {
            proof_pane(
                &paths,
                root,
                PresentationPaneRole::Utility,
                "%2",
                None,
                None,
                None,
                true,
                &[],
                33,
                12,
                95,
                12,
            )
        });
        let mut panes = vec![navigator, provider];
        if let Some(utility) = utility_pane {
            panes.push(utility);
        }
        (
            paths,
            LegacyPresentationEvidenceForTest {
                executable_path: PathBuf::from("/workspace/wsnav"),
                config_identity: None,
                session_id: Some("$0".to_owned()),
                window_id: Some("@0".to_owned()),
                panes,
                clients: clients.iter().map(|value| (*value).to_owned()).collect(),
                shell_claim_present: false,
                attachment_status: None,
            },
        )
    }

    #[test]
    fn legacy_topology_width_uses_rightmost_pane_extent() {
        let panes = vec![
            LegacyOwnedPane {
                id: "%0".to_owned(),
                role: PresentationPaneRole::Navigator,
                host_alias: None,
                workstream_id: None,
                dead: false,
                left: 0,
                top: 0,
                width: 32,
                height: 24,
            },
            LegacyOwnedPane {
                id: "%1".to_owned(),
                role: PresentationPaneRole::Provider,
                host_alias: None,
                workstream_id: None,
                dead: false,
                left: 33,
                top: 0,
                width: 95,
                height: 24,
            },
        ];
        assert_eq!(legacy_topology_dimensions(&panes), (128, 24));
    }

    #[test]
    fn legacy_proof_classifies_detached_ordinary_presentation() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, evidence) = proof_evidence(&temporary, &provider, false, &[]);
        let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        assert_eq!(
            assessment.state(),
            LegacyPresentationState::DetachedOrdinary
        );
        let proof = assessment.proof().expect("exact detached proof");
        assert_eq!(proof.navigator_pid(), Some(101));
        assert_eq!(proof.navigator_process_birth(), Some(11));
        assert_eq!(proof.attached_client_count(), 0);
        assert!(!proof.utility_present());
        assert!(!proof.observer_review_present());
    }

    #[test]
    fn legacy_proof_refuses_attached_presentation() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, evidence) = proof_evidence(&temporary, &provider, false, &["/dev/pts/9"]);
        let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        assert_eq!(assessment.state(), LegacyPresentationState::Attached);
        assert_eq!(assessment.proof().unwrap().attached_client_count(), 1);
    }

    #[test]
    fn legacy_proof_distinguishes_utility_shell_presence() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, evidence) = proof_evidence(&temporary, &provider, true, &[]);
        let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        assert_eq!(assessment.state(), LegacyPresentationState::UtilityShell);
        assert!(assessment.proof().unwrap().utility_present());
    }

    #[test]
    fn legacy_proof_requires_exact_observer_review_command() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_observer_review",
        ];
        let (paths, evidence) = proof_evidence(&temporary, &provider, false, &[]);
        let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        assert_eq!(assessment.state(), LegacyPresentationState::ObserverReview);
        assert!(assessment.proof().unwrap().observer_review_present());
    }

    #[test]
    fn legacy_proof_rejects_pid_birth_and_executable_mismatch() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, mut evidence) = proof_evidence(&temporary, &provider, false, &[]);
        evidence.panes[0].process_pid = Some(999);
        let assessment =
            classify_legacy_evidence(&paths.directory, temporary.path(), evidence.clone());
        assert_eq!(assessment.state(), LegacyPresentationState::Foreign);

        evidence.panes[0].process_pid = None;
        evidence.panes[0].process_stable = false;
        let assessment =
            classify_legacy_evidence(&paths.directory, temporary.path(), evidence.clone());
        assert_eq!(assessment.state(), LegacyPresentationState::Foreign);

        evidence.panes[0].process_stable = true;
        evidence.panes[0].executable_identity = Some(LegacyFileIdentity {
            inode: 999,
            ..proof_identity()
        });
        let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        assert_eq!(assessment.state(), LegacyPresentationState::Foreign);
    }

    #[test]
    fn legacy_proof_rejects_malformed_topology() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, mut evidence) = proof_evidence(&temporary, &provider, false, &[]);
        evidence.panes.pop();
        let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        assert_eq!(assessment.state(), LegacyPresentationState::Malformed);
        assert!(assessment.proof().is_none());
    }

    #[test]
    fn legacy_drain_attach_requires_a_fully_proven_controller_without_state_access() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, evidence) = proof_evidence(&temporary, &provider, false, &["/dev/pts/9"]);
        let attached = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        let expected = attached.proof().expect("attached proof").clone();
        assert!(expected.controller_proven());

        let called = std::cell::Cell::new(false);
        drain_attach_legacy_presentation_with(
            &expected,
            || Ok(attached.clone()),
            |proof| {
                called.set(true);
                assert!(proof.controller_proven());
                Ok(())
            },
        )
        .unwrap();
        assert!(called.get());

        let (paths, mut evidence) = proof_evidence(&temporary, &provider, false, &["/dev/pts/9"]);
        evidence.panes[0].dead = true;
        let unproven = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        let expected = unproven.proof().expect("attached proof").clone();
        assert!(!expected.controller_proven());
        let called = std::cell::Cell::new(false);
        let result = drain_attach_legacy_presentation_with(
            &expected,
            || Ok(unproven.clone()),
            |_| {
                called.set(true);
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(PresentationError::LegacyMutationRefused(
                "navigator/controller evidence is incomplete"
            ))
        ));
        assert!(!called.get());
    }

    #[test]
    fn legacy_retirement_refuses_changed_proof_before_kill() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        let (paths, evidence) = proof_evidence(&temporary, &provider, false, &[]);
        let original = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
        let expected = original.proof().expect("detached proof").clone();
        let (paths, mut changed_evidence) = proof_evidence(&temporary, &provider, false, &[]);
        changed_evidence.panes[1].process_stable = false;
        let changed =
            classify_legacy_evidence(&paths.directory, temporary.path(), changed_evidence);
        let killed = std::cell::Cell::new(false);
        let result = retire_legacy_presentation_with(
            &expected,
            || Ok(changed.clone()),
            |_| {
                killed.set(true);
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(false),
        );
        assert!(matches!(result, Err(PresentationError::LegacyProofChanged)));
        assert!(!killed.get());
    }

    #[test]
    fn legacy_retirement_refuses_all_drain_surfaces_without_kill() {
        let temporary = tempfile::tempdir().unwrap();
        let provider = [
            "/workspace/wsnav",
            "--state-root",
            temporary.path().to_str().unwrap(),
            "_provider_wait",
        ];
        for (utility, clients, provider_arguments) in [
            (
                false,
                vec!["/dev/pts/9"],
                provider
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>(),
            ),
            (
                true,
                Vec::new(),
                provider
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>(),
            ),
            (
                false,
                Vec::new(),
                vec![
                    "/workspace/wsnav",
                    "--state-root",
                    temporary.path().to_str().unwrap(),
                    "_observer_review",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
        ] {
            let provider_arguments = provider_arguments
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let (paths, evidence) =
                proof_evidence(&temporary, &provider_arguments, utility, &clients);
            let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
            let expected = assessment.proof().expect("drain proof").clone();
            let killed = std::cell::Cell::new(false);
            let result = retire_legacy_presentation_with(
                &expected,
                || Ok(assessment.clone()),
                |_| {
                    killed.set(true);
                    Ok(())
                },
                |_| Ok(()),
                |_| Ok(false),
            );
            assert!(matches!(
                result,
                Err(PresentationError::LegacyMutationRefused(
                    "only a detached ordinary presentation may be retired"
                ))
            ));
            assert!(!killed.get());
        }
    }

    #[test]
    fn legacy_classifier_refuses_multiple_directories_even_when_dead() {
        let temporary = tempfile::tempdir().unwrap();
        set_mode(temporary.path(), 0o700).unwrap();
        let presentation_root = temporary.path().join(PRESENTATION_DIRECTORY);
        fs::create_dir(&presentation_root).unwrap();
        set_mode(&presentation_root, 0o700).unwrap();
        let first = presentation_root.join("presentation-0123456789ab");
        let second = presentation_root.join("presentation-abcdefabcdef");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        set_mode(&first, 0o700).unwrap();
        set_mode(&second, 0o700).unwrap();

        assert!(matches!(
            classify_legacy_presentations(temporary.path()),
            Err(PresentationError::AmbiguousLegacyPresentations)
        ));
        assert!(first.exists());
        assert!(second.exists());
    }

    #[test]
    fn legacy_classifier_never_removes_exact_dead_owned_artifacts() {
        let temporary = tempfile::tempdir().unwrap();
        set_mode(temporary.path(), 0o700).unwrap();
        let presentation_root = temporary.path().join(PRESENTATION_DIRECTORY);
        fs::create_dir(&presentation_root).unwrap();
        set_mode(&presentation_root, 0o700).unwrap();
        let directory = presentation_root.join("presentation-0123456789ab");
        fs::create_dir(&directory).unwrap();
        set_mode(&directory, 0o700).unwrap();
        let config = directory.join("tmux.conf");
        fs::write(&config, presentation_tmux_config()).unwrap();
        set_mode(&config, 0o600).unwrap();

        let assessment = classify_legacy_presentations(temporary.path()).unwrap();
        assert_eq!(assessment.state(), LegacyPresentationState::DeadOwned);
        assert!(directory.exists());
        assert!(config.exists());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_cleanup_restart_survives_interruption_after_each_known_artifact() {
        use std::os::unix::net::UnixListener;

        for interrupted_name in [ATTACHMENT_STATUS_FILE, "tmux.conf", "tmux.sock"] {
            let temporary = tempfile::tempdir().unwrap();
            set_mode(temporary.path(), 0o700).unwrap();
            let presentation_root = temporary.path().join(PRESENTATION_DIRECTORY);
            fs::create_dir(&presentation_root).unwrap();
            set_mode(&presentation_root, 0o700).unwrap();
            let paths = PresentationPaths::fresh(temporary.path());
            fs::create_dir(&paths.directory).unwrap();
            set_mode(&paths.directory, 0o700).unwrap();
            fs::write(&paths.config, presentation_tmux_config()).unwrap();
            set_mode(&paths.config, 0o600).unwrap();
            let status = LegacyAttachmentStatus {
                attempt_id: uuid::Uuid::new_v4(),
                host_alias: "local".to_owned(),
                workstream_id: WorkstreamId::new(),
                phase: AttachmentPhase::Pending,
            };
            fs::write(
                &paths.attachment_status,
                serde_json::to_vec(&status).unwrap(),
            )
            .unwrap();
            set_mode(&paths.attachment_status, 0o600).unwrap();

            let assessment = classify_legacy_presentations(temporary.path()).unwrap();
            assert_eq!(assessment.state(), LegacyPresentationState::DeadOwned);
            let mut proof = assessment.proof().unwrap().clone();
            let _socket = if interrupted_name == "tmux.sock" {
                let socket = UnixListener::bind(&paths.socket).unwrap();
                set_mode(&paths.socket, 0o600).unwrap();
                proof.socket_identity = inspect_private_socket(&paths.socket).unwrap();
                Some(socket)
            } else {
                None
            };
            let marker = ensure_retirement_marker(temporary.path(), &proof).unwrap();
            let interruption =
                remove_exact_legacy_artifacts_with(temporary.path(), &proof, &marker, |path| {
                    if path.file_name().and_then(|name| name.to_str()) == Some(interrupted_name) {
                        Err(PresentationError::LegacyMutationRefused(
                            "test interruption after artifact removal",
                        ))
                    } else {
                        Ok(())
                    }
                });
            assert!(matches!(
                interruption,
                Err(PresentationError::LegacyMutationRefused(
                    "test interruption after artifact removal"
                ))
            ));
            assert!(paths.directory.join(LEGACY_RETIREMENT_MARKER_FILE).exists());

            remove_exact_legacy_artifacts(temporary.path(), &proof, &marker).unwrap();
            assert!(matches!(
                classify_legacy_presentations(temporary.path()),
                Ok(assessment) if assessment.state() == LegacyPresentationState::None
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn legacy_cleanup_discovers_marker_after_process_restart() {
        use std::os::unix::net::UnixListener;

        for interrupted_name in [ATTACHMENT_STATUS_FILE, "tmux.conf", "tmux.sock"] {
            let temporary = tempfile::tempdir().unwrap();
            set_mode(temporary.path(), 0o700).unwrap();
            let presentation_root = temporary.path().join(PRESENTATION_DIRECTORY);
            fs::create_dir(&presentation_root).unwrap();
            set_mode(&presentation_root, 0o700).unwrap();
            let paths = PresentationPaths::fresh(temporary.path());
            fs::create_dir(&paths.directory).unwrap();
            set_mode(&paths.directory, 0o700).unwrap();
            fs::write(&paths.config, presentation_tmux_config()).unwrap();
            set_mode(&paths.config, 0o600).unwrap();
            let status = LegacyAttachmentStatus {
                attempt_id: uuid::Uuid::new_v4(),
                host_alias: "local".to_owned(),
                workstream_id: WorkstreamId::new(),
                phase: AttachmentPhase::Pending,
            };
            fs::write(
                &paths.attachment_status,
                serde_json::to_vec(&status).unwrap(),
            )
            .unwrap();
            set_mode(&paths.attachment_status, 0o600).unwrap();

            let assessment = classify_legacy_presentations(temporary.path()).unwrap();
            assert_eq!(assessment.state(), LegacyPresentationState::DeadOwned);
            let mut proof = assessment.proof().unwrap().clone();
            let _socket = if interrupted_name == "tmux.sock" {
                let socket = UnixListener::bind(&paths.socket).unwrap();
                set_mode(&paths.socket, 0o600).unwrap();
                proof.socket_identity = inspect_private_socket(&paths.socket).unwrap();
                Some(socket)
            } else {
                None
            };
            let marker = ensure_retirement_marker(temporary.path(), &proof).unwrap();
            match interrupted_name {
                ATTACHMENT_STATUS_FILE => fs::remove_file(&paths.attachment_status).unwrap(),
                "tmux.conf" => fs::remove_file(&paths.config).unwrap(),
                "tmux.sock" => fs::remove_file(&paths.socket).unwrap(),
                _ => unreachable!(),
            }
            drop(marker);
            drop(proof);

            let restarted = classify_legacy_presentations(temporary.path()).unwrap();
            assert_eq!(restarted.state(), LegacyPresentationState::DeadOwned);
            let restarted_proof = restarted.proof().unwrap().clone();
            let lock = temporary.path().join("transition.lock");
            fs::write(&lock, b"").unwrap();
            set_mode(&lock, 0o600).unwrap();
            let lease = crate::state::acquire_transition_lease(temporary.path()).unwrap();
            remove_dead_legacy_presentation(temporary.path(), &restarted_proof, &lease).unwrap();
            assert!(matches!(
                classify_legacy_presentations(temporary.path()),
                Ok(assessment) if assessment.state() == LegacyPresentationState::None
            ));
        }
    }

    #[test]
    fn legacy_marker_malformed_or_replaced_refuses_without_cleanup() {
        for replaced in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            set_mode(temporary.path(), 0o700).unwrap();
            let presentation_root = temporary.path().join(PRESENTATION_DIRECTORY);
            fs::create_dir(&presentation_root).unwrap();
            set_mode(&presentation_root, 0o700).unwrap();
            let paths = PresentationPaths::fresh(temporary.path());
            fs::create_dir(&paths.directory).unwrap();
            set_mode(&paths.directory, 0o700).unwrap();
            fs::write(&paths.config, presentation_tmux_config()).unwrap();
            set_mode(&paths.config, 0o600).unwrap();
            let assessment = classify_legacy_presentations(temporary.path()).unwrap();
            let proof = assessment.proof().unwrap().clone();
            let marker = ensure_retirement_marker(temporary.path(), &proof).unwrap();
            let marker_path = paths.directory.join(LEGACY_RETIREMENT_MARKER_FILE);
            if replaced {
                let mut replacement = marker;
                replacement.directory = temporary.path().join("foreign");
                fs::write(&marker_path, serde_json::to_vec(&replacement).unwrap()).unwrap();
            } else {
                fs::write(&marker_path, b"not-json").unwrap();
            }
            set_mode(&marker_path, 0o600).unwrap();

            let classified = classify_legacy_presentations(temporary.path()).unwrap();
            assert_eq!(
                classified.state(),
                if replaced {
                    LegacyPresentationState::Foreign
                } else {
                    LegacyPresentationState::Malformed
                }
            );
            assert!(classified.proof().is_none());
            let lock = temporary.path().join("transition.lock");
            fs::write(&lock, b"").unwrap();
            set_mode(&lock, 0o600).unwrap();
            let lease = crate::state::acquire_transition_lease(temporary.path()).unwrap();
            let result = remove_dead_legacy_presentation(temporary.path(), &proof, &lease);
            assert!(result.is_err());
            assert!(paths.directory.exists());
            assert!(paths.config.exists());
        }
    }

    #[test]
    fn legacy_cleanup_rechecks_the_next_artifact_after_an_earlier_unlink() {
        let temporary = tempfile::tempdir().unwrap();
        set_mode(temporary.path(), 0o700).unwrap();
        let presentation_root = temporary.path().join(PRESENTATION_DIRECTORY);
        fs::create_dir(&presentation_root).unwrap();
        set_mode(&presentation_root, 0o700).unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        fs::create_dir(&paths.directory).unwrap();
        set_mode(&paths.directory, 0o700).unwrap();
        fs::write(&paths.config, presentation_tmux_config()).unwrap();
        set_mode(&paths.config, 0o600).unwrap();
        let status = LegacyAttachmentStatus {
            attempt_id: uuid::Uuid::new_v4(),
            host_alias: "local".to_owned(),
            workstream_id: WorkstreamId::new(),
            phase: AttachmentPhase::Pending,
        };
        fs::write(
            &paths.attachment_status,
            serde_json::to_vec(&status).unwrap(),
        )
        .unwrap();
        set_mode(&paths.attachment_status, 0o600).unwrap();

        let assessment = classify_legacy_presentations(temporary.path()).unwrap();
        assert_eq!(assessment.state(), LegacyPresentationState::DeadOwned);
        let proof = assessment.proof().unwrap().clone();
        let marker = ensure_retirement_marker(temporary.path(), &proof).unwrap();
        let replacement = b"set -g status on\n";
        let result =
            remove_exact_legacy_artifacts_with(temporary.path(), &proof, &marker, |path| {
                if path == paths.attachment_status.as_path() {
                    fs::write(&paths.config, replacement).unwrap();
                    set_mode(&paths.config, 0o600).unwrap();
                }
                Ok(())
            });
        assert!(matches!(result, Err(PresentationError::LegacyProofChanged)));
        assert!(!paths.attachment_status.exists());
        assert_eq!(fs::read(&paths.config).unwrap(), replacement);
        assert!(paths.directory.join(LEGACY_RETIREMENT_MARKER_FILE).exists());
    }
}
