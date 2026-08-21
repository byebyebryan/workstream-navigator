//! D16 cutover orchestration.
//!
//! This module owns the ordering and authority boundary needed by the D16
//! activation slice while keeping external effects behind small injected traits. In
//! particular, discovery never opens host state, drain-only presentations
//! never acquire a lease, and the only process identities accepted by a
//! handover are the exact identities corroborated by the injected authority.

#![allow(
    clippy::missing_errors_doc,
    reason = "The orchestration surface shares one typed failure boundary documented below."
)]

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    domain::{IdGenerator, ProviderKind, Revision, RuntimeId, RuntimeStatus},
    presentation::{
        LegacyPresentationAssessment, LegacyPresentationProof, LegacyPresentationState,
        PresentationError, classify_legacy_presentations, retire_legacy_presentation,
    },
    provider::opencode::{
        LOOPBACK_HOST, OpenCodeClient, OpenCodeEndpoint, OpenCodeSessionStatus,
        endpoint_owned_by_process,
    },
    runtime::{LinuxProcessProbe, ProcessProbe},
    state::{
        CurrentObserverHandleProof, D16State, HandoverPhase, HandoverRestartAction,
        LEGACY_CLIENT_DATABASE_FILE, LEGACY_CLIENT_DATABASE_SHM_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE, ObserverHandoverJournal, ObserverProcessIdentity,
        OpenCodeObserverProjection, OpenCodeObserverStatus, StateError, StateRecoveryReason,
        StateRoot, TransitionLease, acquire_transition_lease,
        observer_handover_activation_ack_path, observer_handover_activation_ack_temp_path,
        observer_handover_journal_path, observer_handover_journal_temp_path,
        open_cutover_transition, read_observer_handover_activation_ack,
        read_observer_handover_journal, recover_observer_handover_journal,
        write_observer_handover_journal,
    },
};

const MAX_PRESENTATION_RETIREMENT_ATTEMPTS: usize = 8;

/// The only launch provenance that can authorize a destructive D16 cutover.
/// Hooks, observers, hidden helpers, and scripts are intentionally represented
/// so callers cannot accidentally treat them as an ordinary interactive
/// launcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CutoverLaunchKind {
    OrdinaryInteractive,
    Hook,
    ObserverSidecar,
    HiddenHelper,
    Script,
}

/// The bounded categories named by the pre-presentation confirmation.  These
/// are labels only; constructing a summary never reads `client.sqlite` or any
/// other client artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiscardedCutoverCategory {
    RemoteRegistrations,
    HostAliases,
    ClientProjectGrouping,
    ProjectHiddenState,
    CachedCapabilities,
    ExecutablePaths,
    ClientPreferences,
    LegacyPresentation,
}

impl DiscardedCutoverCategory {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RemoteRegistrations => "remote registrations",
            Self::HostAliases => "host aliases",
            Self::ClientProjectGrouping => "client Project grouping and IDs",
            Self::ProjectHiddenState => "Project hidden state",
            Self::CachedCapabilities => "cached capabilities",
            Self::ExecutablePaths => "client executable paths",
            Self::ClientPreferences => "client preferences",
            Self::LegacyPresentation => "the exact legacy presentation",
        }
    }
}

/// The bounded authoritative categories that survive cutover.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PreservedCutoverCategory {
    HostIdentity,
    Integrations,
    ProjectLocations,
    ProjectBrowserRoot,
    WorkstreamState,
    RuntimeGenerations,
    OpenCodeHandles,
    ProviderBindings,
    IndependentCreationRequests,
    Attention,
    CompoundOperations,
    PrivateRuntimeTmux,
    NativeProviderHistory,
}

impl PreservedCutoverCategory {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::HostIdentity => "host identity",
            Self::Integrations => "provider and observer integrations",
            Self::ProjectLocations => "host-local ProjectLocations",
            Self::ProjectBrowserRoot => "the Project browser root",
            Self::WorkstreamState => "Workstream provider, lifecycle, and activity state",
            Self::RuntimeGenerations => "Runtime generations",
            Self::OpenCodeHandles => "OpenCode Runtime handles",
            Self::ProviderBindings => "provider bindings",
            Self::IndependentCreationRequests => "independent Runtime creation requests",
            Self::Attention => "attention state",
            Self::CompoundOperations => "compound operations",
            Self::PrivateRuntimeTmux => "private Runtime tmux servers",
            Self::NativeProviderHistory => "native provider history",
        }
    }
}

/// The exact summary shown before a launcher presents the confirmation.  The
/// fields are private so a caller cannot silently omit one of the discarded or
/// preserved categories; use [`Self::standard`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CutoverConfirmationSummary {
    discarded: Vec<DiscardedCutoverCategory>,
    preserved: Vec<PreservedCutoverCategory>,
}

impl CutoverConfirmationSummary {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            discarded: vec![
                DiscardedCutoverCategory::RemoteRegistrations,
                DiscardedCutoverCategory::HostAliases,
                DiscardedCutoverCategory::ClientProjectGrouping,
                DiscardedCutoverCategory::ProjectHiddenState,
                DiscardedCutoverCategory::CachedCapabilities,
                DiscardedCutoverCategory::ExecutablePaths,
                DiscardedCutoverCategory::ClientPreferences,
                DiscardedCutoverCategory::LegacyPresentation,
            ],
            preserved: vec![
                PreservedCutoverCategory::HostIdentity,
                PreservedCutoverCategory::Integrations,
                PreservedCutoverCategory::ProjectLocations,
                PreservedCutoverCategory::ProjectBrowserRoot,
                PreservedCutoverCategory::WorkstreamState,
                PreservedCutoverCategory::RuntimeGenerations,
                PreservedCutoverCategory::OpenCodeHandles,
                PreservedCutoverCategory::ProviderBindings,
                PreservedCutoverCategory::IndependentCreationRequests,
                PreservedCutoverCategory::Attention,
                PreservedCutoverCategory::CompoundOperations,
                PreservedCutoverCategory::PrivateRuntimeTmux,
                PreservedCutoverCategory::NativeProviderHistory,
            ],
        }
    }

    #[must_use]
    pub fn discarded(&self) -> &[DiscardedCutoverCategory] {
        &self.discarded
    }

    #[must_use]
    pub fn preserved(&self) -> &[PreservedCutoverCategory] {
        &self.preserved
    }

    fn is_standard(&self) -> bool {
        self == &Self::standard()
    }
}

/// User/launcher input captured before any host state is opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CutoverConfirmationInput {
    pub launch_kind: CutoverLaunchKind,
    pub confirmed: bool,
    pub summary: CutoverConfirmationSummary,
}

impl CutoverConfirmationInput {
    #[must_use]
    pub fn confirmed_interactive() -> Self {
        Self {
            launch_kind: CutoverLaunchKind::OrdinaryInteractive,
            confirmed: true,
            summary: CutoverConfirmationSummary::standard(),
        }
    }

    #[must_use]
    pub fn declined_interactive() -> Self {
        Self {
            launch_kind: CutoverLaunchKind::OrdinaryInteractive,
            confirmed: false,
            summary: CutoverConfirmationSummary::standard(),
        }
    }

    fn authorize(&self) -> Result<(), CutoverError> {
        if !self.confirmed {
            return Err(CutoverError::Declined);
        }
        if self.launch_kind != CutoverLaunchKind::OrdinaryInteractive {
            return Err(CutoverError::UnauthorizedLaunch);
        }
        if !self.summary.is_standard() {
            return Err(CutoverError::InvalidConfirmationSummary);
        }
        Ok(())
    }
}

/// A proof-only source used by planning and by the repeated under-lease
/// verification.  The implementation must inspect only bounded topology and
/// process identity metadata; it must never read pane/provider content.
pub trait PresentationProofSource {
    fn prove(&mut self, state_root: &Path) -> Result<LegacyPresentationAssessment, CutoverError>;
}

/// The mutation authority for an already proven detached legacy presentation.
/// Implementations own any tmux/presentation cleanup; the orchestrator passes
/// only the exact proof that was repeated under the lease.
pub trait PresentationRetirementAuthority {
    fn retire(
        &mut self,
        proof: &LegacyPresentationProof,
        lease: &TransitionLease,
    ) -> Result<(), CutoverError>;
}

/// Combined presentation seam used by [`CutoverOrchestrator`].
pub trait PresentationAuthority: PresentationProofSource + PresentationRetirementAuthority {}

impl<T> PresentationAuthority for T where
    T: PresentationProofSource + PresentationRetirementAuthority
{
}

/// The real read-only presentation discovery adapter.  Retirement remains an
/// injected operation until D16 activation wires it to the presentation owner.
#[derive(Debug, Default)]
pub struct LivePresentationProofSource;

impl PresentationProofSource for LivePresentationProofSource {
    fn prove(&mut self, state_root: &Path) -> Result<LegacyPresentationAssessment, CutoverError> {
        classify_legacy_presentations(state_root).map_err(|error| map_presentation_error(&error))
    }
}

/// A bounded classification before confirmation and state access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CutoverPlanKind {
    Ready,
    DrainOnly,
}

/// The cutover plan carries only the canonical root and proof metadata.  It
/// never contains a state connection or a client-catalog row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CutoverPlan {
    root: PathBuf,
    presentation_root: PathBuf,
    assessment: LegacyPresentationAssessment,
    kind: CutoverPlanKind,
}

impl CutoverPlan {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the operator spelling retained for exact legacy argv proof.
    #[must_use]
    pub fn presentation_root(&self) -> &Path {
        &self.presentation_root
    }

    #[must_use]
    pub const fn kind(&self) -> CutoverPlanKind {
        self.kind
    }

    #[must_use]
    pub const fn assessment(&self) -> &LegacyPresentationAssessment {
        &self.assessment
    }

    #[must_use]
    pub const fn presentation_state(&self) -> LegacyPresentationState {
        self.assessment.state()
    }
}

/// Performs read-only presentation discovery.  This is the only planning
/// entrypoint and is safe to call before showing the confirmation summary.
pub fn discover_cutover<P: PresentationProofSource>(
    source: &mut P,
    state_root: &Path,
) -> Result<CutoverPlan, CutoverError> {
    let root = canonical_root(state_root)?;
    let presentation_root = state_root.to_path_buf();
    let assessment = source.prove(&presentation_root)?;
    let kind = match assessment.state() {
        LegacyPresentationState::None
        | LegacyPresentationState::DetachedOrdinary
        | LegacyPresentationState::DeadOwned => CutoverPlanKind::Ready,
        LegacyPresentationState::Attached
        | LegacyPresentationState::UtilityShell
        | LegacyPresentationState::ObserverReview => CutoverPlanKind::DrainOnly,
        state => return Err(CutoverError::UnsafePresentation(state)),
    };
    Ok(CutoverPlan {
        root,
        presentation_root,
        assessment,
        kind,
    })
}

/// Whether a private `OpenCode` observer is a legacy sidecar or already a D16
/// observer.  The provider Runtime itself is never targeted by this module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenCodeObserverKind {
    PreD16,
    D16,
}

/// One exact live `OpenCode` observer target assembled only after state supplies
/// its durable Runtime row and the process authority corroborates the helper.
/// The state adapter never supplies the observer kind or executable identity.
#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenCodeObserverTarget {
    pub projection: OpenCodeObserverProjection,
    pub observer: ObserverProcessIdentity,
    pub kind: OpenCodeObserverKind,
}

/// Injected host-state effects needed by D16 handover and final migration.
/// Implementations may hold a schema-12/13 connection, but the orchestrator
/// never opens one for a drain-only plan.
pub trait CutoverStateAuthority {
    /// Returns each live `OpenCode` observer together with the exact Runtime
    /// row that binds its provider PID/birth, cwd, tmux generation/session,
    /// lifecycle, and revision. A handle without this projection is not a
    /// cutover target: it cannot prove endpoint ownership or root-session
    /// status without risking adoption of another Runtime's endpoint.
    fn live_opencode_observer_projections(
        &mut self,
    ) -> Result<Vec<OpenCodeObserverProjection>, CutoverError>;

    fn current_observer(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<CurrentObserverHandleProof, CutoverError>;

    fn compare_and_swap_observer(
        &mut self,
        // The exact transition lease held across process handover and the
        // state CAS. Implementations must revalidate it immediately before
        // committing the assignment.
        lease: &TransitionLease,
        runtime_id: RuntimeId,
        expected_revision: Revision,
        standby: &ObserverProcessIdentity,
    ) -> Result<CurrentObserverHandleProof, CutoverError>;

    fn migrate_schema12_to13(
        &mut self,
        lease: &TransitionLease,
        id_generator: &dyn IdGenerator,
    ) -> Result<(), CutoverError>;
}

/// Lazily opens host state only after confirmation, lease acquisition, and the
/// final presentation-None proof.  A drain-only or declined run never invokes
/// this method.
pub trait CutoverStateFactory {
    type Authority: CutoverStateAuthority;

    fn open_under_lease(
        &mut self,
        lease: &TransitionLease,
    ) -> Result<&mut Self::Authority, CutoverError>;
}

/// The production presentation adapter used by the cutover
/// orchestrator.  It keeps the proof/retirement APIs in `presentation.rs` as
/// the sole source of presentation authority; this adapter adds only the
/// selected state-root binding required by the trait seam.
#[derive(Clone, Debug)]
pub struct LivePresentationAuthority {
    state_root: PathBuf,
}

impl LivePresentationAuthority {
    #[must_use]
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn require_root(&self, requested: &Path) -> Result<(), CutoverError> {
        let expected = fs::canonicalize(&self.state_root).map_err(|error| CutoverError::Io {
            path: self.state_root.clone(),
            source: error,
        })?;
        let actual = fs::canonicalize(requested).map_err(|error| CutoverError::Io {
            path: requested.to_path_buf(),
            source: error,
        })?;
        if expected != actual {
            return Err(CutoverError::InvalidRoot);
        }
        Ok(())
    }
}

impl PresentationProofSource for LivePresentationAuthority {
    fn prove(&mut self, state_root: &Path) -> Result<LegacyPresentationAssessment, CutoverError> {
        self.require_root(state_root)?;
        classify_legacy_presentations(state_root).map_err(|error| map_presentation_error(&error))
    }
}

impl PresentationRetirementAuthority for LivePresentationAuthority {
    fn retire(
        &mut self,
        proof: &LegacyPresentationProof,
        lease: &TransitionLease,
    ) -> Result<(), CutoverError> {
        let proof_root = proof
            .directory()
            .parent()
            .and_then(Path::parent)
            .ok_or(CutoverError::InvalidRoot)?;
        self.require_root(proof_root)?;
        self.require_root(lease.root())?;
        retire_legacy_presentation(proof_root, proof, lease)
            .map_err(|error| CutoverError::PresentationRetirement(error.to_string()))
    }
}

/// A real D16 state factory.  Opening remains lazy so the orchestrator can
/// acquire and revalidate the transition lease and the final presentation
/// proof before any host `SQLite` connection is opened.
pub struct LiveCutoverStateFactory {
    root: StateRoot,
    state: Option<D16State>,
}

impl std::fmt::Debug for LiveCutoverStateFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveCutoverStateFactory")
            .field("root", &"<private>")
            .field("state_open", &self.state.is_some())
            .finish()
    }
}

impl LiveCutoverStateFactory {
    #[must_use]
    pub fn new(root: StateRoot) -> Self {
        Self { root, state: None }
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        self.root.base()
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.is_some()
    }
}

impl CutoverStateFactory for LiveCutoverStateFactory {
    type Authority = D16State;

    fn open_under_lease(
        &mut self,
        lease: &TransitionLease,
    ) -> Result<&mut Self::Authority, CutoverError> {
        if self.state.is_some() {
            return Err(CutoverError::StateEffect(
                "D16 state authority was opened more than once".to_owned(),
            ));
        }
        let state = open_cutover_transition(&self.root, lease).map_err(CutoverError::from)?;
        self.state = Some(state);
        Ok(self
            .state
            .as_mut()
            .expect("state was inserted immediately above"))
    }
}

impl CutoverStateAuthority for D16State {
    fn live_opencode_observer_projections(
        &mut self,
    ) -> Result<Vec<OpenCodeObserverProjection>, CutoverError> {
        D16State::live_opencode_observer_projections(self).map_err(CutoverError::from)
    }

    fn current_observer(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<CurrentObserverHandleProof, CutoverError> {
        D16State::current_observer(self, runtime_id).map_err(CutoverError::from)
    }

    fn compare_and_swap_observer(
        &mut self,
        lease: &TransitionLease,
        runtime_id: RuntimeId,
        expected_revision: Revision,
        standby: &ObserverProcessIdentity,
    ) -> Result<CurrentObserverHandleProof, CutoverError> {
        D16State::compare_and_swap_observer(self, lease, runtime_id, expected_revision, standby)
            .map_err(CutoverError::from)
    }

    fn migrate_schema12_to13(
        &mut self,
        lease: &TransitionLease,
        id_generator: &dyn IdGenerator,
    ) -> Result<(), CutoverError> {
        D16State::migrate_schema12_to13(self, lease, id_generator).map_err(CutoverError::from)
    }
}

/// Injected process/observer effects.  Corroboration and standby creation
/// receive the combined state projection; subsequent effects receive only the
/// exact corroborated observer identity so no later operation can widen its
/// target.
pub trait CutoverProcessAuthority {
    fn corroborate_observer(
        &mut self,
        target: &OpenCodeObserverProjection,
    ) -> Result<CorroboratedOpenCodeObserver, CutoverError>;

    fn observe(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<ObserverProcessState, CutoverError>;

    fn start_standby(
        &mut self,
        target: &OpenCodeObserverProjection,
    ) -> Result<ObserverProcessIdentity, CutoverError>;

    fn freeze_exact(&mut self, expected: &ObserverProcessIdentity) -> Result<(), CutoverError>;

    fn restore_old_exact(&mut self, expected: &ObserverProcessIdentity)
    -> Result<(), CutoverError>;

    fn discard_standby_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError>;

    /// Sends the exact activation signal and waits for a bounded, private
    /// acknowledgement from that same process. Implementations must verify
    /// the acknowledgement's assigned Runtime handle and repeat exact PID,
    /// birth, executable, and running-state proof before returning success.
    fn activate_standby_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError>;

    fn terminate_frozen_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError>;
}

/// Process identity/status evidence returned by the injected process authority.
/// `Gone` and `IdentityMismatch` are never mutation authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObserverProcessState {
    Running(ObserverProcessIdentity),
    Stopped(ObserverProcessIdentity),
    Gone,
    IdentityMismatch,
}

/// Corroboration from the process authority, including the helper generation
/// that state alone cannot safely invent from PID/birth rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorroboratedOpenCodeObserver {
    pub observer: ObserverProcessIdentity,
    pub kind: OpenCodeObserverKind,
}

const MAX_CUTOVER_OBSERVER_CMDLINE_BYTES: usize = 16 * 1024;
const MAX_CUTOVER_OBSERVER_ARGUMENTS: usize = 16;
const CUTOVER_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CUTOVER_PROCESS_TIMEOUT: Duration = Duration::from_secs(2);
const STANDBY_READY_LINE_MAX_BYTES: usize = 512;
const STANDBY_ACTIVATION_ACK_LINE_MAX_BYTES: usize = 512;
const STANDBY_ACTIVATION_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Linux/OpenCode process authority for the cutover path.
///
/// The adapter independently verifies the exact Runtime/provider and observer
/// PID/birth/executable, cwd, generation, endpoint ownership, health, and root
/// session status before any signal. The hidden observer argv explicitly
/// distinguishes pre-D16, active D16, and standby generations. Standby
/// creation establishes its SSE stream without host-state mutation and only
/// receives activation authority after the exact handle CAS.
#[derive(Debug)]
pub struct LinuxOpenCodeCutoverProcessAuthority {
    state_root: PathBuf,
    process_probe: LinuxProcessProbe,
    standbys: BTreeMap<u32, StandbyProcess>,
}

#[derive(Debug)]
struct StandbyProcess {
    child: Child,
    activated: bool,
    activation_ack: Receiver<Result<Vec<u8>, io::Error>>,
}

impl Drop for LinuxOpenCodeCutoverProcessAuthority {
    fn drop(&mut self) {
        for standby in self.standbys.values_mut() {
            if !standby.activated {
                let _ = standby.child.kill();
                let _ = standby.child.wait();
            }
        }
    }
}

impl LinuxOpenCodeCutoverProcessAuthority {
    #[must_use]
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
            process_probe: LinuxProcessProbe,
            standbys: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn validate_runtime_projection(
        &self,
        projection: &OpenCodeObserverProjection,
    ) -> Result<(), CutoverError> {
        let runtime = &projection.runtime;
        let handle = &projection.handle;
        let binding = &projection.binding;
        if runtime.provider != ProviderKind::OpenCode
            || handle.runtime_id != runtime.runtime_id
            || handle.native_session_id.provider() != ProviderKind::OpenCode
            || binding.runtime_id != runtime.runtime_id
            || binding.provider != ProviderKind::OpenCode
            || binding.native_session_id != handle.native_session_id
            || binding.runtime_generation != handle.runtime_generation
            || handle.runtime_generation.is_empty()
            || handle.runtime_generation.len() > 256
            || handle.runtime_generation.contains(['\0', '\n', '\r'])
            || runtime.tmux_generation != handle.runtime_generation
            || runtime.tmux_session.is_empty()
            || runtime.tmux_session.len() > 256
            || runtime.tmux_session.contains(['\0', '\n', '\r'])
            || runtime.cwd.as_os_str().is_empty()
            || !runtime.cwd.is_absolute()
            || handle.endpoint_host != LOOPBACK_HOST
            || handle.endpoint_port == 0
            || handle.version.is_empty()
            || handle.version.len() > 256
            || handle.version.contains(['\0', '\n', '\r'])
            || runtime
                .process_birth
                .as_deref()
                .is_some_and(|birth| !valid_birth_token(birth))
            || handle
                .observer_birth
                .as_deref()
                .is_some_and(|birth| !valid_birth_token(birth))
            || runtime.status == RuntimeStatus::Stopped
            || handle.observer_status == OpenCodeObserverStatus::Stopped
        {
            return Err(CutoverError::InvalidObserverTarget);
        }
        let provider_pid = runtime
            .provider_pid
            .ok_or(CutoverError::RuntimeProjectionUnavailable)?;
        let provider_birth = runtime
            .process_birth
            .as_deref()
            .filter(|birth| !birth.is_empty())
            .ok_or(CutoverError::RuntimeProjectionUnavailable)?;
        let provider_evidence = read_cutover_process_evidence(provider_pid)?
            .ok_or(CutoverError::FuzzyProcessIdentity)?;
        if provider_evidence.birth != provider_birth
            || provider_evidence.status != CutoverProcessStatus::Running
            || !paths_match(&provider_evidence.cwd.to_string_lossy(), &runtime.cwd)
        {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let endpoint = OpenCodeEndpoint {
            host: handle.endpoint_host.clone(),
            port: handle.endpoint_port,
        };
        if !endpoint_owned_by_process(&endpoint, provider_pid, provider_birth) {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let client = OpenCodeClient::new(endpoint);
        let health = client
            .health()
            .map_err(|_| CutoverError::FuzzyProcessIdentity)?;
        if health.version != handle.version
            || !matches!(
                client.session_status_with_root(&handle.native_session_id, &runtime.cwd),
                Ok(OpenCodeSessionStatus::Busy | OpenCodeSessionStatus::Idle)
            )
        {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let observer_pid = handle
            .observer_pid
            .ok_or(CutoverError::InvalidObserverTarget)?;
        let observer_birth = handle
            .observer_birth
            .as_deref()
            .filter(|birth| !birth.is_empty())
            .ok_or(CutoverError::InvalidObserverTarget)?;
        let evidence = read_cutover_process_evidence(observer_pid)?
            .ok_or(CutoverError::FuzzyProcessIdentity)?;
        let current_executable = current_executable_path()?;
        if !process_identity_matches_installed_observer(
            &evidence,
            observer_birth,
            &current_executable,
        ) || evidence.status != CutoverProcessStatus::Running
        {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let _ = validate_observer_generation_command_line(
            &evidence.arguments,
            projection,
            &evidence.executable,
            &self.state_root,
        )?;
        Ok(())
    }

    fn exact_evidence(
        expected: &ObserverProcessIdentity,
    ) -> Result<CutoverProcessEvidence, CutoverError> {
        validate_cutover_observer_identity(expected)?;
        let Some(actual) = read_cutover_process_evidence(expected.pid)? else {
            return Err(CutoverError::FuzzyProcessIdentity);
        };
        if !process_identity_matches(&actual, &expected.birth, &expected.executable) {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        Ok(actual)
    }

    fn signal_exact(
        &self,
        expected: &ObserverProcessIdentity,
        signal: CutoverProcessSignal,
    ) -> Result<(), CutoverError> {
        let _ = Self::exact_evidence(expected)?;
        send_cutover_signal(expected, signal, self.process_probe)
    }

    fn wait_for_exit(expected: &ObserverProcessIdentity) -> Result<bool, CutoverError> {
        let deadline = Instant::now() + CUTOVER_PROCESS_TIMEOUT;
        loop {
            match read_cutover_process_evidence(expected.pid)? {
                None => return Ok(true),
                Some(actual)
                    if !process_identity_matches(
                        &actual,
                        &expected.birth,
                        &expected.executable,
                    ) =>
                {
                    return Ok(true);
                }
                Some(_) if Instant::now() >= deadline => return Ok(false),
                Some(_) => thread::sleep(CUTOVER_PROCESS_POLL_INTERVAL),
            }
        }
    }

    fn terminate_exact(&self, expected: &ObserverProcessIdentity) -> Result<(), CutoverError> {
        let evidence = Self::exact_evidence(expected)?;
        self.signal_exact(expected, CutoverProcessSignal::Term)?;
        if evidence.status == CutoverProcessStatus::Stopped {
            self.signal_exact(expected, CutoverProcessSignal::Continue)?;
        }
        if Self::wait_for_exit(expected)? {
            return Ok(());
        }
        // The exact PID/birth/executable proof is repeated inside signal_exact
        // immediately before escalation.  A reused PID can therefore only
        // produce a closed refusal, never a signal to its replacement.
        self.signal_exact(expected, CutoverProcessSignal::Kill)?;
        if Self::wait_for_exit(expected)? {
            Ok(())
        } else {
            Err(CutoverError::ProcessEffect(
                "observer termination timed out".to_owned(),
            ))
        }
    }
}

impl CutoverProcessAuthority for LinuxOpenCodeCutoverProcessAuthority {
    fn corroborate_observer(
        &mut self,
        target: &OpenCodeObserverProjection,
    ) -> Result<CorroboratedOpenCodeObserver, CutoverError> {
        self.validate_runtime_projection(target)?;
        let handle = &target.handle;
        let observer_pid = handle
            .observer_pid
            .ok_or(CutoverError::InvalidObserverTarget)?;
        let evidence = read_cutover_process_evidence(observer_pid)?
            .ok_or(CutoverError::FuzzyProcessIdentity)?;
        let expected_birth = handle
            .observer_birth
            .as_deref()
            .ok_or(CutoverError::InvalidObserverTarget)?;
        let current_executable = current_executable_path()?;
        if !process_identity_matches_installed_observer(
            &evidence,
            expected_birth,
            &current_executable,
        ) || evidence.status != CutoverProcessStatus::Running
        {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let observer = ObserverProcessIdentity {
            pid: observer_pid,
            birth: expected_birth.to_owned(),
            executable: evidence.executable,
        };
        validate_cutover_observer_identity(&observer)?;
        let kind = validate_observer_generation_command_line(
            &evidence.arguments,
            target,
            &observer.executable,
            &self.state_root,
        )?;
        Ok(CorroboratedOpenCodeObserver { observer, kind })
    }

    fn observe(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<ObserverProcessState, CutoverError> {
        match read_cutover_process_evidence(expected.pid)? {
            None => Ok(ObserverProcessState::Gone),
            Some(actual)
                if !process_identity_matches(&actual, &expected.birth, &expected.executable) =>
            {
                Ok(ObserverProcessState::IdentityMismatch)
            }
            Some(actual) => match actual.status {
                CutoverProcessStatus::Running => {
                    Ok(ObserverProcessState::Running(expected.clone()))
                }
                CutoverProcessStatus::Stopped => {
                    Ok(ObserverProcessState::Stopped(expected.clone()))
                }
            },
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The bounded standby handshake keeps spawn, READY proof, and exact cleanup together."
    )]
    fn start_standby(
        &mut self,
        target: &OpenCodeObserverProjection,
    ) -> Result<ObserverProcessIdentity, CutoverError> {
        self.validate_runtime_projection(target)?;
        let executable = current_executable_path()?;
        let provider_pid = target
            .runtime
            .provider_pid
            .ok_or(CutoverError::RuntimeProjectionUnavailable)?;
        let provider_birth = target
            .runtime
            .process_birth
            .as_deref()
            .ok_or(CutoverError::RuntimeProjectionUnavailable)?;
        let _assigned_revision = target
            .handle
            .revision
            .value()
            .checked_add(1)
            .and_then(|value| Revision::try_from(value).ok())
            .ok_or(CutoverError::InvalidHandoverJournal)?;
        let mut command = Command::new(&executable);
        command
            .arg("--state-root")
            .arg(&self.state_root)
            .arg("_opencode_observer_standby")
            .arg(target.runtime.runtime_id.to_string())
            .arg(&target.handle.runtime_generation)
            .arg(target.handle.endpoint_port.to_string())
            .arg(&target.handle.version)
            .arg(target.handle.native_session_id.native_id())
            .arg(provider_pid.to_string())
            .arg(&target.runtime.cwd)
            .arg(provider_birth)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::process::isolate_long_lived_helper(&mut command);
        let mut child = command.spawn().map_err(|error| CutoverError::Io {
            path: PathBuf::from(&executable),
            source: error,
        })?;
        let child_pid = child.id();
        let Some(stdout) = child.stdout.take() else {
            terminate_owned_child(&mut child);
            return Err(CutoverError::StandbyObserverUnavailable);
        };
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let (ack_sender, ack_receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let ready = read_standby_line(&mut reader, STANDBY_READY_LINE_MAX_BYTES);
            let ready_ok = ready.is_ok();
            if ready_sender.send(ready).is_err() || !ready_ok {
                return;
            }
            let ack = read_standby_line(&mut reader, STANDBY_ACTIVATION_ACK_LINE_MAX_BYTES);
            let _ = ack_sender.send(ack);
        });
        let deadline = Instant::now() + CUTOVER_PROCESS_TIMEOUT.max(Duration::from_secs(15));
        let line = loop {
            match ready_receiver.recv_timeout(CUTOVER_PROCESS_POLL_INTERVAL) {
                Ok(Ok(line)) => break line,
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => {
                    terminate_owned_child(&mut child);
                    return Err(CutoverError::StandbyObserverUnavailable);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            let child_finished = match child.try_wait() {
                Ok(status) => status.is_some(),
                Err(error) => {
                    terminate_owned_child(&mut child);
                    return Err(CutoverError::Io {
                        path: PathBuf::from(&executable),
                        source: error,
                    });
                }
            };
            if child_finished || Instant::now() >= deadline {
                terminate_owned_child(&mut child);
                return Err(CutoverError::StandbyObserverUnavailable);
            }
        };
        let (ready_pid, ready_birth) = match parse_standby_ready_line(&line) {
            Ok(identity) => identity,
            Err(error) => {
                terminate_owned_child(&mut child);
                return Err(error);
            }
        };
        if ready_pid != child_pid {
            terminate_owned_child(&mut child);
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let evidence = match read_cutover_process_evidence(child_pid) {
            Ok(Some(evidence)) => evidence,
            Ok(None) => {
                terminate_owned_child(&mut child);
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            Err(error) => {
                terminate_owned_child(&mut child);
                return Err(error);
            }
        };
        if !process_identity_matches(&evidence, &ready_birth, &executable)
            || evidence.status != CutoverProcessStatus::Running
            || !is_standby_command_line(&evidence.arguments)
        {
            terminate_owned_child(&mut child);
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        if let Err(error) = validate_observer_generation_command_line(
            &evidence.arguments,
            target,
            &evidence.executable,
            &self.state_root,
        ) {
            terminate_owned_child(&mut child);
            return Err(error);
        }
        if !endpoint_owned_by_process(
            &OpenCodeEndpoint {
                host: target.handle.endpoint_host.clone(),
                port: target.handle.endpoint_port,
            },
            provider_pid,
            provider_birth,
        ) {
            terminate_owned_child(&mut child);
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        self.standbys.insert(
            child_pid,
            StandbyProcess {
                child,
                activated: false,
                activation_ack: ack_receiver,
            },
        );
        Ok(ObserverProcessIdentity {
            pid: child_pid,
            birth: ready_birth,
            executable,
        })
    }

    fn freeze_exact(&mut self, expected: &ObserverProcessIdentity) -> Result<(), CutoverError> {
        match self.observe(expected)? {
            ObserverProcessState::Stopped(_) => Ok(()),
            ObserverProcessState::Running(_) => {
                self.signal_exact(expected, CutoverProcessSignal::Stop)?;
                wait_for_stopped_process(expected)
            }
            ObserverProcessState::Gone | ObserverProcessState::IdentityMismatch => {
                Err(CutoverError::FuzzyProcessIdentity)
            }
        }
    }

    fn restore_old_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        match self.observe(expected)? {
            ObserverProcessState::Running(_) => Ok(()),
            ObserverProcessState::Stopped(_) => {
                self.signal_exact(expected, CutoverProcessSignal::Continue)?;
                wait_for_running_process(expected)
            }
            ObserverProcessState::Gone | ObserverProcessState::IdentityMismatch => {
                Err(CutoverError::FuzzyProcessIdentity)
            }
        }
    }

    fn discard_standby_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        self.terminate_exact(expected)?;
        if let Some(mut standby) = self.standbys.remove(&expected.pid) {
            let _ = standby.child.wait();
        }
        Ok(())
    }

    fn activate_standby_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        let evidence = Self::exact_evidence(expected)?;
        if !is_standby_command_line(&evidence.arguments) {
            return Err(CutoverError::InvalidObserverTarget);
        }
        let journal = read_observer_handover_journal(&self.state_root)?
            .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
        if journal.standby_observer != *expected {
            return Err(CutoverError::StandbyActivationAckUnavailable);
        }
        let runtime_id = journal
            .runtime_id
            .parse::<RuntimeId>()
            .map_err(|_| CutoverError::StandbyActivationAckUnavailable)?;
        let generation = journal.runtime_generation.clone();
        let expected_revision = Revision::try_from(
            journal
                .expected_handle_revision
                .value()
                .checked_add(1)
                .ok_or(CutoverError::StandbyActivationAckUnavailable)?,
        )
        .map_err(|_| CutoverError::StandbyActivationAckUnavailable)?;
        if durable_activation_ack_matches(
            &self.state_root,
            &journal,
            expected,
            runtime_id,
            &generation,
            expected_revision,
        )? {
            if evidence.status != CutoverProcessStatus::Running {
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            if let Some(standby) = self.standbys.get_mut(&expected.pid) {
                standby.activated = true;
            }
            self.standbys.remove(&expected.pid);
            return Ok(());
        }
        self.signal_exact(expected, CutoverProcessSignal::Activate)?;
        if let Some(standby) = self.standbys.get(&expected.pid)
            && let Ok(ack) = standby
                .activation_ack
                .recv_timeout(STANDBY_ACTIVATION_ACK_TIMEOUT)
        {
            let ack = ack.map_err(|_| CutoverError::StandbyActivationAckUnavailable)?;
            parse_standby_activation_ack(
                &ack,
                expected,
                runtime_id,
                &generation,
                expected_revision,
            )?;
        }
        let deadline = Instant::now() + STANDBY_ACTIVATION_ACK_TIMEOUT;
        loop {
            if durable_activation_ack_matches(
                &self.state_root,
                &journal,
                expected,
                runtime_id,
                &generation,
                expected_revision,
            )? {
                break;
            }
            let current = Self::exact_evidence(expected)?;
            if current.status != CutoverProcessStatus::Running
                || !is_standby_command_line(&current.arguments)
                || Instant::now() >= deadline
            {
                return Err(CutoverError::StandbyActivationAckUnavailable);
            }
            thread::sleep(CUTOVER_PROCESS_POLL_INTERVAL);
        }
        let evidence = Self::exact_evidence(expected)?;
        if evidence.status != CutoverProcessStatus::Running
            || !is_standby_command_line(&evidence.arguments)
        {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        if let Some(standby) = self.standbys.get_mut(&expected.pid) {
            standby.activated = true;
        }
        self.standbys.remove(&expected.pid);
        Ok(())
    }

    fn terminate_frozen_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        match self.observe(expected)? {
            ObserverProcessState::Stopped(_) => self.terminate_exact(expected),
            ObserverProcessState::Running(_) => Err(CutoverError::FuzzyProcessIdentity),
            ObserverProcessState::Gone | ObserverProcessState::IdentityMismatch => {
                Err(CutoverError::FuzzyProcessIdentity)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CutoverProcessStatus {
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CutoverProcessSignal {
    Stop,
    Continue,
    Term,
    Kill,
    Activate,
}

#[derive(Clone, Debug)]
struct CutoverProcessEvidence {
    birth: String,
    executable: String,
    cwd: PathBuf,
    status: CutoverProcessStatus,
    arguments: Vec<String>,
}

fn validate_cutover_observer_identity(
    expected: &ObserverProcessIdentity,
) -> Result<(), CutoverError> {
    if expected.pid == 0
        || !valid_birth_token(&expected.birth)
        || expected.executable.is_empty()
        || expected.executable.len() > 4096
        || expected.executable.contains(['\0', '\n', '\r'])
    {
        Err(CutoverError::InvalidObserverTarget)
    } else {
        Ok(())
    }
}

fn valid_birth_token(birth: &str) -> bool {
    !birth.is_empty() && birth.len() <= 256 && !birth.contains(['\0', '\n', '\r'])
}

fn process_identity_matches(
    actual: &CutoverProcessEvidence,
    expected_birth: &str,
    expected_executable: &str,
) -> bool {
    actual.birth == expected_birth
        && executable_paths_match(&actual.executable, expected_executable)
}

/// Accepts the one executable identity transition that an in-place D16
/// upgrade necessarily creates. Linux exposes an already-running old inode as
/// `/absolute/wsnav (deleted)` after the installed path is replaced. The
/// stripped spelling is accepted only when it is an absolute path matching
/// both the helper's original `argv[0]` and this process's current executable
/// spelling. Every other executable mismatch remains closed.
fn process_identity_matches_installed_observer(
    actual: &CutoverProcessEvidence,
    expected_birth: &str,
    current_executable: &str,
) -> bool {
    if actual.birth != expected_birth {
        return false;
    }
    if executable_paths_match(&actual.executable, current_executable) {
        return true;
    }
    let Some(underlying) = actual.executable.strip_suffix(" (deleted)") else {
        return false;
    };
    underlying.starts_with('/')
        && underlying == current_executable
        && actual
            .arguments
            .first()
            .is_some_and(|argument| argument == underlying)
}

fn executable_paths_match(actual: &str, expected: &str) -> bool {
    let actual_path = Path::new(actual);
    let expected_path = Path::new(expected);
    match (
        fs::canonicalize(actual_path),
        fs::canonicalize(expected_path),
    ) {
        (Ok(actual), Ok(expected)) => actual == expected,
        _ => actual_path == expected_path,
    }
}

fn current_executable_path() -> Result<String, CutoverError> {
    let executable = std::env::current_exe().map_err(|_| {
        CutoverError::ProcessEffect("current observer executable is unavailable".to_owned())
    })?;
    executable.to_str().map(str::to_owned).ok_or_else(|| {
        CutoverError::ProcessEffect("current observer executable is malformed".to_owned())
    })
}

fn read_cutover_process_evidence(pid: u32) -> Result<Option<CutoverProcessEvidence>, CutoverError> {
    if pid == 0 {
        return Err(CutoverError::InvalidObserverTarget);
    }
    let stat_path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(CutoverError::ProcessEffect(
                "observer process metadata is unavailable".to_owned(),
            ));
        }
    };
    let close = stat.rfind(')').ok_or_else(|| {
        CutoverError::ProcessEffect("observer process metadata is malformed".to_owned())
    })?;
    let fields = stat
        .get(close + 2..)
        .ok_or_else(|| {
            CutoverError::ProcessEffect("observer process metadata is malformed".to_owned())
        })?
        .split_whitespace()
        .collect::<Vec<_>>();
    let state = fields
        .first()
        .and_then(|value| value.chars().next())
        .ok_or_else(|| {
            CutoverError::ProcessEffect("observer process metadata is malformed".to_owned())
        })?;
    let birth = fields
        .get(19)
        .ok_or_else(|| {
            CutoverError::ProcessEffect("observer process metadata is malformed".to_owned())
        })?
        .to_string();
    let status = match state {
        'T' | 't' => CutoverProcessStatus::Stopped,
        'Z' => {
            return Err(CutoverError::ProcessEffect(
                "observer process became a zombie".to_owned(),
            ));
        }
        _ => CutoverProcessStatus::Running,
    };
    let executable = fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| {
        CutoverError::ProcessEffect("observer executable identity is unavailable".to_owned())
    })?;
    let executable = executable
        .to_str()
        .ok_or_else(|| {
            CutoverError::ProcessEffect("observer executable identity is malformed".to_owned())
        })?
        .to_owned();
    let cwd = fs::read_link(format!("/proc/{pid}/cwd")).map_err(|_| {
        CutoverError::ProcessEffect("observer working-directory identity is unavailable".to_owned())
    })?;
    let command_line = fs::read(format!("/proc/{pid}/cmdline")).map_err(|_| {
        CutoverError::ProcessEffect("observer command line is unavailable".to_owned())
    })?;
    if command_line.len() > MAX_CUTOVER_OBSERVER_CMDLINE_BYTES {
        return Err(CutoverError::ProcessEffect(
            "observer command line exceeded its bound".to_owned(),
        ));
    }
    let mut arguments = Vec::new();
    for argument in command_line.split(|byte| *byte == 0) {
        if argument.is_empty() {
            continue;
        }
        let argument = std::str::from_utf8(argument).map_err(|_| {
            CutoverError::ProcessEffect("observer command line is malformed".to_owned())
        })?;
        arguments.push(argument.to_owned());
        if arguments.len() > MAX_CUTOVER_OBSERVER_ARGUMENTS {
            return Err(CutoverError::ProcessEffect(
                "observer command line exceeded its argument bound".to_owned(),
            ));
        }
    }
    Ok(Some(CutoverProcessEvidence {
        birth,
        executable,
        cwd,
        status,
        arguments,
    }))
}

fn validate_observer_generation_command_line(
    arguments: &[String],
    target: &OpenCodeObserverProjection,
    executable: &str,
    state_root: &Path,
) -> Result<OpenCodeObserverKind, CutoverError> {
    let runtime = &target.runtime;
    let handle = &target.handle;
    let provider_birth = runtime
        .process_birth
        .as_deref()
        .ok_or(CutoverError::RuntimeProjectionUnavailable)?;
    let provider_pid = runtime
        .provider_pid
        .ok_or(CutoverError::RuntimeProjectionUnavailable)?;
    let executable_argument = arguments
        .first()
        .ok_or(CutoverError::FuzzyProcessIdentity)?;
    let executable_argument_matches = executable_paths_match(executable_argument, executable)
        || executable
            .strip_suffix(" (deleted)")
            .is_some_and(|underlying| {
                underlying.starts_with('/') && executable_argument == underlying
            });
    let common = arguments.len() >= 7
        && executable_argument_matches
        && arguments[1] == "--state-root"
        && paths_match(&arguments[2], state_root)
        && arguments[4] == handle.runtime_id.to_string()
        && arguments[5] == handle.runtime_generation
        && arguments[6] == handle.endpoint_port.to_string();
    if !common {
        return Err(CutoverError::FuzzyProcessIdentity);
    }
    match arguments[3].as_str() {
        "_opencode_observer" if arguments.len() == 11 => {
            if arguments[7] != handle.native_session_id.native_id()
                || arguments[8] != provider_pid.to_string()
                || !paths_match(&arguments[9], &runtime.cwd)
                || arguments[10] != provider_birth
            {
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            Ok(OpenCodeObserverKind::PreD16)
        }
        "_opencode_observer_d16" if arguments.len() == 11 => {
            if arguments[7] != handle.native_session_id.native_id()
                || arguments[8] != provider_pid.to_string()
                || !paths_match(&arguments[9], &runtime.cwd)
                || arguments[10] != provider_birth
            {
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            Ok(OpenCodeObserverKind::D16)
        }
        "_opencode_observer_standby" if arguments.len() == 12 => {
            if arguments[7] != handle.version
                || arguments[8] != handle.native_session_id.native_id()
                || arguments[9] != provider_pid.to_string()
                || !paths_match(&arguments[10], &runtime.cwd)
                || arguments[11] != provider_birth
            {
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            Ok(OpenCodeObserverKind::D16)
        }
        _ => Err(CutoverError::FuzzyProcessIdentity),
    }
}

fn is_standby_command_line(arguments: &[String]) -> bool {
    arguments.len() == 12
        && arguments
            .get(3)
            .is_some_and(|value| value == "_opencode_observer_standby")
}

fn parse_standby_ready_line(line: &[u8]) -> Result<(u32, String), CutoverError> {
    if line.is_empty() || line.len() > 512 {
        return Err(CutoverError::StandbyObserverUnavailable);
    }
    let line = std::str::from_utf8(line)
        .map_err(|_| CutoverError::StandbyObserverUnavailable)?
        .strip_suffix('\n')
        .ok_or(CutoverError::StandbyObserverUnavailable)?;
    let mut fields = line.split(' ');
    if fields.next() != Some("READY") || fields.clone().count() != 2 {
        return Err(CutoverError::StandbyObserverUnavailable);
    }
    let pid = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .ok_or(CutoverError::StandbyObserverUnavailable)?;
    let birth = fields
        .next()
        .filter(|birth| valid_birth_token(birth))
        .ok_or(CutoverError::StandbyObserverUnavailable)?
        .to_owned();
    Ok((pid, birth))
}

fn parse_standby_activation_ack(
    line: &[u8],
    expected: &ObserverProcessIdentity,
    expected_runtime_id: RuntimeId,
    expected_generation: &str,
    expected_revision: Revision,
) -> Result<(), CutoverError> {
    if line.is_empty() || line.len() > STANDBY_ACTIVATION_ACK_LINE_MAX_BYTES {
        return Err(CutoverError::StandbyActivationAckUnavailable);
    }
    let line = std::str::from_utf8(line)
        .map_err(|_| CutoverError::StandbyActivationAckUnavailable)?
        .strip_suffix('\n')
        .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
    let mut fields = line.splitn(6, ' ');
    if fields.next() != Some("ACTIVATED") {
        return Err(CutoverError::StandbyActivationAckUnavailable);
    }
    let pid = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
    let birth = fields
        .next()
        .filter(|birth| valid_birth_token(birth))
        .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
    let runtime_id = fields
        .next()
        .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
    let revision = fields
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| Revision::try_from(value).ok())
        .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
    let generation = fields
        .next()
        .filter(|generation| !generation.is_empty())
        .ok_or(CutoverError::StandbyActivationAckUnavailable)?;
    if pid != expected.pid
        || birth != expected.birth
        || runtime_id != expected_runtime_id.to_string()
        || revision != expected_revision
        || generation != expected_generation
    {
        return Err(CutoverError::StandbyActivationAckUnavailable);
    }
    Ok(())
}

fn durable_activation_ack_matches(
    root: &Path,
    journal: &ObserverHandoverJournal,
    expected: &ObserverProcessIdentity,
    expected_runtime_id: RuntimeId,
    expected_generation: &str,
    expected_revision: Revision,
) -> Result<bool, CutoverError> {
    let Some(ack) = read_observer_handover_activation_ack(root)? else {
        return Ok(false);
    };
    if !ack.matches_journal(journal)?
        || ack.standby_observer != *expected
        || ack.runtime_id != expected_runtime_id.to_string()
        || ack.runtime_generation != expected_generation
        || ack.handle_revision != expected_revision
    {
        return Err(CutoverError::StandbyActivationAckUnavailable);
    }
    Ok(true)
}

fn read_standby_line<R: Read>(reader: &mut BufReader<R>, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader
        .take(max_bytes.saturating_add(1) as u64)
        .read_until(b'\n', &mut line)
        .map(|_| line)
}

fn terminate_owned_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn paths_match(actual: &str, expected: &Path) -> bool {
    let actual = Path::new(actual);
    match (fs::canonicalize(actual), fs::canonicalize(expected)) {
        (Ok(actual), Ok(expected)) => actual == expected,
        _ => actual == expected,
    }
}

fn wait_for_stopped_process(expected: &ObserverProcessIdentity) -> Result<(), CutoverError> {
    let deadline = Instant::now() + CUTOVER_PROCESS_TIMEOUT;
    loop {
        match read_cutover_process_evidence(expected.pid)? {
            Some(actual)
                if process_identity_matches(&actual, &expected.birth, &expected.executable)
                    && actual.status == CutoverProcessStatus::Stopped =>
            {
                return Ok(());
            }
            Some(actual)
                if !process_identity_matches(&actual, &expected.birth, &expected.executable) =>
            {
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            None => return Err(CutoverError::FuzzyProcessIdentity),
            Some(_) if Instant::now() >= deadline => {
                return Err(CutoverError::ProcessEffect(
                    "observer did not stop within the bounded interval".to_owned(),
                ));
            }
            Some(_) => thread::sleep(CUTOVER_PROCESS_POLL_INTERVAL),
        }
    }
}

fn wait_for_running_process(expected: &ObserverProcessIdentity) -> Result<(), CutoverError> {
    let deadline = Instant::now() + CUTOVER_PROCESS_TIMEOUT;
    loop {
        match read_cutover_process_evidence(expected.pid)? {
            Some(actual)
                if process_identity_matches(&actual, &expected.birth, &expected.executable)
                    && actual.status == CutoverProcessStatus::Running =>
            {
                return Ok(());
            }
            Some(actual)
                if !process_identity_matches(&actual, &expected.birth, &expected.executable) =>
            {
                return Err(CutoverError::FuzzyProcessIdentity);
            }
            None => return Err(CutoverError::FuzzyProcessIdentity),
            Some(_) if Instant::now() >= deadline => {
                return Err(CutoverError::ProcessEffect(
                    "observer did not resume within the bounded interval".to_owned(),
                ));
            }
            Some(_) => thread::sleep(CUTOVER_PROCESS_POLL_INTERVAL),
        }
    }
}

fn send_cutover_signal(
    expected: &ObserverProcessIdentity,
    signal: CutoverProcessSignal,
    process_probe: LinuxProcessProbe,
) -> Result<(), CutoverError> {
    #[cfg(target_os = "linux")]
    {
        let pid = i32::try_from(expected.pid).map_err(|_| CutoverError::InvalidObserverTarget)?;
        let pid = rustix::process::Pid::from_raw(pid).ok_or(CutoverError::InvalidObserverTarget)?;
        let pidfd = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
            .map_err(|_| CutoverError::FuzzyProcessIdentity)?;
        let Some(actual_birth) = process_probe
            .process_birth_checked(expected.pid)
            .map_err(|_| CutoverError::FuzzyProcessIdentity)?
        else {
            return Err(CutoverError::FuzzyProcessIdentity);
        };
        if actual_birth != expected.birth {
            return Err(CutoverError::FuzzyProcessIdentity);
        }
        let signal = match signal {
            CutoverProcessSignal::Stop => rustix::process::Signal::STOP,
            CutoverProcessSignal::Continue => rustix::process::Signal::CONT,
            CutoverProcessSignal::Term => rustix::process::Signal::TERM,
            CutoverProcessSignal::Kill => rustix::process::Signal::KILL,
            CutoverProcessSignal::Activate => rustix::process::Signal::USR1,
        };
        rustix::process::pidfd_send_signal(&pidfd, signal)
            .map_err(|_| CutoverError::ProcessEffect("exact observer signal failed".to_owned()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (expected, signal, process_probe);
        Err(CutoverError::ProcessEffect(
            "exact OpenCode observer controls require Linux pidfd support".to_owned(),
        ))
    }
}

/// Final result of a cutover orchestration attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CutoverOutcome {
    Declined,
    DrainOnly(LegacyPresentationState),
    Completed(CutoverReport),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CutoverReport {
    pub retired_presentations: usize,
    pub handed_over_runtimes: Vec<RuntimeId>,
    pub removed_client_files: usize,
}

/// Exact cleanup failure categories.  The cleanup helper never reads or
/// imports file contents, and it validates all three paths before deleting any
/// one of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyClientArtifactReason {
    Symlink,
    NonRegular,
    Foreign,
    NonPrivate,
    Changed,
}

/// Typed orchestration failures.  The categories intentionally avoid carrying
/// provider payloads, pane bytes, or raw process diagnostics.
#[derive(Debug, Error)]
pub enum CutoverError {
    #[error("cutover confirmation declined")]
    Declined,
    #[error("only an explicitly confirmed ordinary interactive launch may authorize cutover")]
    UnauthorizedLaunch,
    #[error("cutover confirmation summary is incomplete or altered")]
    InvalidConfirmationSummary,
    #[error("cutover root is unavailable or not a private directory")]
    InvalidRoot,
    #[error("legacy presentation is attached or requires drain-only review")]
    DrainOnly,
    #[error("legacy presentation is unsafe: {0:?}")]
    UnsafePresentation(LegacyPresentationState),
    #[error("presentation proof changed under the transition lease")]
    PresentationProofChanged,
    #[error("presentation did not retire after bounded exact attempts")]
    PresentationNotRetired,
    #[error("presentation inspection failed: {0}")]
    PresentationInspection(String),
    #[error("presentation retirement failed: {0}")]
    PresentationRetirement(String),
    #[error("ambiguous OpenCode Runtime identity")]
    AmbiguousRuntime,
    #[error("invalid OpenCode observer target")]
    InvalidObserverTarget,
    #[error("fuzzy or changed observer process identity")]
    FuzzyProcessIdentity,
    #[error("exact OpenCode cutover Runtime projection is unavailable")]
    RuntimeProjectionUnavailable,
    #[error("OpenCode observer generation identity is unavailable")]
    ObserverGenerationUnavailable,
    #[error("D16 standby OpenCode observer is unavailable before exact activation")]
    StandbyObserverUnavailable,
    #[error("D16 standby OpenCode observer activation acknowledgement is missing or invalid")]
    StandbyActivationAckUnavailable,
    #[error("observer handover journal is not recoverable")]
    InvalidHandoverJournal,
    #[error("legacy client artifact is unsafe: {0:?}")]
    LegacyClientArtifact(LegacyClientArtifactReason),
    #[error("I/O at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("state effect failed: {0}")]
    StateEffect(String),
    #[error("process effect failed: {0}")]
    ProcessEffect(String),
    #[error(transparent)]
    State(#[from] StateError),
}

/// Owns one cutover orchestration run.  The generic authorities make every
/// external effect deterministic in tests and leave D15 routing untouched.
pub struct CutoverOrchestrator<'a, P, R, F> {
    presentation: &'a mut P,
    process: &'a mut R,
    state_factory: &'a mut F,
}

impl<'a, P, R, F> CutoverOrchestrator<'a, P, R, F>
where
    P: PresentationAuthority,
    R: CutoverProcessAuthority,
    F: CutoverStateFactory,
{
    #[must_use]
    pub fn new(presentation: &'a mut P, process: &'a mut R, state_factory: &'a mut F) -> Self {
        Self {
            presentation,
            process,
            state_factory,
        }
    }

    /// Executes a confirmed ready plan.  Decline is a successful no-op;
    /// drain-only and unsafe plans return without opening state or acquiring a
    /// transition lease.
    pub fn execute(
        &mut self,
        plan: &CutoverPlan,
        confirmation: &CutoverConfirmationInput,
        id_generator: &dyn IdGenerator,
    ) -> Result<CutoverOutcome, CutoverError> {
        match confirmation.authorize() {
            Ok(()) => {}
            Err(CutoverError::Declined) => return Ok(CutoverOutcome::Declined),
            Err(error) => return Err(error),
        }
        if plan.kind == CutoverPlanKind::DrainOnly {
            return Ok(CutoverOutcome::DrainOnly(plan.presentation_state()));
        }

        let acquisition = acquire_or_create_transition_lease(&plan.root)?;
        let created_lock = acquisition.created;
        let lock_identity = acquisition.identity;
        let mut lease = acquisition.lease;
        let under_lease = match self.presentation.prove(&plan.presentation_root) {
            Ok(assessment) => assessment,
            Err(error) => {
                return abort_pre_state_lease(lease, created_lock, lock_identity, false, error);
            }
        };
        if under_lease != plan.assessment {
            return abort_pre_state_lease(
                lease,
                created_lock,
                lock_identity,
                false,
                CutoverError::PresentationProofChanged,
            );
        }

        let retired_presentations = match retire_until_none(
            self.presentation,
            &mut lease,
            &plan.presentation_root,
            under_lease,
        ) {
            Ok(retired) => retired,
            Err(error) => {
                return abort_pre_state_lease(lease, created_lock, lock_identity, true, error);
            }
        };

        // A final independent proof is required immediately before opening
        // state.  This keeps a presentation appearing after retirement from
        // authorizing client deletion or migration.
        let no_presentation = match self.presentation.prove(&plan.presentation_root) {
            Ok(assessment) => assessment,
            Err(error) => {
                return abort_pre_state_lease(
                    lease,
                    created_lock,
                    lock_identity,
                    retired_presentations > 0,
                    error,
                );
            }
        };
        if no_presentation.state() != LegacyPresentationState::None
            || no_presentation.proof().is_some()
        {
            return abort_pre_state_lease(
                lease,
                created_lock,
                lock_identity,
                retired_presentations > 0,
                CutoverError::PresentationNotRetired,
            );
        }

        let state = self.state_factory.open_under_lease(&lease)?;
        let mut handed_over_runtimes = Vec::new();
        resume_existing_handover(&mut lease, self.process, state)?;
        let mut projections = state.live_opencode_observer_projections()?;
        projections.sort_by_key(|projection| projection.runtime.runtime_id);
        for pair in projections.windows(2) {
            if pair[0].runtime.runtime_id == pair[1].runtime.runtime_id {
                return Err(CutoverError::AmbiguousRuntime);
            }
        }
        for projection in projections {
            let corroborated = self.process.corroborate_observer(&projection)?;
            let target = OpenCodeObserverTarget {
                projection,
                observer: corroborated.observer,
                kind: corroborated.kind,
            };
            validate_observer_target(&target)?;
            let changed = execute_observer_target(&mut lease, self.process, state, &target)?;
            if changed {
                handed_over_runtimes.push(target.projection.runtime.runtime_id);
            }
        }

        // The controller may be started by an older binary while handovers
        // run.  Prove the exact presentation is still absent immediately
        // before touching the three legacy files.
        let before_cleanup = self.presentation.prove(&plan.presentation_root)?;
        if before_cleanup.state() != LegacyPresentationState::None
            || before_cleanup.proof().is_some()
        {
            return Err(CutoverError::PresentationNotRetired);
        }
        let removed_client_files = remove_legacy_client_files(&lease, lock_identity)?;
        state.migrate_schema12_to13(&lease, id_generator)?;

        let root = lease.root().to_path_buf();
        remove_transition_lock(&root, lock_identity)?;
        drop(lease);
        Ok(CutoverOutcome::Completed(CutoverReport {
            retired_presentations,
            handed_over_runtimes,
            removed_client_files,
        }))
    }
}

fn abort_pre_state_lease<T>(
    lease: TransitionLease,
    created_lock: bool,
    lock_identity: ClientArtifactIdentity,
    durable_effect: bool,
    error: CutoverError,
) -> Result<T, CutoverError> {
    let root = lease.root().to_path_buf();
    if created_lock && !durable_effect {
        let _ = remove_transition_lock(&root, lock_identity);
    }
    drop(lease);
    Err(error)
}

fn map_presentation_error(error: &PresentationError) -> CutoverError {
    if matches!(error, PresentationError::AmbiguousLegacyPresentations) {
        CutoverError::UnsafePresentation(LegacyPresentationState::Malformed)
    } else {
        CutoverError::PresentationInspection(error.to_string())
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf, CutoverError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| CutoverError::Io {
        path: root.to_path_buf(),
        source: error,
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(CutoverError::InvalidRoot);
    }
    fs::canonicalize(root).map_err(|error| CutoverError::Io {
        path: root.to_path_buf(),
        source: error,
    })
}

fn retire_until_none<P: PresentationAuthority>(
    presentation: &mut P,
    lease: &mut TransitionLease,
    presentation_root: &Path,
    mut assessment: LegacyPresentationAssessment,
) -> Result<usize, CutoverError> {
    let mut retired = 0;
    for attempt in 0..MAX_PRESENTATION_RETIREMENT_ATTEMPTS {
        match assessment.state() {
            LegacyPresentationState::None => return Ok(retired),
            LegacyPresentationState::DetachedOrdinary | LegacyPresentationState::DeadOwned => {
                let proof = assessment
                    .proof()
                    .ok_or(CutoverError::PresentationProofChanged)?;
                presentation
                    .retire(proof, lease)
                    .map_err(|error| match error {
                        CutoverError::PresentationRetirement(_) => error,
                        other => CutoverError::PresentationRetirement(other.to_string()),
                    })?;
                retired += 1;
            }
            LegacyPresentationState::Attached
            | LegacyPresentationState::UtilityShell
            | LegacyPresentationState::ObserverReview => return Err(CutoverError::DrainOnly),
            state => return Err(CutoverError::UnsafePresentation(state)),
        }
        let next = presentation.prove(presentation_root)?;
        if next.state() == LegacyPresentationState::None {
            return Ok(retired);
        }
        if next.proof() != assessment.proof() {
            return Err(CutoverError::PresentationProofChanged);
        }
        if attempt + 1 == MAX_PRESENTATION_RETIREMENT_ATTEMPTS {
            return Err(CutoverError::PresentationNotRetired);
        }
        assessment = next;
    }
    Err(CutoverError::PresentationNotRetired)
}

struct LeaseAcquisition {
    lease: TransitionLease,
    created: bool,
    identity: ClientArtifactIdentity,
}

fn acquire_or_create_transition_lease(root: &Path) -> Result<LeaseAcquisition, CutoverError> {
    acquire_or_create_transition_lease_with(root, |path| {
        acquire_transition_lease(path).map_err(CutoverError::from)
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "The create-and-acquire sequence keeps each exact identity check adjacent to its cleanup boundary."
)]
fn acquire_or_create_transition_lease_with<A>(
    root: &Path,
    mut acquire: A,
) -> Result<LeaseAcquisition, CutoverError>
where
    A: FnMut(&Path) -> Result<TransitionLease, CutoverError>,
{
    let root = canonical_root(root)?;
    let lock_path = root.join("transition.lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || !is_private_owner_file(&metadata)
            {
                return Err(CutoverError::InvalidRoot);
            }
            let lease = acquire(&root)?;
            Ok(LeaseAcquisition {
                lease,
                created: false,
                identity: client_identity(&metadata),
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
                options.mode(0o600);
            }
            let file = options.open(&lock_path).map_err(|error| CutoverError::Io {
                path: lock_path.clone(),
                source: error,
            })?;
            let metadata = file.metadata().map_err(|error| CutoverError::Io {
                path: lock_path.clone(),
                source: error,
            })?;
            let identity = client_identity(&metadata);
            if !is_private_owner_file(&metadata) {
                drop(file);
                let _ = remove_exact_created_lock(&lock_path, identity);
                return Err(CutoverError::InvalidRoot);
            }
            let path_metadata = match fs::symlink_metadata(&lock_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    drop(file);
                    let _ = remove_exact_created_lock(&lock_path, identity);
                    return Err(CutoverError::Io {
                        path: lock_path.clone(),
                        source: error,
                    });
                }
            };
            if path_metadata.file_type().is_symlink()
                || !path_metadata.is_file()
                || !same_client_identity(&path_metadata, &identity)
            {
                drop(file);
                return Err(CutoverError::LegacyClientArtifact(
                    LegacyClientArtifactReason::Changed,
                ));
            }
            if let Err(error) = file.sync_all() {
                drop(file);
                let _ = remove_exact_created_lock(&lock_path, identity);
                return Err(CutoverError::Io {
                    path: lock_path.clone(),
                    source: error,
                });
            }
            if let Err(error) = sync_directory(&root) {
                drop(file);
                let _ = remove_exact_created_lock(&lock_path, identity);
                return Err(error);
            }
            match acquire(&root) {
                Ok(lease) => {
                    drop(file);
                    Ok(LeaseAcquisition {
                        lease,
                        created: true,
                        identity,
                    })
                }
                Err(error) => {
                    let locked = matches!(
                        error,
                        CutoverError::State(StateError::StateRecoveryRequired(
                            StateRecoveryReason::LockedTransitionLease
                        ))
                    );
                    drop(file);
                    if !locked {
                        let _ = remove_exact_created_lock(&lock_path, identity);
                    }
                    Err(error)
                }
            }
        }
        Err(error) => Err(CutoverError::Io {
            path: lock_path,
            source: error,
        }),
    }
}

fn remove_exact_created_lock(
    path: &Path,
    expected_identity: ClientArtifactIdentity,
) -> Result<(), CutoverError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CutoverError::Io {
                path: path.to_path_buf(),
                source: error,
            });
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || !same_client_identity(&metadata, &expected_identity)
    {
        return Ok(());
    }
    fs::remove_file(path).map_err(|error| CutoverError::Io {
        path: path.to_path_buf(),
        source: error,
    })?;
    let root = path.parent().ok_or(CutoverError::InvalidRoot)?;
    sync_directory(root)
}

fn remove_transition_lock(
    root: &Path,
    expected_identity: ClientArtifactIdentity,
) -> Result<(), CutoverError> {
    validate_transition_lock(root, expected_identity)?;
    let path = root.join("transition.lock");
    fs::remove_file(&path).map_err(|error| CutoverError::Io {
        path: path.clone(),
        source: error,
    })?;
    sync_directory(root)
}

fn validate_transition_lock(
    root: &Path,
    expected_identity: ClientArtifactIdentity,
) -> Result<(), CutoverError> {
    let path = root.join("transition.lock");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CutoverError::LegacyClientArtifact(
                LegacyClientArtifactReason::Changed,
            ));
        }
        Err(error) => {
            return Err(CutoverError::Io {
                path,
                source: error,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || !is_private_owner_file(&metadata)
    {
        return Err(CutoverError::InvalidRoot);
    }
    if !same_client_identity(&metadata, &expected_identity) {
        return Err(CutoverError::LegacyClientArtifact(
            LegacyClientArtifactReason::Changed,
        ));
    }
    Ok(())
}

fn validate_observer_target(target: &OpenCodeObserverTarget) -> Result<(), CutoverError> {
    if target.projection.handle.native_session_id.provider() != ProviderKind::OpenCode
        || target.projection.handle.runtime_generation.is_empty()
        || target.projection.handle.observer_pid != Some(target.observer.pid)
        || target.projection.handle.observer_birth.as_deref()
            != Some(target.observer.birth.as_str())
        || target.observer.pid == 0
        || target.observer.birth.is_empty()
        || target.observer.executable.is_empty()
        || target.observer.birth.contains(['\0', '\n', '\r'])
        || target.observer.executable.contains(['\0', '\n', '\r'])
    {
        return Err(CutoverError::InvalidObserverTarget);
    }
    Ok(())
}

fn require_exact_identity(
    actual: &ObserverProcessIdentity,
    expected: &ObserverProcessIdentity,
) -> Result<(), CutoverError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CutoverError::FuzzyProcessIdentity)
    }
}

fn require_running(
    status: Result<ObserverProcessState, CutoverError>,
    expected: &ObserverProcessIdentity,
) -> Result<(), CutoverError> {
    match status? {
        ObserverProcessState::Running(actual) => require_exact_identity(&actual, expected),
        ObserverProcessState::Stopped(_)
        | ObserverProcessState::Gone
        | ObserverProcessState::IdentityMismatch => Err(CutoverError::FuzzyProcessIdentity),
    }
}

fn require_stopped(
    status: Result<ObserverProcessState, CutoverError>,
    expected: &ObserverProcessIdentity,
) -> Result<(), CutoverError> {
    match status? {
        ObserverProcessState::Stopped(actual) => require_exact_identity(&actual, expected),
        ObserverProcessState::Running(_)
        | ObserverProcessState::Gone
        | ObserverProcessState::IdentityMismatch => Err(CutoverError::FuzzyProcessIdentity),
    }
}

fn require_restoreable(
    status: Result<ObserverProcessState, CutoverError>,
    expected: &ObserverProcessIdentity,
) -> Result<(), CutoverError> {
    match status? {
        ObserverProcessState::Running(actual) | ObserverProcessState::Stopped(actual) => {
            require_exact_identity(&actual, expected)
        }
        ObserverProcessState::Gone | ObserverProcessState::IdentityMismatch => {
            Err(CutoverError::FuzzyProcessIdentity)
        }
    }
}

fn require_current_handle(
    current: &CurrentObserverHandleProof,
    target: &OpenCodeObserverTarget,
    expected_pid: u32,
    expected_birth: &str,
    expected_revision: Revision,
) -> Result<(), CutoverError> {
    if current.runtime_id != target.projection.runtime.runtime_id
        || current.runtime_generation != target.projection.handle.runtime_generation
        || current.pid != expected_pid
        || current.birth != expected_birth
        || current.revision != expected_revision
    {
        return Err(CutoverError::FuzzyProcessIdentity);
    }
    Ok(())
}

fn execute_observer_target<R, S>(
    lease: &mut TransitionLease,
    process: &mut R,
    state: &mut S,
    target: &OpenCodeObserverTarget,
) -> Result<bool, CutoverError>
where
    R: CutoverProcessAuthority,
    S: CutoverStateAuthority,
{
    let current = state.current_observer(target.projection.runtime.runtime_id)?;
    require_current_handle(
        &current,
        target,
        target.observer.pid,
        &target.observer.birth,
        target.projection.handle.revision,
    )?;
    require_running(process.observe(&target.observer), &target.observer)?;
    if target.kind == OpenCodeObserverKind::D16 {
        return Ok(false);
    }

    let standby = process.start_standby(&target.projection)?;
    if standby == target.observer
        || standby.pid == 0
        || standby.birth.is_empty()
        || standby.executable.is_empty()
    {
        return Err(CutoverError::InvalidObserverTarget);
    }
    if let Err(error) = require_running(process.observe(&standby), &standby) {
        let _ = process.discard_standby_exact(&standby);
        return Err(error);
    }

    let mut journal = ObserverHandoverJournal {
        version: 1,
        runtime_id: target.projection.runtime.runtime_id.to_string(),
        runtime_generation: target.projection.handle.runtime_generation.clone(),
        old_observer: target.observer.clone(),
        standby_observer: standby.clone(),
        expected_handle_revision: target.projection.handle.revision,
        phase: HandoverPhase::Prepared,
    };
    if let Err(error) = write_observer_handover_journal(lease, &journal) {
        let _ = process.discard_standby_exact(&standby);
        return Err(error.into());
    }
    journal.transition(HandoverPhase::StandbyReady)?;
    write_observer_handover_journal(lease, &journal)?;

    if let Err(error) = process.freeze_exact(&target.observer) {
        let _ = process.discard_standby_exact(&standby);
        return Err(error);
    }
    require_stopped(process.observe(&target.observer), &target.observer)?;
    let frozen_handle = state.current_observer(target.projection.runtime.runtime_id)?;
    require_current_handle(
        &frozen_handle,
        target,
        target.observer.pid,
        &target.observer.birth,
        target.projection.handle.revision,
    )?;
    journal.transition(HandoverPhase::OldFrozen)?;
    write_observer_handover_journal(lease, &journal)?;

    let swapped = state.compare_and_swap_observer(
        lease,
        target.projection.runtime.runtime_id,
        target.projection.handle.revision,
        &standby,
    )?;
    let next_revision = Revision::try_from(
        target
            .projection
            .handle
            .revision
            .value()
            .checked_add(1)
            .ok_or(CutoverError::InvalidHandoverJournal)?,
    )
    .map_err(|_| CutoverError::InvalidHandoverJournal)?;
    require_current_handle(&swapped, target, standby.pid, &standby.birth, next_revision)?;
    // The CAS result is durable state evidence; corroborate the exact standby
    // one more time before making it authoritative or draining its buffer.
    require_running(process.observe(&standby), &standby)?;
    journal.transition(HandoverPhase::HandleSwapped)?;
    write_observer_handover_journal(lease, &journal)?;

    process.activate_standby_exact(&standby)?;
    journal.transition(HandoverPhase::OldCleaning)?;
    write_observer_handover_journal(lease, &journal)?;
    require_stopped(process.observe(&target.observer), &target.observer)?;
    process.terminate_frozen_exact(&target.observer)?;
    journal.transition(HandoverPhase::Complete)?;
    write_observer_handover_journal(lease, &journal)?;
    remove_handover_journal(lease)?;
    Ok(true)
}

/// Replays one exact durable handover journal after an interrupted cutover.
/// The phase machine remains crate-private so callers cannot treat raw journal
/// replay as an authorization primitive; the orchestrator invokes it only
/// after confirmation, lease acquisition, and presentation proof.
pub(crate) fn resume_handover_journal<R, S>(
    lease: &mut TransitionLease,
    process: &mut R,
    state: &mut S,
) -> Result<bool, CutoverError>
where
    R: CutoverProcessAuthority,
    S: CutoverStateAuthority,
{
    let Some(mut journal) = recover_observer_handover_journal(lease)? else {
        return Ok(false);
    };
    let runtime_id = journal
        .runtime_id
        .parse::<RuntimeId>()
        .map_err(|_| CutoverError::InvalidHandoverJournal)?;
    let current = state.current_observer(runtime_id)?;
    let action = journal
        .restart_action(&current)
        .map_err(|_| CutoverError::InvalidHandoverJournal)?;
    match action {
        HandoverRestartAction::RestoreOldObserver => {
            require_restoreable(
                process.observe(&journal.old_observer),
                &journal.old_observer,
            )?;
            process.restore_old_exact(&journal.old_observer)?;
            process.discard_standby_exact(&journal.standby_observer)?;
            remove_handover_journal(lease)?;
        }
        HandoverRestartAction::FinishOldObserverCleanup => {
            require_running(
                process.observe(&journal.standby_observer),
                &journal.standby_observer,
            )?;
            if journal.phase == HandoverPhase::OldFrozen {
                journal.transition(HandoverPhase::HandleSwapped)?;
                write_observer_handover_journal(lease, &journal)?;
            }
            process.activate_standby_exact(&journal.standby_observer)?;
            if journal.phase == HandoverPhase::HandleSwapped {
                journal.transition(HandoverPhase::OldCleaning)?;
                write_observer_handover_journal(lease, &journal)?;
            }
            match process.observe(&journal.old_observer)? {
                ObserverProcessState::Stopped(actual) => {
                    require_exact_identity(&actual, &journal.old_observer)?;
                    process.terminate_frozen_exact(&journal.old_observer)?;
                }
                ObserverProcessState::Gone => {}
                ObserverProcessState::Running(_) | ObserverProcessState::IdentityMismatch => {
                    return Err(CutoverError::FuzzyProcessIdentity);
                }
            }
            if journal.phase == HandoverPhase::OldCleaning {
                journal.transition(HandoverPhase::Complete)?;
                write_observer_handover_journal(lease, &journal)?;
            }
            remove_handover_journal(lease)?;
        }
        HandoverRestartAction::RemoveJournal => {
            require_running(
                process.observe(&journal.standby_observer),
                &journal.standby_observer,
            )?;
            match process.observe(&journal.old_observer)? {
                ObserverProcessState::Gone => {}
                ObserverProcessState::Stopped(actual) => {
                    require_exact_identity(&actual, &journal.old_observer)?;
                }
                ObserverProcessState::Running(_) | ObserverProcessState::IdentityMismatch => {
                    return Err(CutoverError::FuzzyProcessIdentity);
                }
            }
            remove_handover_journal(lease)?;
        }
    }
    Ok(true)
}

fn resume_existing_handover<R, S>(
    lease: &mut TransitionLease,
    process: &mut R,
    state: &mut S,
) -> Result<(), CutoverError>
where
    R: CutoverProcessAuthority,
    S: CutoverStateAuthority,
{
    let _ = resume_handover_journal(lease, process, state)?;
    Ok(())
}

fn remove_handover_journal(lease: &TransitionLease) -> Result<(), CutoverError> {
    let root = lease.root();
    for path in [
        observer_handover_activation_ack_path(root),
        observer_handover_activation_ack_temp_path(root),
        observer_handover_journal_path(root),
        observer_handover_journal_temp_path(root),
    ] {
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || !is_private_owner_file(&metadata)
                {
                    return Err(CutoverError::InvalidHandoverJournal);
                }
                fs::remove_file(&path).map_err(|error| CutoverError::Io {
                    path: path.clone(),
                    source: error,
                })?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CutoverError::Io {
                    path,
                    source: error,
                });
            }
        }
    }
    sync_directory(root)
}

/// Removes only the three exact legacy client paths. It performs metadata
/// validation for every path first, never opens or reads a client database,
/// treats missing files as success, and syncs the root directory. The exact
/// leased transition-lock identity is revalidated before any removal.
fn remove_legacy_client_files(
    lease: &TransitionLease,
    lock_identity: ClientArtifactIdentity,
) -> Result<usize, CutoverError> {
    let root = lease.root();
    validate_transition_lock(root, lock_identity)?;
    let names = [
        LEGACY_CLIENT_DATABASE_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE,
        LEGACY_CLIENT_DATABASE_SHM_FILE,
    ];
    let mut artifacts = Vec::new();
    for name in names {
        let path = root.join(name);
        let Some(identity) = inspect_client_artifact(&path)? else {
            continue;
        };
        artifacts.push((path, identity));
    }
    let mut removed = 0;
    for (path, identity) in artifacts {
        validate_transition_lock(root, lock_identity)?;
        let current = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(CutoverError::Io {
                    path,
                    source: error,
                });
            }
        };
        if !same_client_identity(&current, &identity) {
            return Err(CutoverError::LegacyClientArtifact(
                LegacyClientArtifactReason::Changed,
            ));
        }
        fs::remove_file(&path).map_err(|error| CutoverError::Io {
            path: path.clone(),
            source: error,
        })?;
        removed += 1;
    }
    sync_directory(root)?;
    Ok(removed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClientArtifactIdentity {
    size: u64,
    mode: u32,
    device: u64,
    inode: u64,
    uid: u32,
}

fn inspect_client_artifact(path: &Path) -> Result<Option<ClientArtifactIdentity>, CutoverError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CutoverError::Io {
                path: path.to_path_buf(),
                source: error,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(CutoverError::LegacyClientArtifact(
            LegacyClientArtifactReason::Symlink,
        ));
    }
    if !metadata.is_file() {
        return Err(CutoverError::LegacyClientArtifact(
            LegacyClientArtifactReason::NonRegular,
        ));
    }
    if !is_current_owner_metadata(&metadata) {
        return Err(CutoverError::LegacyClientArtifact(
            LegacyClientArtifactReason::Foreign,
        ));
    }
    if !is_private_owner_file(&metadata) {
        return Err(CutoverError::LegacyClientArtifact(
            LegacyClientArtifactReason::NonPrivate,
        ));
    }
    Ok(Some(client_identity(&metadata)))
}

fn same_client_identity(metadata: &fs::Metadata, identity: &ClientArtifactIdentity) -> bool {
    let current = client_identity(metadata);
    current == *identity
}

fn client_identity(metadata: &fs::Metadata) -> ClientArtifactIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ClientArtifactIdentity {
            size: metadata.len(),
            mode: metadata.mode(),
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
        }
    }
    #[cfg(not(unix))]
    {
        ClientArtifactIdentity {
            size: metadata.len(),
            mode: 0,
            device: 0,
            inode: 0,
            uid: 0,
        }
    }
}

#[cfg(test)]
fn set_private_file_permissions(file: &File, path: &Path) -> Result<(), CutoverError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| CutoverError::Io {
                path: path.to_path_buf(),
                source: error,
            })?;
    }
    #[cfg(not(unix))]
    let _ = (file, path);
    Ok(())
}

#[allow(clippy::verbose_bit_mask)]
fn is_private_owner_directory(metadata: &fs::Metadata) -> bool {
    if !is_current_owner_metadata(metadata) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777 == 0o700
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[allow(clippy::verbose_bit_mask)]
fn is_private_owner_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && is_current_owner_metadata(metadata) && {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777 == 0o600
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

fn is_current_owner_metadata(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use nix::unistd::Uid;
        use std::os::unix::fs::MetadataExt;
        metadata.uid() == Uid::current().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn sync_directory(path: &Path) -> Result<(), CutoverError> {
    let directory = File::open(path).map_err(|error| CutoverError::Io {
        path: path.to_path_buf(),
        source: error,
    })?;
    directory.sync_all().map_err(|error| CutoverError::Io {
        path: path.to_path_buf(),
        source: error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RestartProcess {
        stopped: Option<ObserverProcessIdentity>,
        gone: Option<ObserverProcessIdentity>,
        mismatch: bool,
        calls: Vec<&'static str>,
    }

    impl CutoverProcessAuthority for RestartProcess {
        fn corroborate_observer(
            &mut self,
            _target: &OpenCodeObserverProjection,
        ) -> Result<CorroboratedOpenCodeObserver, CutoverError> {
            Err(CutoverError::ProcessEffect(
                "not used in restart test".to_owned(),
            ))
        }

        fn observe(
            &mut self,
            expected: &ObserverProcessIdentity,
        ) -> Result<ObserverProcessState, CutoverError> {
            self.calls.push("observe");
            if self.mismatch {
                return Ok(ObserverProcessState::IdentityMismatch);
            }
            if self.gone.as_ref() == Some(expected) {
                return Ok(ObserverProcessState::Gone);
            }
            if self.stopped.as_ref() == Some(expected) {
                return Ok(ObserverProcessState::Stopped(expected.clone()));
            }
            Ok(ObserverProcessState::Running(expected.clone()))
        }

        fn start_standby(
            &mut self,
            _target: &OpenCodeObserverProjection,
        ) -> Result<ObserverProcessIdentity, CutoverError> {
            Err(CutoverError::ProcessEffect(
                "not used in restart test".to_owned(),
            ))
        }

        fn freeze_exact(
            &mut self,
            _expected: &ObserverProcessIdentity,
        ) -> Result<(), CutoverError> {
            Err(CutoverError::ProcessEffect(
                "not used in restart test".to_owned(),
            ))
        }

        fn restore_old_exact(
            &mut self,
            expected: &ObserverProcessIdentity,
        ) -> Result<(), CutoverError> {
            self.calls.push("restore");
            self.stopped = None;
            self.gone = None;
            let _ = expected;
            Ok(())
        }

        fn discard_standby_exact(
            &mut self,
            _expected: &ObserverProcessIdentity,
        ) -> Result<(), CutoverError> {
            self.calls.push("discard");
            Ok(())
        }

        fn activate_standby_exact(
            &mut self,
            _expected: &ObserverProcessIdentity,
        ) -> Result<(), CutoverError> {
            self.calls.push("activate");
            Ok(())
        }

        fn terminate_frozen_exact(
            &mut self,
            expected: &ObserverProcessIdentity,
        ) -> Result<(), CutoverError> {
            self.calls.push("terminate");
            self.gone = Some(expected.clone());
            self.stopped = None;
            Ok(())
        }
    }

    struct RestartState {
        current: CurrentObserverHandleProof,
    }

    impl CutoverStateAuthority for RestartState {
        fn live_opencode_observer_projections(
            &mut self,
        ) -> Result<Vec<OpenCodeObserverProjection>, CutoverError> {
            Err(CutoverError::StateEffect(
                "not used in restart test".to_owned(),
            ))
        }

        fn current_observer(
            &mut self,
            _runtime_id: RuntimeId,
        ) -> Result<CurrentObserverHandleProof, CutoverError> {
            Ok(self.current.clone())
        }

        fn compare_and_swap_observer(
            &mut self,
            _lease: &TransitionLease,
            _runtime_id: RuntimeId,
            _expected_revision: Revision,
            _standby: &ObserverProcessIdentity,
        ) -> Result<CurrentObserverHandleProof, CutoverError> {
            Err(CutoverError::StateEffect(
                "not used in restart test".to_owned(),
            ))
        }

        fn migrate_schema12_to13(
            &mut self,
            _lease: &TransitionLease,
            _id_generator: &dyn IdGenerator,
        ) -> Result<(), CutoverError> {
            Err(CutoverError::StateEffect(
                "not used in restart test".to_owned(),
            ))
        }
    }

    fn restart_identity(birth: &str, pid: u32) -> ObserverProcessIdentity {
        ObserverProcessIdentity {
            pid,
            birth: birth.to_owned(),
            executable: "/private/wsnav-observer".to_owned(),
        }
    }

    fn restart_lease(root: &Path) -> TransitionLease {
        fs::create_dir_all(root).expect("restart root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700)).expect("root mode");
        }
        let lock = root.join("transition.lock");
        File::create(&lock).expect("transition lock");
        set_private_file_permissions(&File::open(&lock).expect("lock"), &lock).expect("lock mode");
        acquire_transition_lease(root).expect("transition lease")
    }

    fn restart_journal(
        runtime_id: RuntimeId,
        old: ObserverProcessIdentity,
        standby: ObserverProcessIdentity,
        phase: HandoverPhase,
    ) -> ObserverHandoverJournal {
        ObserverHandoverJournal {
            version: 1,
            runtime_id: runtime_id.to_string(),
            runtime_generation: "generation-a".to_owned(),
            old_observer: old,
            standby_observer: standby,
            expected_handle_revision: Revision::INITIAL,
            phase,
        }
    }

    fn restart_state(
        runtime_id: RuntimeId,
        observer: &ObserverProcessIdentity,
        revision: Revision,
    ) -> RestartState {
        RestartState {
            current: CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: observer.pid,
                birth: observer.birth.clone(),
                revision,
            },
        }
    }

    #[test]
    fn summary_names_all_bounded_categories_without_state_access() {
        let summary = CutoverConfirmationSummary::standard();
        assert_eq!(summary.discarded().len(), 8);
        assert_eq!(summary.preserved().len(), 13);
        assert!(
            summary
                .discarded()
                .iter()
                .all(|category| !category.label().is_empty())
        );
        assert!(
            summary
                .preserved()
                .iter()
                .all(|category| !category.label().is_empty())
        );
    }

    #[test]
    fn noninteractive_confirmation_never_authorizes() {
        let mut input = CutoverConfirmationInput::confirmed_interactive();
        input.launch_kind = CutoverLaunchKind::Hook;
        assert!(matches!(
            input.authorize(),
            Err(CutoverError::UnauthorizedLaunch)
        ));
    }

    #[test]
    fn upgraded_deleted_observer_executable_requires_exact_original_spelling() {
        let current_executable = "/private/wsnav";
        let evidence = CutoverProcessEvidence {
            birth: "birth-old".to_owned(),
            executable: "/private/wsnav (deleted)".to_owned(),
            cwd: PathBuf::from("/private/project"),
            status: CutoverProcessStatus::Running,
            arguments: vec![current_executable.to_owned(), "--state-root".to_owned()],
        };
        assert!(process_identity_matches_installed_observer(
            &evidence,
            "birth-old",
            current_executable,
        ));

        let mut wrong_argv = evidence.clone();
        wrong_argv.arguments[0] = "/private/other-wsnav".to_owned();
        assert!(!process_identity_matches_installed_observer(
            &wrong_argv,
            "birth-old",
            current_executable,
        ));

        let mut arbitrary_deleted_path = evidence.clone();
        arbitrary_deleted_path.executable = "/tmp/unrelated-wsnav (deleted)".to_owned();
        arbitrary_deleted_path.arguments[0] = "/tmp/unrelated-wsnav".to_owned();
        assert!(!process_identity_matches_installed_observer(
            &arbitrary_deleted_path,
            "birth-old",
            current_executable,
        ));
    }

    #[test]
    fn activation_ack_requires_exact_process_and_state_handle_identity() {
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(77));
        let expected = ObserverProcessIdentity {
            pid: 202,
            birth: "standby-birth".to_owned(),
            executable: "/private/wsnav".to_owned(),
        };
        let generation = "generation-a";
        let revision = Revision::INITIAL.next();
        let line = format!(
            "ACTIVATED {} {} {} {} {}\n",
            expected.pid,
            expected.birth,
            runtime_id,
            revision.value(),
            generation,
        );
        assert!(
            parse_standby_activation_ack(
                line.as_bytes(),
                &expected,
                runtime_id,
                generation,
                revision,
            )
            .is_ok()
        );

        for malformed in [
            "",
            "ACTIVATED 202 standby-birth wrong-runtime 2 generation-a\n",
            "ACTIVATED 999 standby-birth 00000000-0000-0000-0000-00000000004d 2 generation-a\n",
            "ACTIVATED 202 standby-birth 00000000-0000-0000-0000-00000000004d 1 generation-a\n",
            "ACTIVATED 202 standby-birth 00000000-0000-0000-0000-00000000004d 2 generation-b\n",
        ] {
            assert!(matches!(
                parse_standby_activation_ack(
                    malformed.as_bytes(),
                    &expected,
                    runtime_id,
                    generation,
                    revision,
                ),
                Err(CutoverError::StandbyActivationAckUnavailable)
            ));
        }
    }

    #[test]
    fn durable_activation_ack_survives_launcher_local_channel_loss() {
        let temporary = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let lease = restart_lease(temporary.path());
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(77));
        let expected = ObserverProcessIdentity {
            pid: 202,
            birth: "standby-birth".to_owned(),
            executable: "/private/wsnav".to_owned(),
        };
        let mut journal = ObserverHandoverJournal {
            version: 1,
            runtime_id: runtime_id.to_string(),
            runtime_generation: "generation-a".to_owned(),
            old_observer: ObserverProcessIdentity {
                pid: 201,
                birth: "old-birth".to_owned(),
                executable: "/private/wsnav".to_owned(),
            },
            standby_observer: expected.clone(),
            expected_handle_revision: Revision::INITIAL,
            phase: HandoverPhase::Prepared,
        };
        write_observer_handover_journal(&lease, &journal).unwrap();
        for phase in [
            HandoverPhase::StandbyReady,
            HandoverPhase::OldFrozen,
            HandoverPhase::HandleSwapped,
        ] {
            journal.transition(phase).unwrap();
            write_observer_handover_journal(&lease, &journal).unwrap();
        }
        crate::state::write_observer_handover_activation_ack(
            temporary.path(),
            &crate::state::ObserverHandoverActivationAck {
                version: 1,
                runtime_id: runtime_id.to_string(),
                runtime_generation: "generation-a".to_owned(),
                standby_observer: expected.clone(),
                handle_revision: Revision::INITIAL.next(),
            },
        )
        .unwrap();
        assert!(
            durable_activation_ack_matches(
                temporary.path(),
                &journal,
                &expected,
                runtime_id,
                "generation-a",
                Revision::INITIAL.next(),
            )
            .unwrap()
        );
    }

    #[test]
    fn new_lock_is_private_at_creation_and_exact_failed_acquisition_cleans_it() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let root = temporary.path();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root, fs::Permissions::from_mode(0o700)).expect("root mode");
        }
        let mut observed = false;
        let result = acquire_or_create_transition_lease_with(root, |acquire_root| {
            let path = acquire_root.join("transition.lock");
            let metadata = fs::symlink_metadata(&path).expect("new lock metadata");
            assert!(is_private_owner_file(&metadata));
            observed = true;
            Err(CutoverError::StateEffect(
                "forced acquisition failure".to_owned(),
            ))
        });
        assert!(matches!(result, Err(CutoverError::StateEffect(_))));
        assert!(observed);
        assert!(!root.join("transition.lock").exists());
    }

    #[test]
    fn restart_replays_every_journal_phase_and_removes_only_after_proof() {
        for phase in [
            HandoverPhase::Prepared,
            HandoverPhase::StandbyReady,
            HandoverPhase::OldFrozen,
            HandoverPhase::HandleSwapped,
            HandoverPhase::OldCleaning,
            HandoverPhase::Complete,
        ] {
            let temporary = tempfile::tempdir().expect("temporary root");
            let root = temporary.path();
            let mut lease = restart_lease(root);
            let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(100 + phase as u128));
            let old = restart_identity("old", 301);
            let standby = restart_identity("standby", 302);
            let current_is_standby = matches!(
                phase,
                HandoverPhase::HandleSwapped | HandoverPhase::OldCleaning | HandoverPhase::Complete
            );
            let current_observer = if current_is_standby { &standby } else { &old };
            let current_revision = if current_is_standby {
                Revision::INITIAL.next()
            } else {
                Revision::INITIAL
            };
            let journal = restart_journal(runtime_id, old.clone(), standby.clone(), phase);
            write_observer_handover_journal(&lease, &journal).expect("journal");
            let mut process = RestartProcess::default();
            if current_is_standby {
                process.gone = Some(old.clone());
            }
            let mut state = restart_state(runtime_id, current_observer, current_revision);
            assert!(resume_handover_journal(&mut lease, &mut process, &mut state).is_ok());
            assert!(!observer_handover_journal_path(root).exists());
        }
    }

    #[test]
    fn completed_restart_refuses_to_forget_a_running_old_observer() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let root = temporary.path();
        let mut lease = restart_lease(root);
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(119));
        let old = restart_identity("old", 391);
        let standby = restart_identity("standby", 392);
        let journal = restart_journal(runtime_id, old, standby.clone(), HandoverPhase::Complete);
        write_observer_handover_journal(&lease, &journal).expect("journal");
        let mut process = RestartProcess::default();
        let mut state = restart_state(runtime_id, &standby, Revision::INITIAL.next());

        assert!(matches!(
            resume_handover_journal(&mut lease, &mut process, &mut state),
            Err(CutoverError::FuzzyProcessIdentity)
        ));
        assert!(observer_handover_journal_path(root).exists());
    }

    #[test]
    fn restart_restores_exact_stopped_old_after_freeze_interruption() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let root = temporary.path();
        let mut lease = restart_lease(root);
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(120));
        let old = restart_identity("old", 401);
        let standby = restart_identity("standby", 402);
        let journal = restart_journal(
            runtime_id,
            old.clone(),
            standby,
            HandoverPhase::StandbyReady,
        );
        write_observer_handover_journal(&lease, &journal).expect("journal");
        let mut process = RestartProcess {
            stopped: Some(old.clone()),
            ..RestartProcess::default()
        };
        let mut state = restart_state(runtime_id, &old, Revision::INITIAL);
        assert!(resume_handover_journal(&mut lease, &mut process, &mut state).is_ok());
        assert!(process.calls.contains(&"restore"));
        assert!(!process.calls.contains(&"terminate"));
    }

    #[test]
    fn old_cleaning_restart_accepts_exact_old_gone_without_signaling() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let root = temporary.path();
        let mut lease = restart_lease(root);
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(121));
        let old = restart_identity("old", 411);
        let standby = restart_identity("standby", 412);
        let journal = restart_journal(
            runtime_id,
            old.clone(),
            standby.clone(),
            HandoverPhase::OldCleaning,
        );
        write_observer_handover_journal(&lease, &journal).expect("journal");
        let mut process = RestartProcess {
            gone: Some(old),
            ..RestartProcess::default()
        };
        let mut state = restart_state(runtime_id, &standby, Revision::INITIAL.next());
        assert!(resume_handover_journal(&mut lease, &mut process, &mut state).is_ok());
        assert!(!process.calls.contains(&"terminate"));
    }

    #[test]
    fn fuzzy_restart_identity_signals_nothing() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let root = temporary.path();
        let mut lease = restart_lease(root);
        let runtime_id = RuntimeId::from(uuid::Uuid::from_u128(122));
        let old = restart_identity("old", 421);
        let standby = restart_identity("standby", 422);
        let journal = restart_journal(
            runtime_id,
            old.clone(),
            standby,
            HandoverPhase::StandbyReady,
        );
        write_observer_handover_journal(&lease, &journal).expect("journal");
        let mut process = RestartProcess {
            mismatch: true,
            ..RestartProcess::default()
        };
        let mut state = restart_state(runtime_id, &old, Revision::INITIAL);
        let error = resume_handover_journal(&mut lease, &mut process, &mut state)
            .expect_err("fuzzy process identity must refuse");
        assert!(matches!(error, CutoverError::FuzzyProcessIdentity));
        assert!(!process.calls.contains(&"restore"));
        assert!(!process.calls.contains(&"discard"));
        assert!(observer_handover_journal_path(root).exists());
    }
}
