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
    domain::{OnboardingPhase, Revision, RuntimeId, WorkstreamId},
    private_tmux::{
        COPY_MODE_SCROLL_BINDINGS, TERMINAL_CAPABILITY_CONFIG, copy_mode_scroll_config,
    },
    process::{BoundedProcessError, output_bounded},
    provisional::{
        PROVISIONAL_MARKER_FILE, ProvisionalPhase, ProvisionalSlot, cancel_pre_handoff_under_lease,
        read_marker, remove_exact_provisional_runtime_artifacts,
        retire_provider_exec_proven_marker, validate_exact_provisional_runtime_artifacts,
    },
    runtime::{LinuxProcessProbe, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux},
    state::{
        CurrentState, ProvisionalLease, StateRoot, current::OnboardingOperationInventory,
        open_current,
    },
};

mod attachment;
mod cleanup;
mod control;
mod ownership;
mod provisional;
mod topology;

#[cfg(test)]
use attachment::prepare_attach_window_with_size;
#[cfg(test)]
use cleanup::should_reuse_presentation;
use cleanup::{stopped_owned_presentation, validate_presentation_artifact_entries};
pub(crate) use control::retry_default_navigator_width;
#[cfg(test)]
use ownership::create_paths;
#[cfg(test)]
pub(crate) use ownership::create_paths_for_test;
use ownership::{
    PresentationArtifactSet, presentation_ownership_marker_path, read_presentation_ownership,
};
pub(crate) use provisional::classify_provisional_inventory;
use topology::{PresentationTopology, parse_topology, parse_topology_with_dead};

const PRESENTATION_DIRECTORY: &str = "presentation";
const PRESENTATION_PREFIX: &str = "wsnav-presentation-";
const NAVIGATOR_WINDOW: &str = "navigator";
const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
const NAVIGATOR_WIDTH_HOOKS: [&str; 2] = ["client-attached", "window-resized"];
/// The normal narrow navigator width, including its outside borders.
const DEFAULT_NAVIGATOR_PANE_WIDTH: u16 = 32;
const PREFERRED_PROVIDER_PANE_WIDTH: u16 = 96;
/// Detached tmux servers otherwise begin at 80 columns. Asking that window for
/// a 96-column provider pane can transiently squeeze the Navigator to one
/// column before its width hook exists, which races the first TUI draw on tmux
/// 3.4. Start with the exact intended two-pane width instead.
const INITIAL_PRESENTATION_WIDTH: u16 =
    DEFAULT_NAVIGATOR_PANE_WIDTH + PREFERRED_PROVIDER_PANE_WIDTH + 1;
const INITIAL_PRESENTATION_HEIGHT: u16 = 24;
const MAX_TMUX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_ATTACHMENT_STATUS_BYTES: u64 = 4 * 1024;
const MAX_PRESENTATION_ARTIFACT_ENTRIES: usize = 32;
const MAX_PRESENTATION_CONFIG_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENT_STATUS_BYTES_USIZE: usize = 4 * 1024;
const ATTACHMENT_STATUS_FILE: &str = "attachment.json";
const PRESENTATION_OWNERSHIP_MARKER_FILE: &str = "ownership.json";
const MAX_PRESENTATION_OWNERSHIP_MARKER_BYTES: usize = 4 * 1024;
const PRESENTATION_CONTEXT_VERSION: u8 = 1;
const MAX_PROVISIONAL_MARKER_BYTES: usize = 8 * 1024;
const MAX_PROVISIONAL_INVENTORY_ENTRIES: usize = 128;
const ROLE_OPTION: &str = "@wsnav_role";
const WORKSTREAM_OPTION: &str = "@wsnav_workstream_id";
const PRESENTATION_CLAIM_OPTION: &str = "@wsnav_presentation_claim";
const NAVIGATOR_STOP_ATTEMPTS: usize = 20;
const NAVIGATOR_STOP_RETRY: Duration = Duration::from_millis(5);
/// A private tmux server can briefly expose an incomplete pane topology while
/// it publishes a new pane or attaches its first controlling client.  Only
/// that exact transient topology observation may be retried, and only for
/// this bounded interval; persistent or unrelated failures remain refusals.
pub(crate) const INVALID_TOPOLOGY_RETRY_ATTEMPTS: usize = 20;
pub(crate) const INVALID_TOPOLOGY_RETRY_INTERVAL: Duration = Duration::from_millis(5);
// tmux 3.4 normalizes literal control separators in `-F` output. Every field
// using this printable separator is either an owned identifier/enum or is
// rejected fail-closed if the separator appears in free-form evidence.
const TMUX_FIELD_SEPARATOR: char = '|';
const TOPOLOGY_FORMAT: &str = "#{pane_id}|#{@wsnav_role}|#{@wsnav_workstream_id}|#{pane_dead}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}|#{window_width}|#{window_height}";
const PRESENTATION_TMUX_CONFIG_PREFIX: &str = concat!(
    "set -g status off\n",
    "set -g mouse on\n",
    "set -g remain-on-exit on\n",
    "set -g focus-events on\n",
    "set -g pane-border-status off\n",
    "set -g pane-border-format \"\"\n",
    "set -g pane-border-indicators off\n",
    "set -g pane-border-style fg=colour7\n",
    "set -g pane-active-border-style fg=colour7\n",
    "set -g prefix C-b\n",
    "set -g prefix2 None\n",
    "bind-key -T prefix F12 display-message \"\"\n",
    "bind-key -T root F12 display-message \"\"\n",
    "unbind-key -a -T prefix\n",
    "unbind-key -a -T root\n",
);
const PRESENTATION_TMUX_CONFIG_SUFFIX: &str = concat!(
    "bind-key -T root MouseUp1Pane send-keys -M\n",
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

fn expected_presentation_config_identity() -> PresentationFileIdentity {
    let config = presentation_tmux_config();
    let mut digest = Sha256::new();
    digest.update(config.as_bytes());
    PresentationFileIdentity {
        size: config.len() as u64,
        mode: 0o600,
        device: 0,
        inode: 0,
        digest: Some(digest.finalize().into()),
    }
}

fn config_content_matches(identity: &PresentationFileIdentity) -> bool {
    let expected = expected_presentation_config_identity();
    identity.size == expected.size && identity.digest == expected.digest
}

fn presentation_mouse_validation_command(
    paths: &PresentationPaths,
    executable: &Path,
    state_root: &Path,
) -> Result<String, PresentationError> {
    let executable = shell_quote(executable.as_os_str())?;
    let state_root = shell_quote(state_root.as_os_str())?;
    let socket = shell_quote(paths.socket.as_os_str())?;
    let session = shell_quote(paths.session_name.as_ref())?;
    Ok(format!(
        "exec {executable} --state-root {state_root} _presentation_mouse --presentation-socket {socket} --presentation-session {session} --target-pane '#{{mouse_pane}}' --client-name #{{q:client_name}}"
    ))
}

fn private_tmux_command() -> Command {
    let mut command = Command::new("tmux");
    command.env_remove("TMUX").arg("-u");
    command
}

/// Actions exposed by the private presentation prefix table. The strings are
/// fixed internal ABI values; no arbitrary tmux command can enter this path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationAction {
    SwitchPrevious,
    SwitchNext,
    FocusLeft,
    FocusRight,
    LiteralCtrlB,
}

impl PresentationAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SwitchPrevious => "switch-previous",
            Self::SwitchNext => "switch-next",
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
            "switch-previous" => Ok(Self::SwitchPrevious),
            "switch-next" => Ok(Self::SwitchNext),
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
    /// Process-local UI synchronization purpose. D18 status files omit this
    /// field and therefore decode as an ordinary Navigator attachment.
    #[serde(default)]
    pub purpose: AttachmentPurpose,
}

/// Distinguishes a provider-pane cycle from an ordinary Navigator attach
/// without adding durable UI state or changing the bounded status contract.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum AttachmentPurpose {
    #[default]
    Ordinary,
    ProviderCycle,
}

/// Immutable, presentation-private shell-onboarding context. The seed is
/// intentionally available only to the materializer; it never
/// enters a navigator snapshot, provider command, or durable host registry.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct PresentationContext {
    presentation_id: uuid::Uuid,
    presentation_revision: Revision,
    seed_cwd: PathBuf,
}

impl std::fmt::Debug for PresentationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PresentationContext")
            .field("presentation_id", &"<opaque>")
            .field("presentation_revision", &self.presentation_revision)
            .field("seed_cwd", &"<private>")
            .finish()
    }
}

impl PresentationContext {
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

/// Result of the read-only provisional-slot classifier. The caller must
/// hold the stable provisional lease before acting on this result; the
/// classifier itself never creates, adopts, removes, attaches, or signals a
/// presentation or Runtime artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProvisionalInventory {
    Vacant,
    Occupied,
}

/// Bounded refusal from 's cross-presentation provisional-slot inventory.
/// No path, marker body, operation identifier, shell evidence, or provider
/// content crosses this boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum ProvisionalInventoryError {
    #[error("provisional inventory is unavailable")]
    Unavailable,
    #[error("provisional inventory is ambiguous")]
    Ambiguous,
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

/// Exact pre-handoff provisional evidence captured before presentation
/// close. It is deliberately private: it carries cleanup authority only for
/// this presentation's materialized account shell, never for a managed
/// Runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProvisionalCleanupProof {
    slot: ProvisionalSlot,
    marker_identity: PresentationFileIdentity,
}

/// Stable identity for one owned regular file or private socket.  Device and
/// inode are populated on Unix; size/mode remain useful on other platforms.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PresentationFileIdentity {
    pub size: u64,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
    pub digest: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresentationProbeFailure {
    Malformed,
    Foreign,
    Inaccessible,
}

impl PresentationProbeFailure {
    const fn into_probe(self) -> Self {
        self
    }
}

fn map_presentation_ownership_probe(failure: PresentationProbeFailure) -> PresentationError {
    match failure {
        PresentationProbeFailure::Inaccessible => PresentationError::ControlRefused(
            "presentation ownership artifact could not be inspected safely",
        ),
        PresentationProbeFailure::Foreign => PresentationError::ControlRefused(
            "presentation ownership artifact is foreign or symlinked",
        ),
        PresentationProbeFailure::Malformed => {
            PresentationError::ControlRefused("presentation ownership artifact is malformed")
        }
    }
}

fn classify_presentation_fs_error(error: &std::io::Error) -> PresentationProbeFailure {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied
        | std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::TimedOut => PresentationProbeFailure::Inaccessible,
        _ => PresentationProbeFailure::Malformed,
    }
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
    directory_identity: PresentationFileIdentity,
    config_identity: PresentationFileIdentity,
    socket_identity: Option<PresentationFileIdentity>,
    /// Omitted from the initial marker so its serialized shape remains
    /// unchanged until the presentation context is initialized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<PresentationMarker>,
}

/// The bounded fields carried by the existing presentation-ownership
/// marker. Keeping this embedded prevents a second loose artifact from being
/// mistaken for presentation authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PresentationMarker {
    version: u8,
    presentation_id: uuid::Uuid,
    presentation_revision: Revision,
    seed_cwd: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresentationOwnershipProof {
    marker: PresentationOwnershipMarker,
    marker_identity: PresentationFileIdentity,
    socket_identity: Option<PresentationFileIdentity>,
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

fn remove_exact_regular_artifact<F>(
    path: &Path,
    expected: Option<&PresentationFileIdentity>,
    max_bytes: usize,
    after_remove: &mut F,
) -> Result<(), PresentationError>
where
    F: FnMut(&Path) -> Result<(), PresentationError>,
{
    // This is deliberately repeated for each unlink.  An interruption hook
    // or another actor may replace a later artifact after an earlier unlink;
    // the replacement must fail identity validation before it is touched.
    let actual =
        inspect_regular_file(path, false, max_bytes).map_err(map_presentation_ownership_probe)?;
    if !optional_identity_compatible(expected, actual.as_ref()) {
        return Err(PresentationError::ControlRefused(
            "presentation artifact identity changed",
        ));
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
    expected: Option<&PresentationFileIdentity>,
    after_remove: &mut F,
) -> Result<(), PresentationError>
where
    F: FnMut(&Path) -> Result<(), PresentationError>,
{
    let actual = inspect_private_socket(path).map_err(map_presentation_ownership_probe)?;
    if !optional_identity_compatible(expected, actual.as_ref()) {
        return Err(PresentationError::ControlRefused(
            "presentation socket identity changed",
        ));
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

fn inspect_regular_file(
    path: &Path,
    required: bool,
    max_bytes: usize,
) -> Result<Option<PresentationFileIdentity>, PresentationProbeFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => return Ok(None),
        Err(error) => return Err(classify_presentation_fs_error(&error).into_probe()),
    };
    if metadata.file_type().is_symlink() {
        return Err(PresentationProbeFailure::Foreign);
    }
    if !metadata.is_file() {
        return Err(PresentationProbeFailure::Malformed);
    }
    if !is_private_owner_file(&metadata) {
        return Err(PresentationProbeFailure::Foreign);
    }
    let bytes =
        read_private_file(path, max_bytes)?.ok_or(PresentationProbeFailure::Inaccessible)?;
    Ok(Some(presentation_file_identity(&metadata, Some(&bytes))))
}

fn inspect_private_socket(
    path: &Path,
) -> Result<Option<PresentationFileIdentity>, PresentationProbeFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(classify_presentation_fs_error(&error).into_probe()),
    };
    if metadata.file_type().is_symlink() {
        return Err(PresentationProbeFailure::Foreign);
    }
    if !file_type_is_socket(&metadata) {
        return Err(PresentationProbeFailure::Malformed);
    }
    if !is_private_owner_socket(&metadata) {
        return Err(PresentationProbeFailure::Foreign);
    }
    Ok(Some(presentation_file_identity(&metadata, None)))
}

fn read_private_file(
    path: &Path,
    maximum: usize,
) -> Result<Option<Vec<u8>>, PresentationProbeFailure> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(classify_presentation_fs_error(&error).into_probe()),
    };
    let mut bytes = Vec::new();
    file.take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| classify_presentation_fs_error(&error).into_probe())?;
    if bytes.len() > maximum {
        return Err(PresentationProbeFailure::Malformed);
    }
    Ok(Some(bytes))
}

fn presentation_file_identity(
    metadata: &fs::Metadata,
    bytes: Option<&[u8]>,
) -> PresentationFileIdentity {
    PresentationFileIdentity {
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

fn count_client_rows(
    output: &str,
    expected_session: &str,
) -> Result<usize, PresentationProbeFailure> {
    let mut count = 0;
    for line in output.lines() {
        if line.is_empty() || count >= MAX_PRESENTATION_ARTIFACT_ENTRIES {
            return Err(PresentationProbeFailure::Malformed);
        }
        let mut fields = line.split(TMUX_FIELD_SEPARATOR);
        let Some(name) = fields.next() else {
            return Err(PresentationProbeFailure::Malformed);
        };
        let Some(session) = fields.next() else {
            return Err(PresentationProbeFailure::Malformed);
        };
        let Some(window_name) = fields.next() else {
            return Err(PresentationProbeFailure::Malformed);
        };
        if fields.next().is_some()
            || name.is_empty()
            || session != expected_session
            || window_name != NAVIGATOR_WINDOW
        {
            return Err(PresentationProbeFailure::Malformed);
        }
        count += 1;
    }
    Ok(count)
}

fn sync_directory(path: &Path) -> Result<(), PresentationError> {
    let directory = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(PresentationError::Io)?;
    directory.sync_all().map_err(PresentationError::Io)
}

fn directory_identity_compatible(
    expected: &PresentationFileIdentity,
    actual: &PresentationFileIdentity,
) -> bool {
    expected.mode == actual.mode
        && expected.device == actual.device
        && expected.inode == actual.inode
}

fn optional_identity_compatible(
    expected: Option<&PresentationFileIdentity>,
    actual: Option<&PresentationFileIdentity>,
) -> bool {
    actual.is_none() || actual == expected
}

fn socket_identity_compatible(
    expected: &PresentationFileIdentity,
    actual: &PresentationFileIdentity,
) -> bool {
    private_socket_mode(expected.mode)
        && private_socket_mode(actual.mode)
        && expected.size == actual.size
        && expected.device == actual.device
        && expected.inode == actual.inode
        && expected.digest == actual.digest
}

fn optional_socket_identity_compatible(
    expected: Option<&PresentationFileIdentity>,
    actual: Option<&PresentationFileIdentity>,
) -> bool {
    match (expected, actual) {
        (_, None) => true,
        (Some(expected), Some(actual)) => socket_identity_compatible(expected, actual),
        (None, Some(_)) => false,
    }
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

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), PresentationError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(PresentationError::Io)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), PresentationError> {
    Ok(())
}

/// Presentation ownership failures; no provider content is retained in their
/// diagnostics.
#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("multiple private navigator presentations are live; close one before reconnecting")]
    AmbiguousPresentations,
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
    #[error("the presentation context is unavailable")]
    ContextUnavailable,
    #[error("the presentation context is already initialized")]
    ContextAlreadyInitialized,
    #[error("the presentation context is invalid")]
    ContextInvalid,
    #[error("the presentation seed cwd is unavailable")]
    SeedUnavailable,
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
        let detached = PresentationFileIdentity {
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
                let expected =
                    format!("bind-key -T {table} {key} send-keys -X -N {repeat_count} {direction}");
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
        presentation
            .start_with_context(uuid::Uuid::from_u128(0x1706), temporary.path())
            .unwrap();
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
        assert_geometry(&presentation_socket, &presentation_target, "129x24");
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
                "#{pane_id}|#{@wsnav_role}|#{pane_width}|#{pane_height}",
            ]
            .as_slice(),
        );
        let provider = panes
            .lines()
            .find_map(|line| {
                let mut fields = line.split(TMUX_FIELD_SEPARATOR);
                let pane_id = fields.next()?;
                let role = fields.next()?;
                let columns = fields.next()?.parse::<u16>().ok()?;
                let rows = fields.next()?.parse::<u16>().ok()?;
                (role == "provider").then_some((pane_id.to_owned(), columns, rows))
            })
            .unwrap_or_else(|| panic!("provider pane geometry missing from {panes:?}"));
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
    fn context_is_marker_bound_and_uses_only_the_canonical_seed() {
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
            .initialize_context(presentation_id, &seed)
            .unwrap();
        assert_eq!(context.presentation_id(), presentation_id);
        assert_eq!(context.presentation_revision(), Revision::INITIAL);
        assert_eq!(context.seed_cwd(), seed.canonicalize().unwrap());
        assert_eq!(presentation.context().unwrap(), context);
        assert!(matches!(
            presentation.initialize_context(uuid::Uuid::from_u128(72), &seed),
            Err(PresentationError::ContextAlreadyInitialized)
        ));

        let marker = fs::read(paths.directory.join(PRESENTATION_OWNERSHIP_MARKER_FILE)).unwrap();
        assert!(String::from_utf8(marker).unwrap().contains("\"current\":{"));
    }

    #[test]
    fn context_reopens_only_the_exact_owned_directory_after_slot_materialization() {
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
            .initialize_context(presentation_id, &seed)
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
            Presentation::context_from_directory(temporary.path(), &paths.directory).unwrap(),
            context
        );
        assert!(matches!(
            Presentation::context_from_directory(temporary.path(), &seed),
            Err(PresentationError::ContextUnavailable)
        ));
    }

    #[test]
    fn close_removes_an_owned_presentation_without_a_provisional_shell() {
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
        presentation
            .initialize_context(uuid::Uuid::from_u128(75), &seed)
            .unwrap();

        presentation.close().unwrap();

        assert!(!paths.directory.exists());
    }

    #[test]
    fn close_recovers_an_interrupted_exact_observer_review() {
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
        let context = presentation
            .initialize_context(uuid::Uuid::from_u128(7_501), &seed)
            .unwrap();
        let review = std::mem::ManuallyDrop::new(
            crate::review::ReviewDirectory::create(
                &paths.directory,
                context.presentation_id(),
                context.presentation_revision(),
            )
            .unwrap(),
        );
        assert!(review.path().exists());

        presentation.close().unwrap();

        assert!(!paths.directory.exists());
    }

    #[test]
    fn close_preserves_a_replaced_observer_review_directory() {
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
        let context = presentation
            .initialize_context(uuid::Uuid::from_u128(7_502), &seed)
            .unwrap();
        let review = std::mem::ManuallyDrop::new(
            crate::review::ReviewDirectory::create(
                &paths.directory,
                context.presentation_id(),
                context.presentation_revision(),
            )
            .unwrap(),
        );
        let review_path = review.path();
        fs::remove_dir(&review_path).unwrap();
        fs::create_dir(&review_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&review_path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(review_path.join("foreign"), b"preserve").unwrap();

        assert!(presentation.close().is_err());
        assert_eq!(fs::read(review_path.join("foreign")).unwrap(), b"preserve");
        assert!(paths.directory.exists());
    }

    #[test]
    fn startup_failure_uses_marker_aware_cleanup_owner() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        let mut state =
            crate::state::create_current(&state_path, &crate::domain::RandomIdGenerator).unwrap();
        let provisional_lease = state.acquire_provisional_lease().unwrap();
        let presentation =
            Presentation::fresh_with_executable(&state_path, PathBuf::from("/workspace/wsnav"));
        let paths = presentation.paths().clone();
        create_paths(&paths).unwrap();
        let context = presentation
            .initialize_context(uuid::Uuid::from_u128(8_000), &seed)
            .unwrap();
        let slot = crate::provisional::ProvisionalSlot::materializing(
            &state_path,
            context.presentation_id(),
            context.presentation_revision(),
            provisional_lease.lease_generation(),
            crate::domain::RuntimeId::from(uuid::Uuid::from_u128(8_001)),
            crate::provisional::SlotGeneration::new(uuid::Uuid::from_u128(8_002)),
            &seed,
        )
        .unwrap();
        crate::provisional::write_new_marker(&state_path, &paths.directory, &slot).unwrap();
        drop(state);
        drop(provisional_lease);

        let failure = presentation.complete_start_stage(
            "synthetic stage",
            Err::<(), _>(PresentationError::ControlRefused("synthetic failure")),
        );
        assert!(matches!(
            failure,
            Err(PresentationError::StartupFailed { .. })
        ));
        assert!(!paths.directory.exists());
    }

    #[test]
    fn provisional_inventory_allows_one_marker_but_refuses_stale_journal_or_runtime_artifact() {
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
            .initialize_context(presentation_id, &seed)
            .unwrap();
        assert_eq!(
            Presentation::context_from_directory(temporary.path(), &paths.directory).unwrap(),
            context
        );

        assert_eq!(
            classify_provisional_inventory(temporary.path(), &[], &[]).unwrap(),
            ProvisionalInventory::Vacant
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
            classify_provisional_inventory(temporary.path(), &[], &[]).unwrap(),
            ProvisionalInventory::Occupied
        );
        let stale = OnboardingOperationInventory {
            operation_id: crate::domain::OperationId::from(uuid::Uuid::from_u128(79)),
            workstream_id: WorkstreamId::from(uuid::Uuid::from_u128(80)),
            runtime_id: candidate_runtime_id,
            phase: OnboardingPhase::CapabilityIssued,
        };
        assert_eq!(
            classify_provisional_inventory(temporary.path(), &[], &[stale]),
            Err(ProvisionalInventoryError::Ambiguous)
        );

        let other = tempfile::tempdir().unwrap();
        set_mode(other.path(), 0o700).unwrap();
        let run = other.path().join("run");
        fs::create_dir(&run).unwrap();
        set_mode(&run, 0o700).unwrap();
        fs::create_dir(run.join("runtime-foreign")).unwrap();
        assert_eq!(
            classify_provisional_inventory(other.path(), &[], &[]),
            Err(ProvisionalInventoryError::Ambiguous)
        );

        let completed = tempfile::tempdir().unwrap();
        set_mode(completed.path(), 0o700).unwrap();
        let completed_runtime = crate::domain::RuntimeId::from(uuid::Uuid::from_u128(86));
        let completed_operation = OnboardingOperationInventory {
            operation_id: crate::domain::OperationId::from(uuid::Uuid::from_u128(87)),
            workstream_id: WorkstreamId::from(uuid::Uuid::from_u128(88)),
            runtime_id: completed_runtime,
            phase: OnboardingPhase::ProviderExecProven,
        };
        assert_eq!(
            classify_provisional_inventory(
                completed.path(),
                &[RuntimePaths::for_runtime(
                    completed.path(),
                    completed_runtime
                )],
                &[completed_operation],
            )
            .unwrap(),
            ProvisionalInventory::Vacant,
            "a terminal onboarding journal without its retired marker leaves the singleton vacant"
        );
    }

    #[test]
    fn host_materialization_validation_requires_exact_vacant_context_and_lease() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        drop(crate::state::create_current(&state_path, &crate::domain::RandomIdGenerator).unwrap());

        let root = crate::state::StateRoot::select(&state_path);

        let presentation =
            Presentation::fresh_with_executable(&state_path, PathBuf::from("/workspace/wsnav"));
        create_paths(presentation.paths()).unwrap();
        let presentation_id = uuid::Uuid::from_u128(81);
        let context = presentation
            .initialize_context(presentation_id, &seed)
            .unwrap();
        let mut state = crate::state::open_current(&root).unwrap();
        let provisional_lease = state.acquire_provisional_lease().unwrap();
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
    fn fresh_presentation_marker_omits_until_context_initialization() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        create_paths(&paths).unwrap();

        let marker = fs::read(paths.directory.join(PRESENTATION_OWNERSHIP_MARKER_FILE)).unwrap();
        assert!(!marker.windows(5).any(|window| window == b"\"current\""));
    }

    #[test]
    fn context_refuses_a_deleted_seed_without_fallback() {
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
            .initialize_context(uuid::Uuid::from_u128(73), &seed)
            .unwrap();
        fs::remove_dir(&seed).unwrap();

        assert!(matches!(
            presentation.context(),
            Err(PresentationError::SeedUnavailable)
        ));
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
    fn presentation_config_selects_the_clicked_pane_on_mouse_press() {
        let config = presentation_tmux_config();
        assert!(config.contains("set -g mouse on"));
        assert!(config.contains("set -g focus-events on"));
        assert!(config.contains("set -g pane-border-status off"));
        assert!(config.contains("set -g pane-border-format \"\""));
        assert!(config.contains("set -g pane-border-indicators off"));
        assert!(config.contains("set -g pane-border-style fg=colour7"));
        assert!(config.contains("set -g pane-active-border-style fg=colour7"));
        assert!(!config.contains("ACTIVE"));
        assert!(!config.contains("MouseDown1Pane"));
        assert!(config.contains("bind-key -T root MouseUp1Pane send-keys -M"));
        assert!(config.contains("WheelUpPane"));
        assert!(!config.contains("MouseDown3Pane"));
    }

    #[test]
    fn presentation_config_rebuilds_bounded_allowlists() {
        let config = presentation_tmux_config();
        for expected in [
            "set -g status off",
            "set -g mouse on",
            "set -g focus-events on",
            "set -g pane-border-status off",
            "set -g pane-border-format \"\"",
            "set -g pane-border-indicators off",
            "set -g pane-border-style fg=colour7",
            "set -g pane-active-border-style fg=colour7",
            "bind-key -T prefix F12 display-message \"\"",
            "bind-key -T root F12 display-message \"\"",
            "bind-key -T root MouseUp1Pane send-keys -M",
            "bind-key -T root WheelUpPane if-shell",
        ] {
            assert!(config.contains(expected), "missing {expected:?}");
        }
        assert!(!config.contains("MouseDown1Pane"));
        assert!(!config.contains("split-window"));
        assert!(!config.contains("command-prompt"));
    }

    #[test]
    fn topology_parser_rejects_dead_duplicate_and_unknown_roles() {
        let valid = concat!(
            "%0|navigator||0|0|0|32|24|128|24\n",
            "%1|provider|01234567-89ab-cdef-0123-456789abcdef|0|33|0|95|24|128|24\n",
        );
        assert!(parse_topology(valid).is_ok());
        assert!(matches!(
            parse_topology(&valid.replace("|0|0|0|32", "|1|0|0|32")),
            Err(PresentationError::InvalidTopology)
        ));
        let duplicate = valid.replace("%1|provider", "%0|provider");
        assert!(matches!(
            parse_topology(&duplicate),
            Err(PresentationError::InvalidTopology)
        ));
        let unknown = valid.replace("provider", "unknown");
        assert!(matches!(
            parse_topology(&unknown),
            Err(PresentationError::InvalidTopology)
        ));
        assert!(parse_topology_with_dead(&valid.replace("|0|0|0|32", "|1|0|0|32"), true).is_ok());
    }

    #[test]
    fn topology_parser_rejects_unsupported_geometry() {
        let valid = concat!(
            "%0|navigator||0|0|0|32|24|128|24\n",
            "%1|provider|01234567-89ab-cdef-0123-456789abcdef|0|33|0|95|24|128|24\n",
        );
        assert!(parse_topology(valid).is_ok());
        assert!(parse_topology(&valid.replace("|33|0|95|24", "|34|0|94|24")).is_err());
        assert!(parse_topology(&valid.replace("|128|24", "|127|24")).is_err());

        let bordered = valid
            .replace("|0|0|32|24|128", "|0|1|32|23|128")
            .replace("|0|33|0|95|24|128", "|0|33|1|95|23|128");
        assert!(parse_topology(&bordered).is_ok());
        let arbitrary_offset = bordered
            .replace("|0|1|32|23|128", "|0|2|32|22|128")
            .replace("|0|33|1|95|23|128", "|0|33|2|95|22|128");
        assert!(parse_topology(&arbitrary_offset).is_err());

        let three_pane = concat!(
            "%0|navigator||0|0|0|32|24|128|24\n",
            "%1|provider|01234567-89ab-cdef-0123-456789abcdef|0|33|0|95|11|128|24\n",
            "%2|utility|01234567-89ab-cdef-0123-456789abcdef|0|33|12|95|12|128|24\n",
        );
        assert!(parse_topology(three_pane).is_ok());
        assert!(parse_topology(&three_pane.replace("|12|95|12|128", "|11|95|13|128")).is_err());
        assert!(parse_topology(&three_pane.replace("|33|12|95|12", "|34|12|94|12")).is_err());
    }

    #[test]
    fn control_binding_uses_fixed_quoting_and_tmux_format_source() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state's root/#{danger}/#(marker)");
        let presentation = Presentation {
            paths: PresentationPaths::fresh(&state_root),
            executable: PathBuf::from("/tmp/wsnav's executable/#{danger}/#(marker)"),
            state_root,
        };

        let command = presentation
            .control_shell_command(PresentationAction::SwitchPrevious)
            .unwrap();

        assert!(command.contains("'/tmp/wsnav'\\''s executable/##{danger}/##(marker)'"));
        assert!(command.contains("##{danger}"));
        assert!(command.contains("##(marker)"));
        let source_only = command.replace("##{danger}", "").replace("##(marker)", "");
        assert_eq!(source_only.matches("#{").count(), 2);
        assert!(!source_only.contains("#("));
        assert!(command.contains("--action switch-previous"));
        assert!(command.contains("--source-pane '#{pane_id}'"));
        assert!(command.contains("--client-name #{q:client_name}"));
        assert!(!command.contains("; tmux"));
        assert!(!command.contains("split-window"));
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
    fn detached_presentation_starts_at_the_exact_two_pane_geometry() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let arguments = presentation.new_session_arguments();

        assert_eq!(INITIAL_PRESENTATION_WIDTH, 129);
        assert_eq!(INITIAL_PRESENTATION_HEIGHT, 24);
        assert_eq!(arguments[0], "new-session");
        assert_eq!(arguments[1], "-d");
        assert_eq!(arguments[2], "-x");
        assert_eq!(arguments[3], "129");
        assert_eq!(arguments[4], "-y");
        assert_eq!(arguments[5], "24");
        assert_eq!(arguments[6], "-s");
        assert_eq!(arguments[8], "-n");
        assert_eq!(arguments[9], NAVIGATOR_WINDOW);
        assert_eq!(arguments[13], "_navigator");
    }

    #[test]
    fn navigator_command_uses_the_current_hidden_route() {
        let temporary = tempfile::tempdir().unwrap();
        let presentation = Presentation {
            paths: PresentationPaths::fresh(temporary.path()),
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };

        let command = presentation
            .navigator_command()
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command[3], "_navigator");
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
        let pending = presentation
            .prepare_attachment_with_purpose(workstream_id, AttachmentPurpose::Ordinary)
            .unwrap();

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
            .prepare_attachment_with_purpose(WorkstreamId::new(), AttachmentPurpose::Ordinary)
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
            .prepare_attachment_with_purpose(WorkstreamId::new(), AttachmentPurpose::Ordinary)
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
    fn provider_attachment_carries_exact_snapshot_revisions() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = PresentationPaths::fresh(temporary.path());
        let presentation = Presentation {
            paths,
            executable: PathBuf::from("/workspace/wsnav"),
            state_root: temporary.path().to_path_buf(),
        };
        let workstream_id = WorkstreamId::from(uuid::Uuid::from_u128(71));
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(72));
        let command = presentation.provider_attach_command(
            workstream_id,
            Revision::INITIAL,
            runtime_id,
            Revision::INITIAL,
            uuid::Uuid::from_u128(73),
            AttachmentPurpose::Ordinary,
        );

        assert!(
            command
                .iter()
                .all(|argument| argument != "sh" && argument != "/bin/sh")
        );
        assert_eq!(command[3], "_provider_attach");
        assert_eq!(command[4], OsString::from(workstream_id.to_string()));
        assert_eq!(command[5], "--expected-workstream-revision");
        assert_eq!(
            command[6],
            OsString::from(Revision::INITIAL.value().to_string())
        );
        assert_eq!(command[7], "--expected-runtime-id");
        assert_eq!(command[8], OsString::from(runtime_id.to_string()));
        assert_eq!(command[9], "--expected-runtime-revision");
        assert_eq!(
            command[10],
            OsString::from(Revision::INITIAL.value().to_string())
        );
        assert_eq!(command.len(), 17);
    }

    #[test]
    fn provisional_attachment_uses_only_the_exact_private_tmux_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = RuntimePaths::for_runtime(
            temporary.path(),
            crate::domain::RuntimeId::from(uuid::Uuid::from_u128(87)),
        );

        let command = Presentation::provisional_attach_command(&runtime);

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
}
