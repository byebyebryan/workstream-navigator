//! Onboarding journal transactions and provider-exec ownership.
//!
//! This module owns the exact current-schema onboarding state machine. It
//! accepts only a lease held by the current presentation and never persists
//! provider payloads or terminal output.

use super::{
    CapabilityError, CompoundOperation, CurrentState, IdGenerator, LocationId,
    MAX_NAVIGATOR_WORKSTREAMS, ONBOARDING_CANCELLED_OUTCOME, OnboardingPhase, OperationId,
    OperationKind, OptionalExtension, PARKED_RECOVERY_RESOLVED_OUTCOME, Path, PathBuf, ProjectId,
    ProviderKind, ProviderSessionId, RepositoryDiscovery, Revision, RuntimeId, RuntimePaths,
    StateError, StateMode, Uuid, WorkstreamId, bind_opencode_session_in_transaction,
    bump_project_revision, create_project, ensure_current_mode, find_project_by_fingerprint,
    load_binding, load_opencode_handle, next_activity_sequence, operation_phase_from_text,
    operation_phase_text, provider_kind_from_text, runtime_status_from_text,
    validate_project_display_name, validate_project_membership_transaction,
    validate_provider_metadata, validate_registry_text, validate_repository_fingerprint,
    validate_safe_origin_display, validate_schema15, verify_launch_capability,
};

use std::{fs, time::Duration};

use rusqlite::{TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{
    domain::RuntimeStatus,
    onboarding::{LaunchCapability, LaunchCapabilityClaims, LaunchCapabilityMetadata},
    state::{OpenCodeObserverStatus, OpenCodeRuntimeHandle, ProvisionalLease, RuntimeRecord},
};

pub(crate) struct OnboardingPrepareRequest {
    pub(crate) request_key: String,
    pub(crate) presentation_id: Uuid,
    pub(crate) presentation_revision: Revision,
    pub(crate) slot_generation: Uuid,
    pub(crate) candidate_runtime_id: RuntimeId,
    pub(crate) runtime_paths: RuntimePaths,
    pub(crate) provider: ProviderKind,
    pub(crate) repository: RepositoryDiscovery,
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
pub(crate) struct OnboardingReservation {
    operation_id: OperationId,
    #[cfg(test)]
    workstream_id: WorkstreamId,
    capability: LaunchCapability,
}

impl std::fmt::Debug for OnboardingReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OnboardingReservation")
            .field("operation_id", &"<opaque>")
            .field("workstream_id", &"<opaque>")
            .field("capability", &self.capability)
            .finish()
    }
}

impl OnboardingReservation {
    #[must_use]
    pub(crate) const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn workstream_id(&self) -> WorkstreamId {
        self.workstream_id
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn capability(&self) -> &LaunchCapability {
        &self.capability
    }

    /// Transfers the live capability only to the crate-private broker
    /// channel. Durable state never receives this value.
    pub(crate) fn into_capability(self) -> LaunchCapability {
        self.capability
    }
}

/// A request-key replay that found the one existing unresolved onboarding
/// journal.  It never reissues the lost live token or creates another graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub(crate) struct ExistingOnboardingReservation {
    pub(crate) operation_id: OperationId,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_id: RuntimeId,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum OnboardingPreparation {
    Issued(OnboardingReservation),
    Existing(ExistingOnboardingReservation),
}

/// The only state-side result of an exact helper capability consumption. It
/// establishes durable Runtime ownership but deliberately does not grant
/// attach/action authority or imply provider exec proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub(crate) struct OnboardingOwnership {
    pub(crate) operation_id: OperationId,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_id: RuntimeId,
    pub(crate) operation_revision: Revision,
}

/// The only onboarding states that alter the passive Workstreams
/// projection. A reservation remains presentation-private until the helper
/// has committed Runtime ownership; later unproven or recovery states stay
/// visible but never grant ordinary action authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OnboardingVisibility {
    Reserved,
    ActionFenced,
    RecoveryRequired,
}

/// A bounded relationship between one onboarding journal and its exact
/// reserved Runtime. It intentionally excludes operation IDs, paths, shell
/// evidence, capability metadata, and provider payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OnboardingWorkstreamProjection {
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_id: RuntimeId,
    pub(crate) visibility: OnboardingVisibility,
}

/// One exact onboarding journal relationship retained only for the
/// provisional-slot singleton classifier. It is never a navigator snapshot:
/// operation identity, paths, shell evidence, and capability metadata remain
/// outside the display surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OnboardingOperationInventory {
    pub(crate) operation_id: OperationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_id: RuntimeId,
    pub(crate) phase: OnboardingPhase,
}

pub(crate) struct OnboardingOperationInventoryPage {
    pub(crate) operations: Vec<OnboardingOperationInventory>,
    pub(crate) workstream_ids: Vec<WorkstreamId>,
    pub(crate) next_cursor: Option<u32>,
}

pub(crate) struct RuntimePathsPage {
    pub(crate) paths: Vec<RuntimePaths>,
    pub(crate) next_cursor: Option<u32>,
}

pub(crate) type OnboardingMarkerRow = (String, String, String);

pub(crate) fn page_parameters(page_size: usize) -> Result<(i64, u32), StateError> {
    if page_size == 0 || page_size > MAX_NAVIGATOR_WORKSTREAMS {
        return Err(StateError::InvalidNavigatorPageSize);
    }
    let query_limit = i64::try_from(page_size)
        .map_err(|_| StateError::InvalidNavigatorPageSize)?
        .checked_add(1)
        .ok_or(StateError::InvalidNavigatorPageSize)?;
    let cursor_step = u32::try_from(page_size).map_err(|_| StateError::InvalidNavigatorPageSize)?;
    Ok((query_limit, cursor_step))
}

/// The bounded durable phase associated with one exact presentation marker.
/// This read is intentionally smaller than the journal inventory: callers use
/// it only to reconcile a marker crash window, never to build a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OnboardingMarkerOperation {
    pub(crate) operation_id: OperationId,
    pub(crate) phase: OnboardingPhase,
}

/// Bounded file identity captured from the exact native provider executable
/// immediately before the helper transfers into provider preparation. It is
/// durable proof input only: the executable path and command line are never
/// stored in the onboarding journal or exposed through a snapshot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct OnboardingProviderExecutableIdentity {
    device: u64,
    inode: u64,
}

impl std::fmt::Debug for OnboardingProviderExecutableIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OnboardingProviderExecutableIdentity(<opaque>)")
    }
}

impl OnboardingProviderExecutableIdentity {
    pub(crate) fn new(device: u64, inode: u64) -> Result<Self, StateError> {
        if inode == 0 || i64::try_from(device).is_err() || i64::try_from(inode).is_err() {
            return Err(StateError::InvalidOnboardingPreparation);
        }
        Ok(Self { device, inode })
    }

    #[must_use]
    pub(crate) const fn device(self) -> u64 {
        self.device
    }

    #[must_use]
    pub(crate) const fn inode(self) -> u64 {
        self.inode
    }
}

/// Bounded process evidence supplied only after an external reconciler has
/// proved that the adopted private pane still contains the expected native
/// provider executable. State stores no executable path or command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OnboardingProviderExecEvidence {
    provider_pid: u32,
    provider_birth: String,
}

impl OnboardingProviderExecEvidence {
    pub(crate) fn new(provider_pid: u32, provider_birth: String) -> Result<Self, StateError> {
        if provider_pid == 0 {
            return Err(StateError::InvalidOnboardingPreparation);
        }
        validate_registry_text("provider birth", &provider_birth)
            .map_err(|_| StateError::InvalidOnboardingPreparation)?;
        Ok(Self {
            provider_pid,
            provider_birth,
        })
    }
}

/// The private durable target against which a reconciler proves one native
/// provider exec. It deliberately remains crate-visible: the repository path
/// and runtime generation are necessary for local proof but never belong in a
/// public snapshot or diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OnboardingProviderExecTarget {
    ownership: OnboardingOwnership,
    provider: ProviderKind,
    project_root: PathBuf,
    runtime_generation: String,
    executable_identity: OnboardingProviderExecutableIdentity,
}

impl OnboardingProviderExecTarget {
    #[must_use]
    pub(crate) const fn ownership(&self) -> OnboardingOwnership {
        self.ownership
    }

    #[must_use]
    pub(crate) const fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub(crate) fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub(crate) fn runtime_generation(&self) -> &str {
        &self.runtime_generation
    }

    #[must_use]
    pub(crate) const fn executable_identity(&self) -> OnboardingProviderExecutableIdentity {
        self.executable_identity
    }
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
pub(crate) struct PersistedOnboardingIntent {
    pub(crate) version: u8,
    pub(crate) presentation_id: Uuid,
    pub(crate) presentation_revision: Revision,
    pub(crate) slot_generation: Uuid,
    pub(crate) lease_generation: i64,
    pub(crate) candidate_runtime_id: RuntimeId,
    pub(crate) provider: ProviderKind,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) runtime_generation: String,
    pub(crate) registry_generation: String,
    pub(crate) argv_digest: String,
    pub(crate) boot_provenance: String,
    /// Whether this onboarding attempt inserted its Location row.  Older
    /// schema-15 journals omit this field and therefore fail closed against
    /// destructive location cleanup during recovery.
    #[serde(default)]
    pub(crate) location_created: bool,
    /// Whether the inserted Location also created its Project row.  This is
    /// only cleanup authority for an exact fresh attempt graph.
    #[serde(default)]
    pub(crate) project_created: bool,
}

/// The current state handle is the only authority permitted to mutate
/// onboarding state.
#[derive(Clone, Copy)]
enum OnboardingAuthority {
    Current,
}

/// The private mutation class for one onboarding journal transition.
/// Known-absent Codex exec failure is deliberately distinct from ordinary
/// phase progression because it can be recorded only after exact proof that
/// no provider process, binding, or earlier external effect exists.
#[derive(Clone, Copy)]
enum OnboardingAdvance {
    Normal(OnboardingPhase),
    OpenCodeExternalEffectStarted,
    CodexExecFailedKnownAbsent,
}

impl OnboardingAdvance {
    const fn next(self) -> OnboardingPhase {
        match self {
            Self::Normal(next) => next,
            Self::OpenCodeExternalEffectStarted => OnboardingPhase::ProviderExternalEffectStarted,
            Self::CodexExecFailedKnownAbsent => OnboardingPhase::KnownAbsentExec,
        }
    }

    const fn effect_watermark(self) -> Option<&'static str> {
        match self {
            Self::Normal(_) => None,
            Self::OpenCodeExternalEffectStarted => Some(OPENCODE_EXTERNAL_EFFECT_STARTED_WATERMARK),
            Self::CodexExecFailedKnownAbsent => Some(CODEX_EXEC_FAILED_KNOWN_ABSENT_WATERMARK),
        }
    }

    const fn requires_codex_known_absence(self) -> bool {
        matches!(self, Self::CodexExecFailedKnownAbsent)
    }

    const fn requires_opencode_external_effect(self) -> bool {
        matches!(self, Self::OpenCodeExternalEffectStarted)
    }
}

const OPENCODE_EXTERNAL_EFFECT_STARTED_WATERMARK: &str = "wsnav-opencode-external-effect-started";
const CODEX_EXEC_FAILED_KNOWN_ABSENT_WATERMARK: &str = "wsnav-codex-exec-failed-known-absent";

impl OnboardingAuthority {
    fn revalidate(self, mode: StateMode, root: &Path) -> Result<(), StateError> {
        match self {
            Self::Current => {
                let _ = root;
                ensure_current_mode(mode)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ExistingOnboardingLocation {
    location_id: LocationId,
}

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
    clippy::too_many_lines,
    reason = "the replay loader keeps the full bounded onboarding identity comparison auditable"
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

/// Revalidates every retained request/Runtime claim for a capability that
/// has already transferred to Runtime ownership. The caller must have rebuilt
/// `request` from the live marker, shell, grammar, and Git-worktree evidence
/// while retaining the provisional lease; this transaction repeats the
/// registry-generation, intent, claim-digest, and graph checks before a later
/// provider fence can advance.
#[allow(
    clippy::too_many_lines,
    reason = "the ownership validator keeps every durable handoff identity check auditable"
)]
fn validate_owned_onboarding_transaction(
    transaction: &rusqlite::Transaction<'_>,
    request: &OnboardingPrepareRequest,
    lease_generation: i64,
    registry_generation: &str,
    ownership: OnboardingOwnership,
) -> Result<(OnboardingPhase, Revision), StateError> {
    let persisted: Option<(String, String, String, Option<String>, i64)> = transaction
        .query_row(
            "SELECT kind, phase, expected_revisions_json, launch_claims_digest, revision
             FROM compound_operations WHERE operation_id = ?1",
            [ownership.operation_id.to_string()],
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
    let Some((kind, phase, encoded_intent, claims_digest, revision)) = persisted else {
        return Err(StateError::UnknownOperation(ownership.operation_id));
    };
    if kind != "onboard" {
        return Err(StateError::OnboardingOperationUnavailable);
    }
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
        || intent.location_id != ownership.location_id
        || intent.workstream_id != ownership.workstream_id
        || intent.candidate_runtime_id != ownership.runtime_id
    {
        return Err(StateError::OperationRequestMismatch);
    }
    let claims = onboarding_claims(
        ownership.operation_id,
        intent.location_id,
        &intent.runtime_generation,
        registry_generation,
        lease_generation,
        request,
    )?;
    if claims_digest.as_deref() != Some(claims.digest().as_str()) {
        return Err(StateError::OperationRequestMismatch);
    }
    let runtime: Option<(String, String, String, String, String, String)> = transaction
        .query_row(
            "SELECT runtimes.workstream_id, workstreams.location_id, runtimes.provider,
                    runtimes.tmux_generation, runtimes.tmux_session, runtimes.cwd
             FROM runtimes
             JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
             WHERE runtimes.runtime_id = ?1",
            [ownership.runtime_id.to_string()],
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
    if workstream_id != ownership.workstream_id.to_string()
        || location_id != ownership.location_id.to_string()
        || provider != request.provider.as_str()
        || runtime_generation != intent.runtime_generation
        || session != request.runtime_paths.session_name
        || cwd != repository_path
    {
        return Err(StateError::MalformedHostSchema);
    }
    let persisted_revision = Revision::try_from(revision)?;
    if persisted_revision != ownership.operation_revision {
        return Err(StateError::ConcurrentWrite);
    }
    let phase = OnboardingPhase::from_operation_phase(
        operation_phase_from_text(&phase).map_err(|_| StateError::MalformedHostSchema)?,
    )
    .ok_or(StateError::MalformedHostSchema)?;
    Ok((phase, persisted_revision))
}

/// Proves the narrow Codex-only condition under which an exact final `execve`
/// error can be considered known-absent. Any recorded provider identity,
/// binding, prior effect watermark, or non-starting Runtime is ambiguous and
/// must remain fenced for recovery instead.
fn validate_codex_known_absence(
    transaction: &rusqlite::Transaction<'_>,
    ownership: OnboardingOwnership,
) -> Result<(), StateError> {
    let runtime: (String, Option<i64>, Option<String>, String) = transaction
        .query_row(
            "SELECT provider, provider_pid, process_birth, lifecycle
             FROM runtimes WHERE runtime_id = ?1 AND workstream_id = ?2",
            params![
                ownership.runtime_id.to_string(),
                ownership.workstream_id.to_string(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(StateError::Sqlite)?;
    let bindings: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM provider_bindings WHERE runtime_id = ?1",
            [ownership.runtime_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    let operation_effects: (Option<String>, Option<String>) = transaction
        .query_row(
            "SELECT effect_watermark, outcome_json
             FROM compound_operations WHERE operation_id = ?1",
            [ownership.operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(StateError::Sqlite)?;
    if runtime.0 != ProviderKind::Codex.as_str()
        || runtime.1.is_some()
        || runtime.2.is_some()
        || runtime.3 != "starting"
        || bindings != 0
        || operation_effects.0.is_some()
        || operation_effects.1.is_some()
    {
        return Err(StateError::OnboardingOperationUnavailable);
    }
    Ok(())
}

/// Records only `OpenCode`'s pre-effect boundary. The subsequent native exec or
/// any failed/unknown POST must retain this watermark, so no later path can
/// misclassify that attempt as a clean Codex-style known absence.
fn validate_opencode_external_effect(
    transaction: &rusqlite::Transaction<'_>,
    ownership: OnboardingOwnership,
) -> Result<(), StateError> {
    let provider: String = transaction
        .query_row(
            "SELECT provider FROM runtimes WHERE runtime_id = ?1 AND workstream_id = ?2",
            params![
                ownership.runtime_id.to_string(),
                ownership.workstream_id.to_string(),
            ],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if provider != ProviderKind::OpenCode.as_str() {
        return Err(StateError::OnboardingOperationUnavailable);
    }
    Ok(())
}

/// Requires the provider-specific pre-exec history that the journal retains
/// after the transient helper exits. `Codex` has no provider pre-effect path;
/// `OpenCode` must retain its exact potential-effect watermark before native
/// exec can begin.
fn validate_provider_exec_start(
    transaction: &rusqlite::Transaction<'_>,
    ownership: OnboardingOwnership,
    provider: ProviderKind,
) -> Result<(), StateError> {
    let persisted: (String, Option<String>, Option<String>) = transaction
        .query_row(
            "SELECT runtimes.provider, compound_operations.effect_watermark,
                    compound_operations.outcome_json
             FROM runtimes
             JOIN compound_operations ON compound_operations.operation_id = ?1
             WHERE runtimes.runtime_id = ?2 AND runtimes.workstream_id = ?3",
            params![
                ownership.operation_id.to_string(),
                ownership.runtime_id.to_string(),
                ownership.workstream_id.to_string(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(StateError::Sqlite)?;
    let valid = match provider {
        ProviderKind::Codex => {
            persisted.0 == ProviderKind::Codex.as_str()
                && persisted.1.is_none()
                && persisted.2.is_none()
        }
        ProviderKind::OpenCode => {
            persisted.0 == ProviderKind::OpenCode.as_str()
                && persisted.1.as_deref() == Some(OPENCODE_EXTERNAL_EFFECT_STARTED_WATERMARK)
                && persisted.2.is_none()
        }
    };
    if !valid {
        return Err(StateError::OnboardingOperationUnavailable);
    }
    let targets_table = "onboarding_exec_targets";
    let target_provider: Option<String> = transaction
        .query_row(
            &format!("SELECT provider FROM {targets_table} WHERE operation_id = ?1"),
            [ownership.operation_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    if target_provider.as_deref() != Some(provider.as_str()) {
        return Err(StateError::OnboardingOperationUnavailable);
    }
    Ok(())
}

type ExecProofRuntimeRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    String,
);

/// Loads one exact provider-exec operation and its retained graph
/// identity. This is deliberately narrower than a snapshot: it returns no
/// command, path, provider payload, or marker data, and it refuses every phase
/// except the requested final pre-exec or already-proven reconciliation fence.
#[allow(
    clippy::too_many_lines,
    reason = "the exec-proof loader keeps every bounded process and ownership check auditable"
)]
fn load_exec_proof_target(
    transaction: &rusqlite::Transaction<'_>,
    state_root: &Path,
    operation_id: OperationId,
    required_phase: OnboardingPhase,
) -> Result<OnboardingProviderExecTarget, StateError> {
    let persisted: Option<(String, String, String, i64)> = transaction
        .query_row(
            "SELECT kind, phase, expected_revisions_json, revision
             FROM compound_operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((kind, phase, encoded_intent, revision)) = persisted else {
        return Err(StateError::UnknownOperation(operation_id));
    };
    if kind != "onboard" || phase != operation_phase_text(required_phase.operation_phase()) {
        return Err(StateError::OnboardingOperationUnavailable);
    }
    let intent: PersistedOnboardingIntent =
        serde_json::from_str(&encoded_intent).map_err(|_| StateError::MalformedHostSchema)?;
    if intent.version != 1 {
        return Err(StateError::MalformedHostSchema);
    }
    let targets_table = "onboarding_exec_targets";
    let executable_identity: Option<(String, i64, i64)> = transaction
        .query_row(
            &format!(
                "SELECT provider, executable_device, executable_inode
             FROM {targets_table} WHERE operation_id = ?1"
            ),
            [operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((target_provider, device, inode)) = executable_identity else {
        return Err(StateError::OnboardingOperationUnavailable);
    };
    let target_provider = target_provider
        .parse::<ProviderKind>()
        .map_err(|_| StateError::MalformedHostSchema)?;
    let executable_identity = OnboardingProviderExecutableIdentity::new(
        u64::try_from(device).map_err(|_| StateError::MalformedHostSchema)?,
        u64::try_from(inode).map_err(|_| StateError::MalformedHostSchema)?,
    )
    .map_err(|_| StateError::MalformedHostSchema)?;
    let runtime: Option<ExecProofRuntimeRow> = transaction
        .query_row(
            "SELECT runtimes.workstream_id, workstreams.location_id, runtimes.provider,
                    runtimes.tmux_generation, runtimes.tmux_session, runtimes.cwd,
                    runtimes.provider_pid, runtimes.process_birth, runtimes.lifecycle
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
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((
        workstream_id,
        location_id,
        provider,
        runtime_generation,
        session,
        cwd,
        provider_pid,
        provider_birth,
        lifecycle,
    )) = runtime
    else {
        return Err(StateError::MalformedHostSchema);
    };
    let provider = provider
        .parse::<ProviderKind>()
        .map_err(|_| StateError::MalformedHostSchema)?;
    let project_root = PathBuf::from(cwd);
    let state_root =
        fs::canonicalize(state_root).map_err(|_| StateError::InvalidOnboardingPreparation)?;
    let expected_runtime_paths =
        RuntimePaths::for_runtime(&state_root, intent.candidate_runtime_id);
    let process_identity_matches = match required_phase {
        OnboardingPhase::ProviderExecStarted => match (provider_pid, provider_birth.as_deref()) {
            (None, None) => lifecycle == "starting",
            (Some(pid), Some(birth)) => {
                provider == ProviderKind::OpenCode
                    && u32::try_from(pid).is_ok_and(|pid| pid > 0)
                    && validate_registry_text("provider birth", birth).is_ok()
                    && lifecycle == "starting"
            }
            _ => false,
        },
        OnboardingPhase::ProviderExecProven => {
            runtime_status_from_text(&lifecycle).is_ok()
                && provider_pid
                    .and_then(|pid| u32::try_from(pid).ok())
                    .is_some_and(|pid| pid > 0)
                && provider_birth
                    .as_deref()
                    .is_some_and(|birth| validate_registry_text("provider birth", birth).is_ok())
        }
        _ => return Err(StateError::MalformedHostSchema),
    };
    if workstream_id != intent.workstream_id.to_string()
        || location_id != intent.location_id.to_string()
        || provider != intent.provider
        || target_provider != intent.provider
        || runtime_generation != intent.runtime_generation
        || session != expected_runtime_paths.session_name
        || !is_normalized_absolute_utf8_path(&project_root)
        || !process_identity_matches
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(OnboardingProviderExecTarget {
        ownership: OnboardingOwnership {
            operation_id,
            location_id: intent.location_id,
            workstream_id: intent.workstream_id,
            runtime_id: intent.candidate_runtime_id,
            operation_revision: Revision::try_from(revision)?,
        },
        provider,
        project_root,
        runtime_generation,
        executable_identity,
    })
}

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

impl CurrentState {
    pub(crate) fn prepare_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        id_generator: &dyn IdGenerator,
    ) -> Result<OnboardingPreparation, StateError> {
        self.prepare_onboarding_authorized(
            OnboardingAuthority::Current,
            provisional_lease,
            request,
            id_generator,
        )
    }

    fn prepare_onboarding_authorized(
        &mut self,
        authority: OnboardingAuthority,
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
        let preparation = self.prepare_onboarding_with_zero_timeout(
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
    fn prepare_onboarding_with_zero_timeout(
        &mut self,
        authority: OnboardingAuthority,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        id_generator: &dyn IdGenerator,
    ) -> Result<OnboardingPreparation, StateError> {
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
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
        let location_created = existing_location.is_none();
        let project_created = if location_created {
            match request.repository.remote_identity_fingerprint.as_deref() {
                Some(fingerprint) => {
                    find_project_by_fingerprint(&transaction, fingerprint)?.is_none()
                }
                None => true,
            }
        } else {
            false
        };
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
            location_created,
            project_created,
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
        validate_schema15(&transaction)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(OnboardingPreparation::Issued(OnboardingReservation {
            operation_id,
            #[cfg(test)]
            workstream_id,
            capability,
        }))
    }

    /// Reads the one durable onboarding journal that may belong to an exact
    /// provisional marker.  The comparison is deliberately marker-scoped:
    /// no shell, path, capability, or provider payload is returned.  A
    /// mismatch is a closed refusal rather than an adoption opportunity.
    pub(crate) fn onboarding_marker_operation_current(
        &self,
        provisional_lease: &ProvisionalLease,
        presentation_id: Uuid,
        presentation_revision: Revision,
        slot_generation: Uuid,
        candidate_runtime_id: RuntimeId,
        handoff_request: Option<OperationId>,
    ) -> Result<Option<OnboardingMarkerOperation>, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let rows = if let Some(handoff_request) = handoff_request {
            self.connection
                .query_row(
                    "SELECT operation_id, phase, expected_revisions_json
                     FROM compound_operations
                     WHERE kind = 'onboard' AND operation_id = ?1",
                    [handoff_request.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(StateError::Sqlite)?
                .into_iter()
                .collect()
        } else {
            let _read_snapshot = self
                .connection
                .unchecked_transaction()
                .map_err(StateError::Sqlite)?;
            let mut cursor: u32 = 0;
            let mut rows = Vec::new();
            loop {
                let (page, next_cursor) =
                    self.onboarding_marker_operation_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
                rows.extend(page);
                let Some(next_cursor) = next_cursor else {
                    break rows;
                };
                cursor = next_cursor;
            }
        };
        let mut match_result = None;
        for (operation_id, phase, encoded_intent) in rows {
            let operation_id = operation_id
                .parse::<OperationId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            if handoff_request.is_some_and(|expected| expected != operation_id) {
                continue;
            }
            let intent: PersistedOnboardingIntent = serde_json::from_str(&encoded_intent)
                .map_err(|_| StateError::MalformedHostSchema)?;
            if intent.candidate_runtime_id == candidate_runtime_id
                && (intent.presentation_id != presentation_id
                    || intent.presentation_revision != presentation_revision
                    || intent.slot_generation != slot_generation)
            {
                return Err(StateError::OperationRequestMismatch);
            }
            let identity_matches = intent.presentation_id == presentation_id
                && intent.presentation_revision == presentation_revision
                && intent.slot_generation == slot_generation
                && intent.candidate_runtime_id == candidate_runtime_id;
            if !identity_matches {
                if handoff_request == Some(operation_id) {
                    return Err(StateError::OperationRequestMismatch);
                }
                continue;
            }
            let operation_phase =
                operation_phase_from_text(&phase).map_err(|_| StateError::MalformedHostSchema)?;
            let onboarding_phase = OnboardingPhase::from_operation_phase(operation_phase)
                .ok_or(StateError::OnboardingOperationUnavailable)?;
            if match_result
                .replace(OnboardingMarkerOperation {
                    operation_id,
                    phase: onboarding_phase,
                })
                .is_some()
            {
                return Err(StateError::MalformedHostSchema);
            }
        }
        provisional_lease.revalidate_for_mutation(&self.root)?;
        if handoff_request.is_some() && match_result.is_none() {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        Ok(match_result)
    }

    /// Cancels one exact, unconsumed onboarding attempt.  The transaction
    /// proves the marker identity against the journal, verifies that no
    /// Runtime/provider effect crossed the ownership fence, invalidates the
    /// persisted capability, removes only attempt-owned graph rows, and
    /// leaves a terminal bounded rollback record so replay cannot succeed.
    ///
    /// A `false` result means that no journal belongs to this marker (the
    /// marker may be a materialization-only crash).  Any identity, lifecycle,
    /// or effect ambiguity returns an error without mutation.  Capability
    /// expiry, boot provenance, and unrelated registry writes do not weaken
    /// this exact pre-effect cleanup authority: cancellation is the journal's
    /// serialized winner under the provisional lease.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::type_complexity,
        reason = "the exact marker identity and lease fields are the cancellation authority"
    )]
    pub(crate) fn cancel_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        presentation_id: Uuid,
        presentation_revision: Revision,
        slot_generation: Uuid,
        candidate_runtime_id: RuntimeId,
        runtime_paths: &RuntimePaths,
        handoff_request: Option<OperationId>,
    ) -> Result<bool, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let canonical_root =
            fs::canonicalize(&self.root).map_err(|_| StateError::InvalidOnboardingPreparation)?;
        if runtime_paths != &RuntimePaths::for_runtime(&canonical_root, candidate_runtime_id) {
            return Err(StateError::InvalidOnboardingPreparation);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let rows = if let Some(handoff_request) = handoff_request {
            transaction
                .query_row(
                    "SELECT operation_id, phase, expected_revisions_json,
                            launch_token_expiry_monotonic, effect_watermark, outcome_json,
                            launch_token_id, launch_token_verifier,
                            launch_claims_digest, revision
                     FROM compound_operations
                     WHERE kind = 'onboard' AND operation_id = ?1",
                    [handoff_request.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, i64>(9)?,
                        ))
                    },
                )
                .optional()
                .map_err(StateError::Sqlite)?
                .into_iter()
                .collect()
        } else {
            let mut cursor: u32 = 0;
            let mut rows = Vec::new();
            loop {
                let (page, next_cursor) = {
                    let mut statement = transaction
                        .prepare(
                            "SELECT operation_id, phase, expected_revisions_json,
                                    launch_token_expiry_monotonic, effect_watermark, outcome_json,
                                    launch_token_id, launch_token_verifier,
                                    launch_claims_digest, revision
                             FROM compound_operations
                             WHERE kind = 'onboard'
                             ORDER BY operation_id
                             LIMIT ?1 OFFSET ?2",
                        )
                        .map_err(StateError::Sqlite)?;
                    let (query_limit, cursor_step) = page_parameters(MAX_NAVIGATOR_WORKSTREAMS)?;
                    let page = statement
                        .query_map([query_limit, i64::from(cursor)], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, Option<i64>>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, Option<String>>(5)?,
                                row.get::<_, Option<String>>(6)?,
                                row.get::<_, Option<String>>(7)?,
                                row.get::<_, Option<String>>(8)?,
                                row.get::<_, i64>(9)?,
                            ))
                        })
                        .map_err(StateError::Sqlite)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(StateError::Sqlite)?;
                    let has_more = page.len() > MAX_NAVIGATOR_WORKSTREAMS;
                    let next_cursor = if has_more {
                        Some(
                            cursor
                                .checked_add(cursor_step)
                                .ok_or(StateError::NavigatorCursorOverflow)?,
                        )
                    } else {
                        None
                    };
                    (
                        page.into_iter()
                            .take(MAX_NAVIGATOR_WORKSTREAMS)
                            .collect::<Vec<_>>(),
                        next_cursor,
                    )
                };
                rows.extend(page);
                let Some(next_cursor) = next_cursor else {
                    break rows;
                };
                cursor = next_cursor;
            }
        };

        let mut matched = None;
        for row in rows {
            let operation_id = row
                .0
                .parse::<OperationId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            if handoff_request.is_some_and(|expected| expected != operation_id) {
                continue;
            }
            let intent: PersistedOnboardingIntent =
                serde_json::from_str(&row.2).map_err(|_| StateError::MalformedHostSchema)?;
            if intent.candidate_runtime_id == candidate_runtime_id
                && (intent.presentation_id != presentation_id
                    || intent.presentation_revision != presentation_revision
                    || intent.slot_generation != slot_generation)
            {
                return Err(StateError::OperationRequestMismatch);
            }
            let identity_matches = intent.presentation_id == presentation_id
                && intent.presentation_revision == presentation_revision
                && intent.slot_generation == slot_generation
                && intent.candidate_runtime_id == candidate_runtime_id;
            if !identity_matches {
                if handoff_request == Some(operation_id) {
                    return Err(StateError::OperationRequestMismatch);
                }
                continue;
            }
            if matched.is_some() {
                return Err(StateError::MalformedHostSchema);
            }
            matched = Some((operation_id, row, intent));
        }
        let Some((operation_id, row, intent)) = matched else {
            let runtime_exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtimes WHERE runtime_id = ?1)",
                    [candidate_runtime_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(StateError::Sqlite)?;
            if runtime_exists {
                return Err(StateError::OnboardingOperationUnavailable);
            }
            transaction.commit().map_err(StateError::Sqlite)?;
            provisional_lease.revalidate_for_mutation(&self.root)?;
            return Ok(false);
        };
        if row.1 == "rolled_back" {
            if row.5.as_deref() != Some(ONBOARDING_CANCELLED_OUTCOME)
                || row.4.is_some()
                || row.6.is_some()
                || row.7.is_some()
                || row.8.is_some()
            {
                return Err(StateError::MalformedHostSchema);
            }
            transaction.commit().map_err(StateError::Sqlite)?;
            provisional_lease.revalidate_for_mutation(&self.root)?;
            return Ok(false);
        }
        if row.1 != "capability_issued" {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let _ = row.3.ok_or(StateError::MalformedHostSchema)?;
        if row.4.is_some() || row.5.is_some() {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        if row.6.is_none() || row.7.is_none() || row.8.is_none() {
            return Err(StateError::MalformedHostSchema);
        }
        if intent.version != 1 || intent.lease_generation != provisional_lease.lease_generation() {
            return Err(StateError::OperationRequestMismatch);
        }
        let runtime: Option<(
            String,
            String,
            String,
            String,
            String,
            Option<i64>,
            Option<String>,
            String,
        )> = transaction
            .query_row(
                "SELECT runtimes.workstream_id, workstreams.location_id,
                            runtimes.provider, runtimes.tmux_generation,
                            runtimes.tmux_session, runtimes.provider_pid,
                            runtimes.process_birth, runtimes.cwd
                     FROM runtimes
                     JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                     WHERE runtimes.runtime_id = ?1",
                [candidate_runtime_id.to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((
            workstream_id,
            location_id,
            provider,
            runtime_generation,
            session,
            provider_pid,
            process_birth,
            runtime_cwd,
        )) = runtime
        else {
            return Err(StateError::MalformedHostSchema);
        };
        if workstream_id != intent.workstream_id.to_string()
            || location_id != intent.location_id.to_string()
            || provider != intent.provider.as_str()
            || runtime_generation != intent.runtime_generation
            || session != runtime_paths.session_name
            || provider_pid.is_some()
            || process_birth.is_some()
        {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let lifecycle: String = transaction
            .query_row(
                "SELECT lifecycle FROM runtimes WHERE runtime_id = ?1",
                [candidate_runtime_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if lifecycle != "starting" {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let bindings: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM provider_bindings WHERE runtime_id = ?1",
                [candidate_runtime_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        let handles: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM opencode_runtime_handles WHERE runtime_id = ?1",
                [candidate_runtime_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        let targets_table = "onboarding_exec_targets";
        let targets: i64 = transaction
            .query_row(
                &format!("SELECT COUNT(*) FROM {targets_table} WHERE operation_id = ?1"),
                [operation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if bindings != 0 || handles != 0 || targets != 0 {
            return Err(StateError::OnboardingOperationUnavailable);
        }

        // Capture the exact Location/Project relationship before deleting the
        // attempt graph.  Missing or changed rows are ambiguous and therefore
        // leave the transaction untouched.
        let location: Option<(Option<String>, String)> = transaction
            .query_row(
                "SELECT project_id, repository_path FROM project_locations
                 WHERE location_id = ?1",
                [intent.location_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((project_id, repository_path)) = location else {
            return Err(StateError::MalformedHostSchema);
        };
        if runtime_cwd != repository_path {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        if !intent.location_created {
            // A pre-existing location is never destructive cleanup authority.
            // Its Runtime/workstream are still exact attempt rows and can be
            // removed; the retained Location/Project history stays intact.
        } else if intent.project_created {
            let project_id = project_id
                .as_deref()
                .ok_or(StateError::MalformedHostSchema)?
                .parse::<ProjectId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            let project: Option<(String, i64)> = transaction
                .query_row(
                    "SELECT label_location_id, revision FROM projects WHERE project_id = ?1",
                    [project_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(StateError::Sqlite)?;
            let Some((label_location_id, _revision)) = project else {
                return Err(StateError::MalformedHostSchema);
            };
            let location_count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
                    [project_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(StateError::Sqlite)?;
            if label_location_id != intent.location_id.to_string() || location_count != 1 {
                return Err(StateError::OnboardingOperationUnavailable);
            }
            let detached = transaction
                .execute(
                    "UPDATE project_locations SET project_id = NULL
                     WHERE location_id = ?1 AND project_id = ?2",
                    params![intent.location_id.to_string(), project_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            if detached != 1 {
                return Err(StateError::ConcurrentWrite);
            }
            let deleted = transaction
                .execute(
                    "DELETE FROM projects WHERE project_id = ?1 AND label_location_id = ?2",
                    params![project_id.to_string(), intent.location_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            if deleted != 1 {
                return Err(StateError::ConcurrentWrite);
            }
        } else if let Some(project_id) = project_id.as_deref() {
            let label_location_id: String = transaction
                .query_row(
                    "SELECT label_location_id FROM projects WHERE project_id = ?1",
                    [project_id],
                    |row| row.get(0),
                )
                .map_err(StateError::Sqlite)?;
            if label_location_id == intent.location_id.to_string() {
                return Err(StateError::OnboardingOperationUnavailable);
            }
        }

        let repository_path = PathBuf::from(repository_path);
        if intent.location_created
            && (!is_normalized_absolute_utf8_path(&repository_path)
                || repository_path
                    != repository_path
                        .canonicalize()
                        .map_err(|_| StateError::MalformedHostSchema)?)
        {
            return Err(StateError::MalformedHostSchema);
        }
        let deleted = transaction
            .execute(
                "DELETE FROM runtimes WHERE runtime_id = ?1 AND workstream_id = ?2
                 AND lifecycle = 'starting' AND provider_pid IS NULL
                 AND process_birth IS NULL",
                params![
                    candidate_runtime_id.to_string(),
                    intent.workstream_id.to_string()
                ],
            )
            .map_err(StateError::Sqlite)?;
        if deleted != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        let deleted = transaction
            .execute(
                "DELETE FROM workstreams WHERE workstream_id = ?1
                 AND location_id = ?2 AND lifecycle = 'open'",
                params![
                    intent.workstream_id.to_string(),
                    intent.location_id.to_string()
                ],
            )
            .map_err(StateError::Sqlite)?;
        if deleted != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        if intent.location_created {
            let deleted = transaction
                .execute(
                    "DELETE FROM project_locations WHERE location_id = ?1
                     AND repository_path = ?2",
                    params![
                        intent.location_id.to_string(),
                        repository_path.to_string_lossy()
                    ],
                )
                .map_err(StateError::Sqlite)?;
            if deleted != 1 {
                return Err(StateError::ConcurrentWrite);
            }
        }
        let operation_revision = Revision::try_from(row.9)?;
        let next_operation_revision = next_revision(operation_revision)?;
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = 'rolled_back', outcome_json = ?1, revision = ?2,
                     launch_token_id = NULL, launch_token_verifier = NULL,
                     launch_token_expiry_monotonic = NULL, launch_claims_digest = NULL
                 WHERE operation_id = ?3 AND kind = 'onboard'
                   AND phase = 'capability_issued' AND revision = ?4
                   AND effect_watermark IS NULL AND outcome_json IS NULL",
                params![
                    ONBOARDING_CANCELLED_OUTCOME,
                    next_operation_revision.value(),
                    operation_id.to_string(),
                    operation_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        validate_project_membership_transaction(&transaction)?;
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(true)
    }

    /// Atomically consumes one launch capability from a normal
    /// schema-15 opening. The provisional lease remains the only mutable
    /// shell-slot authority; provider execution remains outside this seam.
    pub(crate) fn consume_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        token: &str,
        now_monotonic_millis: i64,
    ) -> Result<OnboardingOwnership, StateError> {
        self.consume_onboarding_authorized(
            OnboardingAuthority::Current,
            provisional_lease,
            request,
            token,
            now_monotonic_millis,
        )
    }

    fn consume_onboarding_authorized(
        &mut self,
        authority: OnboardingAuthority,
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
        let ownership = self.consume_onboarding_with_zero_timeout(
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
    fn consume_onboarding_with_zero_timeout(
        &mut self,
        authority: OnboardingAuthority,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        token: &str,
        now_monotonic_millis: i64,
    ) -> Result<OnboardingOwnership, StateError> {
        authority.revalidate(self.mode, &self.root)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
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
        validate_schema15(&transaction)?;
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
    /// Runtime ownership has committed. The identity comes from the resolved
    /// canonical native executable and is committed atomically with the
    /// preparation phase; no executable path or command line is stored.
    pub(crate) fn record_provider_preparation_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
        executable_identity: OnboardingProviderExecutableIdentity,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_onboarding_current(
            provisional_lease,
            request,
            ownership,
            OnboardingAdvance::Normal(OnboardingPhase::ProviderPreparation),
            Some(executable_identity),
        )
    }

    /// Records the point at which provider-specific preparation may have an
    /// external effect. The caller must record this before making that effect;
    /// this state seam itself does not contact a provider.
    pub(crate) fn record_provider_external_effect_started_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_onboarding_current(
            provisional_lease,
            request,
            ownership,
            OnboardingAdvance::OpenCodeExternalEffectStarted,
            None,
        )
    }

    /// Persists the exact blank `OpenCode` session returned after the already
    /// recorded non-idempotent POST boundary. The binding is written only
    /// while the journal remains at that boundary, so a different session
    /// can never be adopted and a future native exec has one durable root
    /// session to revalidate.
    pub(crate) fn record_opencode_created_session_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
        session: &ProviderSessionId,
    ) -> Result<OnboardingOwnership, StateError> {
        ensure_current_mode(self.mode)?;
        if session.provider() != ProviderKind::OpenCode {
            return Err(StateError::ProviderIdentityMismatch);
        }
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        validate_onboarding_prepare_request(request, &self.root)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let registry_generation = load_registry_generation(&transaction)?;
        let (phase, _) = validate_owned_onboarding_transaction(
            &transaction,
            request,
            provisional_lease.lease_generation(),
            &registry_generation,
            ownership,
        )?;
        if phase != OnboardingPhase::ProviderExternalEffectStarted {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let encoded_intent: String = transaction
            .query_row(
                "SELECT expected_revisions_json FROM compound_operations
                 WHERE operation_id = ?1 AND kind = 'onboard'",
                [ownership.operation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        let intent: PersistedOnboardingIntent =
            serde_json::from_str(&encoded_intent).map_err(|_| StateError::MalformedHostSchema)?;
        if intent.provider != ProviderKind::OpenCode
            || intent.runtime_generation.is_empty()
            || intent.candidate_runtime_id != ownership.runtime_id
        {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        bind_opencode_session_in_transaction(
            &transaction,
            ownership.runtime_id,
            &intent.runtime_generation,
            session,
            "new",
        )?;
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(ownership)
    }

    /// Binds the exact loopback endpoint/version/session creation record to a
    /// `OpenCode` onboarding attempt before the final native exec.  The
    /// temporary precreation server is already gone when this commits; this
    /// row is only durable identity evidence for the later detached observer,
    /// never authority to contact or replace a provider.
    #[allow(
        clippy::too_many_lines,
        reason = "the transaction keeps handle identity validation and its one insert boundary auditable together"
    )]
    pub(crate) fn record_opencode_runtime_handle_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
        endpoint_port: u16,
        version: &str,
        session: &ProviderSessionId,
    ) -> Result<OpenCodeRuntimeHandle, StateError> {
        if endpoint_port == 0 || session.provider() != ProviderKind::OpenCode {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_provider_metadata(version)?;
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        validate_onboarding_prepare_request(request, &self.root)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let registry_generation = load_registry_generation(&transaction)?;
        let (phase, _) = validate_owned_onboarding_transaction(
            &transaction,
            request,
            provisional_lease.lease_generation(),
            &registry_generation,
            ownership,
        )?;
        if phase != OnboardingPhase::ProviderExternalEffectStarted {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let encoded_intent: String = transaction
            .query_row(
                "SELECT expected_revisions_json FROM compound_operations
                 WHERE operation_id = ?1 AND kind = 'onboard'",
                [ownership.operation_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        let intent: PersistedOnboardingIntent =
            serde_json::from_str(&encoded_intent).map_err(|_| StateError::MalformedHostSchema)?;
        if intent.provider != ProviderKind::OpenCode
            || intent.runtime_generation.is_empty()
            || intent.candidate_runtime_id != ownership.runtime_id
        {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let runtime: (String, String, String, Option<i64>, Option<String>) = transaction
            .query_row(
                "SELECT provider, tmux_generation, lifecycle, provider_pid, process_birth
                 FROM runtimes WHERE runtime_id = ?1 AND workstream_id = ?2",
                params![
                    ownership.runtime_id.to_string(),
                    ownership.workstream_id.to_string(),
                ],
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
        if provider_kind_from_text(&runtime.0)? != ProviderKind::OpenCode
            || runtime.1 != intent.runtime_generation
            || runtime.2 != "starting"
            || runtime.3.is_some()
            || runtime.4.is_some()
        {
            return Err(StateError::HookEvidenceMismatch);
        }
        let binding = load_binding(&transaction, ownership.runtime_id)?
            .ok_or(StateError::HookEvidenceMismatch)?;
        if binding.provider != ProviderKind::OpenCode
            || binding.runtime_generation != intent.runtime_generation
            || binding.native_session_id != *session
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
        if let Some(existing) = load_opencode_handle(&transaction, ownership.runtime_id)? {
            if existing.runtime_generation == intent.runtime_generation
                && existing.endpoint_host == crate::provider::opencode::LOOPBACK_HOST
                && existing.endpoint_port == endpoint_port
                && existing.version == version
                && existing.native_session_id == *session
                && existing.observer_status == OpenCodeObserverStatus::Starting
                && existing.observer_pid.is_none()
                && existing.observer_birth.is_none()
            {
                transaction.commit().map_err(StateError::Sqlite)?;
                provisional_lease.revalidate_for_mutation(&self.root)?;
                return Ok(existing);
            }
            return Err(StateError::ConcurrentWrite);
        }
        transaction
            .execute(
                "INSERT INTO opencode_runtime_handles (
                    runtime_id, runtime_generation, endpoint_host, endpoint_port,
                    version, native_session_id, observer_pid, observer_birth,
                    observer_status, revision
                 ) VALUES (?1, ?2, '127.0.0.1', ?3, ?4, ?5, NULL, NULL, 'starting', 1)",
                params![
                    ownership.runtime_id.to_string(),
                    intent.runtime_generation,
                    i64::from(endpoint_port),
                    version,
                    session.native_id(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        let handle = load_opencode_handle(&transaction, ownership.runtime_id)?
            .ok_or(StateError::ConcurrentWrite)?;
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(handle)
    }

    /// Fences one Runtime-owned onboarding attempt for explicit recovery.
    /// Callers use this after an ambiguous provider effect or a failed final
    /// exec that cannot be classified as Codex's known absence. It never
    /// rolls back graph rows, reissues a capability, or contacts a provider.
    pub(crate) fn record_recovery_required_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_onboarding_current(
            provisional_lease,
            request,
            ownership,
            OnboardingAdvance::Normal(OnboardingPhase::RecoveryRequired),
            None,
        )
    }

    /// Resolves one terminal onboarding recovery only after compound Archive
    /// has exact-stopped the adopted Runtime and recorded the Workstream's
    /// stopped/parked lifecycle. This commits the onboarding journal without
    /// asserting that the original provider exec was proven, without deleting
    /// its Runtime/binding, and without contacting or retrying a provider.
    ///
    /// The stable provisional lease serializes this terminal classification
    /// with every remaining onboarding participant. A caller supplies the
    /// revision returned by Archive's completed exact-stop cleanup, so a later
    /// Runtime or Workstream transition cannot be mistaken for that deliberate
    /// recovery.
    pub(crate) fn resolve_parked_recovery_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    ) -> Result<(), StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let operation = self
            .onboarding_operation_inventory()?
            .into_iter()
            .find(|operation| {
                operation.workstream_id == workstream_id
                    && operation.phase == OnboardingPhase::RecoveryRequired
            })
            .ok_or(StateError::OnboardingOperationUnavailable)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let operation_revision = transaction
            .query_row(
                "SELECT revision FROM compound_operations
                 WHERE operation_id = ?1 AND kind = 'onboard' AND phase = 'recovery_required'",
                [operation.operation_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .map(Revision::try_from)
            .transpose()?
            .ok_or(StateError::OnboardingOperationUnavailable)?;
        let runtime: Option<(String, String, String, i64)> = transaction
            .query_row(
                "SELECT runtimes.runtime_id, runtimes.lifecycle,
                        workstreams.lifecycle, workstreams.revision
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1 AND runtimes.workstream_id = ?2",
                params![operation.runtime_id.to_string(), workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((runtime_id, runtime_lifecycle, workstream_lifecycle, revision)) = runtime else {
            return Err(StateError::OnboardingOperationUnavailable);
        };
        if runtime_id != operation.runtime_id.to_string()
            || runtime_lifecycle != "stopped"
            || workstream_lifecycle != "parked"
            || Revision::try_from(revision)? != expected_workstream_revision
        {
            return Err(StateError::ConcurrentWrite);
        }
        let next_operation_revision = next_revision(operation_revision)?;
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = 'committed', outcome_json = ?1, revision = ?2
                 WHERE operation_id = ?3 AND kind = 'onboard'
                   AND phase = 'recovery_required' AND revision = ?4
                   AND outcome_json IS NULL",
                params![
                    PARKED_RECOVERY_RESOLVED_OUTCOME,
                    next_operation_revision.value(),
                    operation.operation_id.to_string(),
                    operation_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)
    }

    /// Records the final durable boundary immediately before the helper would
    /// execute the native provider. It intentionally does not expose an
    /// unproven Runtime to ordinary attachment or action authority.
    pub(crate) fn record_provider_exec_started_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_onboarding_current(
            provisional_lease,
            request,
            ownership,
            OnboardingAdvance::Normal(OnboardingPhase::ProviderExecStarted),
            None,
        )
    }

    /// Records a Codex final-`execve` failure only after exact durable proof
    /// that neither a provider process nor a binding can exist. This is a
    /// terminal onboarding fact, not ordinary attachment/action authority;
    /// later recovery decides guarded rollback.
    pub(crate) fn record_codex_exec_failed_known_absent_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
    ) -> Result<OnboardingOwnership, StateError> {
        self.advance_onboarding_current(
            provisional_lease,
            request,
            ownership,
            OnboardingAdvance::CodexExecFailedKnownAbsent,
            None,
        )
    }

    /// Advances one exact Runtime-owned journal through a pre-exec fence.
    /// Provider-exec proof, known absence, rollback, and recovery each need
    /// their own evidence-bearing reconciler APIs and cannot use this generic
    /// mutation seam.
    fn advance_onboarding_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
        advance: OnboardingAdvance,
        executable_identity: Option<OnboardingProviderExecutableIdentity>,
    ) -> Result<OnboardingOwnership, StateError> {
        let previous_busy_timeout = self
            .connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .map_err(StateError::Sqlite)?;
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(StateError::Sqlite)?;
        let advanced = self.advance_onboarding_with_zero_timeout(
            provisional_lease,
            request,
            ownership,
            advance,
            executable_identity,
        );
        let restore = self.connection.busy_timeout(Duration::from_millis(
            u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
        ));
        match (advanced, restore) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(StateError::Sqlite(error)),
            (Ok(ownership), Ok(())) => Ok(ownership),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the lease-held journal update and its identity insert must remain one atomic audit boundary"
    )]
    fn advance_onboarding_with_zero_timeout(
        &mut self,
        provisional_lease: &ProvisionalLease,
        request: &OnboardingPrepareRequest,
        ownership: OnboardingOwnership,
        advance: OnboardingAdvance,
        executable_identity: Option<OnboardingProviderExecutableIdentity>,
    ) -> Result<OnboardingOwnership, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        validate_onboarding_prepare_request(request, &self.root)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let registry_generation = load_registry_generation(&transaction)?;
        let (current, persisted_revision) = validate_owned_onboarding_transaction(
            &transaction,
            request,
            provisional_lease.lease_generation(),
            &registry_generation,
            ownership,
        )?;
        let next = advance.next();
        current.transition(next)?;
        if next == OnboardingPhase::ProviderPreparation && executable_identity.is_none() {
            return Err(StateError::InvalidOnboardingPreparation);
        }
        if next != OnboardingPhase::ProviderPreparation && executable_identity.is_some() {
            return Err(StateError::InvalidOnboardingPreparation);
        }
        if advance.requires_codex_known_absence() {
            validate_codex_known_absence(&transaction, ownership)?;
        }
        if advance.requires_opencode_external_effect() {
            validate_opencode_external_effect(&transaction, ownership)?;
        }
        if next == OnboardingPhase::ProviderExecStarted {
            validate_provider_exec_start(&transaction, ownership, request.provider)?;
        }
        let next_revision = next_revision(persisted_revision)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let updated = if let Some(watermark) = advance.effect_watermark() {
            transaction
                .execute(
                    "UPDATE compound_operations
                     SET phase = ?1, effect_watermark = ?2, revision = ?3
                     WHERE operation_id = ?4 AND kind = 'onboard'
                       AND phase = ?5 AND revision = ?6
                       AND effect_watermark IS NULL AND outcome_json IS NULL",
                    params![
                        operation_phase_text(next.operation_phase()),
                        watermark,
                        next_revision.value(),
                        ownership.operation_id.to_string(),
                        operation_phase_text(current.operation_phase()),
                        persisted_revision.value(),
                    ],
                )
                .map_err(StateError::Sqlite)?
        } else {
            transaction
                .execute(
                    "UPDATE compound_operations
                     SET phase = ?1, revision = ?2
                     WHERE operation_id = ?3 AND kind = 'onboard'
                       AND phase = ?4 AND revision = ?5",
                    params![
                        operation_phase_text(next.operation_phase()),
                        next_revision.value(),
                        ownership.operation_id.to_string(),
                        operation_phase_text(current.operation_phase()),
                        persisted_revision.value(),
                    ],
                )
                .map_err(StateError::Sqlite)?
        };
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        if let Some(executable_identity) = executable_identity {
            let targets_table = "onboarding_exec_targets";
            let inserted = transaction
                .execute(
                    &format!(
                        "INSERT INTO {targets_table} (
                        operation_id, provider, executable_device, executable_inode
                     ) VALUES (?1, ?2, ?3, ?4)"
                    ),
                    params![
                        ownership.operation_id.to_string(),
                        request.provider.as_str(),
                        i64::try_from(executable_identity.device())
                            .map_err(|_| StateError::InvalidOnboardingPreparation)?,
                        i64::try_from(executable_identity.inode())
                            .map_err(|_| StateError::InvalidOnboardingPreparation)?,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            if inserted != 1 {
                return Err(StateError::ConcurrentWrite);
            }
        }
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(OnboardingOwnership {
            operation_revision: next_revision,
            ..ownership
        })
    }

    /// Loads the full private target needed for local post-exec proof.
    /// This remains unavailable to snapshots and ordinary current actions.
    pub(crate) fn onboarding_exec_proof_target_current(
        &self,
        provisional_lease: &ProvisionalLease,
        operation_id: OperationId,
    ) -> Result<OnboardingProviderExecTarget, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let target = load_exec_proof_target(
            &transaction,
            &self.root,
            operation_id,
            OnboardingPhase::ProviderExecStarted,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(target)
    }

    /// Loads the exact ready `OpenCode` observer handle for a still-fenced
    /// provider-exec operation. This is deliberately a -only read: the
    /// caller must still corroborate the returned PID/birth pair against the
    /// live process table before it can activate ordinary attachment.
    pub(crate) fn opencode_observer_ready_current(
        &self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
    ) -> Result<OpenCodeRuntimeHandle, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let target = load_exec_proof_target(
            &transaction,
            &self.root,
            ownership.operation_id,
            OnboardingPhase::ProviderExecStarted,
        )?;
        if target.ownership() != ownership || target.provider() != ProviderKind::OpenCode {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        let handle = load_opencode_handle(&transaction, ownership.runtime_id)?
            .ok_or(StateError::OnboardingOperationUnavailable)?;
        if handle.runtime_generation != target.runtime_generation()
            || handle.observer_status != OpenCodeObserverStatus::Ready
            || handle.observer_pid.is_none()
            || handle.observer_birth.is_none()
        {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(handle)
    }

    /// Loads an already proven provider-exec target so presentation-private
    /// marker reconciliation can complete after a state-before-marker crash.
    /// It performs no provider I/O and exposes no public snapshot data.
    pub(crate) fn onboarding_exec_proven_target_current(
        &self,
        provisional_lease: &ProvisionalLease,
        operation_id: OperationId,
    ) -> Result<OnboardingProviderExecTarget, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let target = load_exec_proof_target(
            &transaction,
            &self.root,
            operation_id,
            OnboardingPhase::ProviderExecProven,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(target)
    }

    /// Atomically records the exact process identity of a provider whose
    /// native exec was independently proven. This requires the one current
    /// `provider_exec_started` revision, preserves the unbound `starting`
    /// Runtime lifecycle, and never starts, attaches, signals, or contacts a
    /// provider.
    pub(crate) fn record_provider_exec_proven_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
        evidence: &OnboardingProviderExecEvidence,
    ) -> Result<OnboardingOwnership, StateError> {
        let previous_busy_timeout = self
            .connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .map_err(StateError::Sqlite)?;
        self.connection
            .busy_timeout(Duration::ZERO)
            .map_err(StateError::Sqlite)?;
        let proven = self.record_provider_exec_proven_with_zero_timeout(
            provisional_lease,
            ownership,
            evidence,
        );
        let restore = self.connection.busy_timeout(Duration::from_millis(
            u64::try_from(previous_busy_timeout.max(0)).unwrap_or(0),
        ));
        match (proven, restore) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(StateError::Sqlite(error)),
            (Ok(ownership), Ok(())) => Ok(ownership),
        }
    }

    /// Records exact native process identity while retaining the journal
    /// at `provider_exec_started`.  This is the only state a post-exec
    /// `OpenCode` observer may adopt; ordinary attachment remains fenced until
    /// the controller has independently established the exact observer.
    #[allow(
        clippy::too_many_lines,
        reason = "the transaction keeps process evidence and its no-activation invariant auditable together"
    )]
    pub(crate) fn record_provider_exec_observed_current(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
        evidence: &OnboardingProviderExecEvidence,
    ) -> Result<OnboardingOwnership, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let current = load_exec_proof_target(
            &transaction,
            &self.root,
            ownership.operation_id,
            OnboardingPhase::ProviderExecStarted,
        )?;
        if current.ownership != ownership {
            return Err(StateError::ConcurrentWrite);
        }
        let runtime: (Option<i64>, Option<String>, String, i64) = transaction
            .query_row(
                "SELECT provider_pid, process_birth, lifecycle, revision
                 FROM runtimes WHERE runtime_id = ?1 AND workstream_id = ?2",
                params![
                    ownership.runtime_id.to_string(),
                    ownership.workstream_id.to_string(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(StateError::Sqlite)?;
        if runtime.2 != "starting" {
            return Err(StateError::MalformedHostSchema);
        }
        match (runtime.0, runtime.1) {
            (Some(pid), Some(birth))
                if pid == i64::from(evidence.provider_pid) && birth == evidence.provider_birth => {}
            (None, None) => {
                let runtime_revision = Revision::try_from(runtime.3)?;
                let next_runtime_revision = next_revision(runtime_revision)?;
                let changed = transaction
                    .execute(
                        "UPDATE runtimes
                         SET provider_pid = ?1, process_birth = ?2, revision = ?3
                         WHERE runtime_id = ?4 AND workstream_id = ?5
                           AND provider_pid IS NULL AND process_birth IS NULL
                           AND lifecycle = 'starting' AND revision = ?6",
                        params![
                            i64::from(evidence.provider_pid),
                            evidence.provider_birth,
                            next_runtime_revision.value(),
                            ownership.runtime_id.to_string(),
                            ownership.workstream_id.to_string(),
                            runtime_revision.value(),
                        ],
                    )
                    .map_err(StateError::Sqlite)?;
                if changed != 1 {
                    return Err(StateError::ConcurrentWrite);
                }
            }
            _ => return Err(StateError::ConcurrentWrite),
        }
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(ownership)
    }

    fn record_provider_exec_proven_with_zero_timeout(
        &mut self,
        provisional_lease: &ProvisionalLease,
        ownership: OnboardingOwnership,
        evidence: &OnboardingProviderExecEvidence,
    ) -> Result<OnboardingOwnership, StateError> {
        ensure_current_mode(self.mode)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        validate_schema15(&self.connection)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        let current = load_exec_proof_target(
            &transaction,
            &self.root,
            ownership.operation_id,
            OnboardingPhase::ProviderExecStarted,
        )?;
        if current.ownership != ownership {
            return Err(StateError::ConcurrentWrite);
        }
        if current.provider() == ProviderKind::OpenCode {
            let handle = load_opencode_handle(&transaction, ownership.runtime_id)?
                .ok_or(StateError::OnboardingOperationUnavailable)?;
            if handle.runtime_generation != current.runtime_generation()
                || handle.observer_status != OpenCodeObserverStatus::Ready
                || handle.observer_pid.is_none()
                || handle.observer_birth.is_none()
            {
                return Err(StateError::OnboardingOperationUnavailable);
            }
        }
        let next_operation_revision = next_revision(ownership.operation_revision)?;
        let runtime: (Option<i64>, Option<String>, String, i64) = transaction
            .query_row(
                "SELECT provider_pid, process_birth, lifecycle, revision
                 FROM runtimes WHERE runtime_id = ?1 AND workstream_id = ?2",
                params![
                    ownership.runtime_id.to_string(),
                    ownership.workstream_id.to_string(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(StateError::Sqlite)?;
        let runtime_revision = Revision::try_from(runtime.3)?;
        if runtime.2 != "starting" {
            return Err(StateError::MalformedHostSchema);
        }
        match (runtime.0, runtime.1) {
            (Some(pid), Some(birth))
                if pid == i64::from(evidence.provider_pid) && birth == evidence.provider_birth => {}
            (None, None) => {
                let next_runtime_revision = next_revision(runtime_revision)?;
                let runtime_updated = transaction
                    .execute(
                        "UPDATE runtimes
                         SET provider_pid = ?1, process_birth = ?2, revision = ?3
                         WHERE runtime_id = ?4 AND workstream_id = ?5
                           AND provider_pid IS NULL AND process_birth IS NULL
                           AND lifecycle = 'starting' AND revision = ?6",
                        params![
                            i64::from(evidence.provider_pid),
                            evidence.provider_birth,
                            next_runtime_revision.value(),
                            ownership.runtime_id.to_string(),
                            ownership.workstream_id.to_string(),
                            runtime_revision.value(),
                        ],
                    )
                    .map_err(StateError::Sqlite)?;
                if runtime_updated != 1 {
                    return Err(StateError::ConcurrentWrite);
                }
            }
            _ => return Err(StateError::ConcurrentWrite),
        }
        let operation_updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = 'provider_exec_proven', revision = ?1
                 WHERE operation_id = ?2 AND kind = 'onboard'
                   AND phase = 'provider_exec_started' AND revision = ?3",
                params![
                    next_operation_revision.value(),
                    ownership.operation_id.to_string(),
                    ownership.operation_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if operation_updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        validate_schema15(&transaction)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        provisional_lease.revalidate_for_mutation(&self.root)?;
        Ok(OnboardingOwnership {
            operation_revision: next_operation_revision,
            ..ownership
        })
    }
}
