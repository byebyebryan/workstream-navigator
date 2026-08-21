//! Deterministic D16 cutover orchestration coverage.

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use uuid::Uuid;
use wsnav::{
    cutover::{
        CorroboratedOpenCodeObserver, CutoverConfirmationInput, CutoverError, CutoverOrchestrator,
        CutoverOutcome, CutoverProcessAuthority, CutoverStateAuthority, CutoverStateFactory,
        LivePresentationProofSource, ObserverProcessState, OpenCodeObserverKind,
        PresentationProofSource, PresentationRetirementAuthority, discover_cutover,
    },
    domain::{IdGenerator, ProviderKind, Revision, RuntimeId, RuntimeStatus, WorkstreamId},
    presentation::{
        LegacyFileIdentity, LegacyPresentationAssessment, LegacyPresentationEvidenceForTest,
        LegacyPresentationPaneEvidenceForTest, LegacyPresentationProof, PresentationPaneRole,
        PresentationPaths, classify_legacy_evidence,
    },
    provider::names::NameState,
    state::{
        CurrentObserverHandleProof, HandoverPhase, ObserverProcessIdentity,
        OpenCodeObserverProjection, OpenCodeObserverStatus, OpenCodeRuntimeHandle, ProviderBinding,
        RuntimeRecord, TransitionLease, acquire_transition_lease,
    },
};

#[derive(Default)]
struct SequenceIds;

impl IdGenerator for SequenceIds {
    fn uuid(&self) -> Uuid {
        Uuid::from_u128(1)
    }
}

/// The empty presentation classifier is real and read-only; it gives tests a
/// canonical `None` assessment without constructing private presentation
/// internals or reading any pane content.
struct EmptyPresentation {
    root: PathBuf,
}

impl PresentationProofSource for EmptyPresentation {
    fn prove(&mut self, _root: &Path) -> Result<LegacyPresentationAssessment, CutoverError> {
        let mut source = LivePresentationProofSource;
        source
            .prove(&self.root)
            .map_err(|error| CutoverError::PresentationInspection(error.to_string()))
    }
}

impl PresentationRetirementAuthority for EmptyPresentation {
    fn retire(
        &mut self,
        _proof: &LegacyPresentationProof,
        _lease: &TransitionLease,
    ) -> Result<(), CutoverError> {
        panic!("empty presentation cannot require retirement")
    }
}

struct ScriptedPresentation {
    assessments: VecDeque<LegacyPresentationAssessment>,
    retire_calls: usize,
}

impl PresentationProofSource for ScriptedPresentation {
    fn prove(&mut self, _root: &Path) -> Result<LegacyPresentationAssessment, CutoverError> {
        self.assessments
            .pop_front()
            .ok_or_else(|| CutoverError::PresentationInspection("script exhausted".to_owned()))
    }
}

impl PresentationRetirementAuthority for ScriptedPresentation {
    fn retire(
        &mut self,
        _proof: &LegacyPresentationProof,
        _lease: &TransitionLease,
    ) -> Result<(), CutoverError> {
        self.retire_calls += 1;
        Ok(())
    }
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

#[allow(clippy::too_many_arguments)]
fn proof_pane(
    role: PresentationPaneRole,
    id: &str,
    pid: u32,
    birth: u64,
    arguments: &[String],
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    root: &Path,
) -> LegacyPresentationPaneEvidenceForTest {
    let _ = root;
    LegacyPresentationPaneEvidenceForTest {
        id: id.to_owned(),
        role,
        dead: false,
        pid: Some(pid),
        process_pid: None,
        birth: Some(birth),
        process_stable: true,
        executable_path: Some(PathBuf::from("/workspace/wsnav")),
        executable_identity: Some(proof_identity()),
        arguments: arguments.to_vec(),
        left,
        top,
        width,
        height,
        window_width: 128,
        window_height: 24,
    }
}

fn presentation_assessment(
    root: &Path,
    clients: &[&str],
    navigator_pid: u32,
) -> LegacyPresentationAssessment {
    let paths = PresentationPaths::fresh(root);
    let navigator_arguments = vec![
        "/workspace/wsnav".to_owned(),
        "--state-root".to_owned(),
        root.to_string_lossy().into_owned(),
        "_navigator".to_owned(),
        "--presentation-socket".to_owned(),
        paths.socket.to_string_lossy().into_owned(),
        "--presentation-session".to_owned(),
        paths.session_name.clone(),
    ];
    let provider_arguments = vec![
        "/workspace/wsnav".to_owned(),
        "--state-root".to_owned(),
        root.to_string_lossy().into_owned(),
        "_provider_wait".to_owned(),
    ];
    let evidence = LegacyPresentationEvidenceForTest {
        executable_path: PathBuf::from("/workspace/wsnav"),
        config_identity: None,
        session_id: Some("$0".to_owned()),
        window_id: Some("@0".to_owned()),
        panes: vec![
            proof_pane(
                PresentationPaneRole::Navigator,
                "%0",
                navigator_pid,
                11,
                &navigator_arguments,
                0,
                0,
                32,
                24,
                root,
            ),
            proof_pane(
                PresentationPaneRole::Provider,
                "%1",
                102,
                12,
                &provider_arguments,
                33,
                0,
                95,
                24,
                root,
            ),
        ],
        clients: clients.iter().map(|client| (*client).to_owned()).collect(),
        shell_claim_present: false,
        attachment_status: None,
    };
    classify_legacy_evidence(&paths.directory, root, evidence)
}

struct NoopProcess {
    calls: Vec<&'static str>,
    fail_at: Option<&'static str>,
    wrong_observation: bool,
    frozen: Option<ObserverProcessIdentity>,
    gone: Option<ObserverProcessIdentity>,
    corroborated_kind: OpenCodeObserverKind,
    corroboration_error: Option<CutoverError>,
}

impl Default for NoopProcess {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            fail_at: None,
            wrong_observation: false,
            frozen: None,
            gone: None,
            corroborated_kind: OpenCodeObserverKind::PreD16,
            corroboration_error: None,
        }
    }
}

impl NoopProcess {
    fn fail(&mut self, name: &'static str) -> Result<(), CutoverError> {
        self.calls.push(name);
        if self.fail_at == Some(name) {
            return Err(CutoverError::ProcessEffect(name.to_owned()));
        }
        Ok(())
    }
}

impl CutoverProcessAuthority for NoopProcess {
    fn corroborate_observer(
        &mut self,
        target: &OpenCodeObserverProjection,
    ) -> Result<CorroboratedOpenCodeObserver, CutoverError> {
        self.fail("corroborate")?;
        if let Some(error) = self.corroboration_error.take() {
            return Err(error);
        }
        let pid = target
            .handle
            .observer_pid
            .ok_or(CutoverError::InvalidObserverTarget)?;
        let birth = target
            .handle
            .observer_birth
            .clone()
            .ok_or(CutoverError::InvalidObserverTarget)?;
        Ok(CorroboratedOpenCodeObserver {
            observer: observer(&birth, pid),
            kind: self.corroborated_kind,
        })
    }

    fn observe(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<ObserverProcessState, CutoverError> {
        self.fail("observe")?;
        if self.wrong_observation {
            return Ok(ObserverProcessState::Running(observer("fuzzy", 999)));
        }
        if self.gone.as_ref() == Some(expected) {
            return Ok(ObserverProcessState::Gone);
        }
        if self.frozen.as_ref() == Some(expected) {
            Ok(ObserverProcessState::Stopped(expected.clone()))
        } else {
            Ok(ObserverProcessState::Running(expected.clone()))
        }
    }

    fn start_standby(
        &mut self,
        _target: &OpenCodeObserverProjection,
    ) -> Result<ObserverProcessIdentity, CutoverError> {
        self.fail("start-standby")?;
        Ok(observer("standby", 202))
    }

    fn freeze_exact(&mut self, expected: &ObserverProcessIdentity) -> Result<(), CutoverError> {
        self.fail("freeze")?;
        self.frozen = Some(expected.clone());
        Ok(())
    }

    fn restore_old_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        self.fail("restore")?;
        self.frozen = None;
        self.gone = None;
        let _ = expected;
        Ok(())
    }

    fn discard_standby_exact(
        &mut self,
        _expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        self.fail("discard-standby")
    }

    fn activate_standby_exact(
        &mut self,
        _expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        self.fail("activate")
    }

    fn terminate_frozen_exact(
        &mut self,
        expected: &ObserverProcessIdentity,
    ) -> Result<(), CutoverError> {
        self.fail("terminate")?;
        self.gone = Some(expected.clone());
        self.frozen = None;
        let _ = expected;
        Ok(())
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct StateFake {
    opened: bool,
    migration_called: bool,
    migration_saw_no_client_files: bool,
    observers: Vec<OpenCodeRuntimeHandle>,
    current: Option<CurrentObserverHandleProof>,
    swapped: Option<CurrentObserverHandleProof>,
    replace_lock_on_migrate: bool,
    remove_lock_before_cleanup: bool,
    root: Option<PathBuf>,
}

impl CutoverStateAuthority for StateFake {
    fn live_opencode_observer_projections(
        &mut self,
    ) -> Result<Vec<OpenCodeObserverProjection>, CutoverError> {
        self.opened = true;
        if self.remove_lock_before_cleanup {
            let root = self.root.as_ref().expect("state test root");
            fs::remove_file(root.join("transition.lock"))
                .expect("remove transition lock before cleanup");
        }
        Ok(self
            .observers
            .iter()
            .cloned()
            .map(projection_from_handle)
            .collect())
    }

    fn current_observer(
        &mut self,
        _runtime_id: RuntimeId,
    ) -> Result<CurrentObserverHandleProof, CutoverError> {
        self.opened = true;
        self.current
            .clone()
            .ok_or_else(|| CutoverError::StateEffect("missing current observer".to_owned()))
    }

    fn compare_and_swap_observer(
        &mut self,
        _lease: &TransitionLease,
        _runtime_id: RuntimeId,
        _expected_revision: Revision,
        _standby: &ObserverProcessIdentity,
    ) -> Result<CurrentObserverHandleProof, CutoverError> {
        self.opened = true;
        self.swapped
            .clone()
            .ok_or_else(|| CutoverError::StateEffect("missing swap result".to_owned()))
    }

    fn migrate_schema12_to13(
        &mut self,
        lease: &TransitionLease,
        _id_generator: &dyn IdGenerator,
    ) -> Result<(), CutoverError> {
        self.migration_called = true;
        self.migration_saw_no_client_files =
            ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"]
                .iter()
                .all(|name| !lease.root().join(name).exists());
        if self.replace_lock_on_migrate {
            let lock = lease.root().join("transition.lock");
            fs::remove_file(&lock).expect("replace leased lock path");
            File::create(&lock).expect("replacement transition lock");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(lock, fs::Permissions::from_mode(0o600))
                    .expect("replacement lock mode");
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct StateFactory {
    opened: bool,
    state: StateFake,
}

impl CutoverStateFactory for StateFactory {
    type Authority = StateFake;

    fn open_under_lease(
        &mut self,
        _lease: &TransitionLease,
    ) -> Result<&mut Self::Authority, CutoverError> {
        self.opened = true;
        Ok(&mut self.state)
    }
}

fn execute_empty_cutover(root: &Path) -> Result<CutoverOutcome, CutoverError> {
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root)?;
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    orchestrator.execute(
        &plan,
        &CutoverConfirmationInput::confirmed_interactive(),
        &SequenceIds,
    )
}

fn observer(birth: &str, pid: u32) -> ObserverProcessIdentity {
    ObserverProcessIdentity {
        pid,
        birth: birth.to_owned(),
        executable: "/private/wsnav-observer".to_owned(),
    }
}

fn make_handle(
    runtime_id: RuntimeId,
    old: &ObserverProcessIdentity,
    revision: Revision,
) -> OpenCodeRuntimeHandle {
    OpenCodeRuntimeHandle {
        runtime_id,
        runtime_generation: "generation-a".to_owned(),
        endpoint_host: "127.0.0.1".to_owned(),
        endpoint_port: 4321,
        version: "1.0".to_owned(),
        native_session_id: wsnav::domain::ProviderSessionId::new(
            ProviderKind::OpenCode,
            "native-session",
        )
        .expect("session identity"),
        observer_pid: Some(old.pid),
        observer_birth: Some(old.birth.clone()),
        observer_status: OpenCodeObserverStatus::Ready,
        revision,
    }
}

fn projection_from_handle(handle: OpenCodeRuntimeHandle) -> OpenCodeObserverProjection {
    OpenCodeObserverProjection {
        runtime: RuntimeRecord {
            runtime_id: handle.runtime_id,
            workstream_id: WorkstreamId::new(),
            provider: ProviderKind::OpenCode,
            tmux_generation: handle.runtime_generation.clone(),
            tmux_session: "private-runtime".to_owned(),
            cwd: PathBuf::from("/workspace/project"),
            provider_pid: Some(404),
            process_birth: Some("provider-birth".to_owned()),
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        },
        binding: ProviderBinding {
            runtime_id: handle.runtime_id,
            provider: ProviderKind::OpenCode,
            native_session_id: handle.native_session_id.clone(),
            start_source: "test".to_owned(),
            last_settled_turn_id: None,
            observed_thread_name: None,
            name_state: NameState::Unavailable,
            predecessor_native_session_id: None,
            predecessor_effective_name: None,
            runtime_generation: handle.runtime_generation.clone(),
            revision: Revision::INITIAL,
        },
        handle,
    }
}

fn open_lease(root: &Path) -> TransitionLease {
    let lock = root.join("transition.lock");
    File::create(&lock).expect("transition lock");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
    }
    acquire_transition_lease(root).expect("exclusive transition lease")
}

fn private_tempdir() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().expect("temporary root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("root mode");
    }
    temporary
}

#[test]
fn decline_has_no_lease_state_or_client_effects() {
    let temporary = private_tempdir();
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, temporary.path()).expect("empty plan");
    let mut presentation = EmptyPresentation {
        root: temporary.path().to_owned(),
    };
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let outcome = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::declined_interactive(),
            &SequenceIds,
        )
        .expect("decline is a no-op");
    assert_eq!(outcome, CutoverOutcome::Declined);
    assert!(!state_factory.opened);
    assert!(!temporary.path().join("transition.lock").exists());
}

#[test]
fn noninteractive_launch_provenance_cannot_authorize_mutation() {
    let temporary = private_tempdir();
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, temporary.path()).expect("empty plan");
    let mut presentation = EmptyPresentation {
        root: temporary.path().to_owned(),
    };
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let mut confirmation = CutoverConfirmationInput::confirmed_interactive();
    confirmation.launch_kind = wsnav::cutover::CutoverLaunchKind::Hook;
    let error = orchestrator
        .execute(&plan, &confirmation, &SequenceIds)
        .expect_err("hook launch cannot authorize");
    assert!(matches!(error, CutoverError::UnauthorizedLaunch));
    assert!(!state_factory.opened);
    assert!(!temporary.path().join("transition.lock").exists());
}

#[test]
fn attached_presentation_is_drain_only_without_state_open() {
    let temporary = private_tempdir();
    let attached = presentation_assessment(temporary.path(), &["/dev/pts/9"], 101);
    assert_eq!(
        attached.state(),
        wsnav::presentation::LegacyPresentationState::Attached
    );
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([attached]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, temporary.path()).expect("drain plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let outcome = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect("drain is a successful no-op");
    assert_eq!(
        outcome,
        CutoverOutcome::DrainOnly(wsnav::presentation::LegacyPresentationState::Attached)
    );
    assert!(!state_factory.opened);
    assert_eq!(presentation.retire_calls, 0);
    assert!(!temporary.path().join("transition.lock").exists());
}

#[cfg(unix)]
#[test]
fn root_mode_must_be_exactly_private_owner_directory() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = private_tempdir();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o750))
        .expect("relaxed root mode");
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::new(),
        retire_calls: 0,
    };
    let error = discover_cutover(&mut presentation, temporary.path())
        .expect_err("non-exact root mode must refuse");
    assert!(matches!(error, CutoverError::InvalidRoot));
}

#[test]
fn presentation_proof_keeps_operator_root_spelling_separate_from_lease_root() {
    let temporary = private_tempdir();
    let spelling = temporary.path().join(".");
    let none = {
        let mut source = LivePresentationProofSource;
        source.prove(&spelling).expect("none assessment")
    };
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([none]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, &spelling).expect("ready plan");
    assert_eq!(plan.presentation_root(), spelling.as_path());
    assert_eq!(
        plan.root(),
        fs::canonicalize(&spelling)
            .expect("canonical root")
            .as_path()
    );
}

#[test]
fn changed_under_lease_proof_refuses_before_retirement() {
    let temporary = private_tempdir();
    let first = presentation_assessment(temporary.path(), &[], 101);
    let changed = presentation_assessment(temporary.path(), &[], 999);
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([first, changed]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, temporary.path()).expect("ready plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("changed proof must refuse");
    assert!(matches!(error, CutoverError::PresentationProofChanged));
    assert_eq!(presentation.retire_calls, 0);
    assert!(!state_factory.opened);
    assert!(!temporary.path().join("transition.lock").exists());
}

#[test]
fn preexisting_transition_lock_is_retained_on_pre_state_refusal() {
    let temporary = private_tempdir();
    let root = temporary.path();
    let lock = root.join("transition.lock");
    File::create(&lock).expect("preexisting transition lock");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).expect("lock mode");
    }
    let first = presentation_assessment(root, &[], 101);
    let changed = presentation_assessment(root, &[], 999);
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([first, changed]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, root).expect("ready plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("changed proof must refuse");
    assert!(matches!(error, CutoverError::PresentationProofChanged));
    assert!(lock.exists(), "preexisting recovery lock must remain");
    assert!(!state_factory.opened);
}

#[test]
fn preexisting_locked_transition_lock_is_never_removed_on_acquisition_failure() {
    let temporary = private_tempdir();
    let root = temporary.path();
    let held = open_lease(root);
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("held preexisting lock must refuse");
    assert!(matches!(error, CutoverError::State(_)));
    assert!(root.join("transition.lock").exists());
    drop(held);
    assert!(!state_factory.opened);
}

#[test]
fn retirement_repeats_exact_proof_until_none() {
    let temporary = private_tempdir();
    let detached = presentation_assessment(temporary.path(), &[], 101);
    let none = {
        let mut source = LivePresentationProofSource;
        source.prove(temporary.path()).expect("none assessment")
    };
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([
            detached.clone(),
            detached.clone(),
            detached,
            none.clone(),
            none.clone(),
            none,
        ]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, temporary.path()).expect("ready plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let outcome = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect("retirement repeats");
    assert!(matches!(outcome, CutoverOutcome::Completed(_)));
    assert_eq!(presentation.retire_calls, 2);
}

#[test]
fn retirement_effect_keeps_new_lock_as_recovery_evidence_on_late_refusal() {
    let temporary = private_tempdir();
    let root = temporary.path();
    let detached = presentation_assessment(root, &[], 101);
    let changed = presentation_assessment(root, &[], 999);
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([detached.clone(), detached, changed]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, root).expect("ready plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("proof change after retirement must refuse");
    assert!(matches!(error, CutoverError::PresentationProofChanged));
    assert!(root.join("transition.lock").exists());
    assert_eq!(presentation.retire_calls, 1);
}

#[test]
fn presentation_reappearance_after_handover_keeps_client_files_intact() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        let path = root.join(name);
        File::create(&path).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("client mode");
        }
    }
    let detached = presentation_assessment(root, &[], 101);
    let reappeared = presentation_assessment(root, &[], 303);
    let none = {
        let mut source = LivePresentationProofSource;
        source.prove(root).expect("none assessment")
    };
    let mut presentation = ScriptedPresentation {
        assessments: VecDeque::from([detached.clone(), detached, none.clone(), none, reappeared]),
        retire_calls: 0,
    };
    let plan = discover_cutover(&mut presentation, root).expect("ready plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory::default();
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("reappearance must refuse before cleanup");
    assert!(matches!(error, CutoverError::PresentationNotRetired));
    assert!(state_factory.opened);
    assert!(root.join("client.sqlite").exists());
    assert!(root.join("client.sqlite-wal").exists());
    assert!(root.join("client.sqlite-shm").exists());
}

#[test]
fn exact_client_cleanup_is_idempotent_after_partial_deletion() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(name))
            .expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))
                .expect("client mode");
        }
    }
    fs::remove_file(root.join("client.sqlite")).expect("partial prior deletion");
    let outcome = execute_empty_cutover(root).expect("retry cleanup");
    assert!(matches!(
        outcome,
        CutoverOutcome::Completed(report) if report.removed_client_files == 2
    ));
    let outcome = execute_empty_cutover(root).expect("idempotent retry");
    assert!(matches!(
        outcome,
        CutoverOutcome::Completed(report) if report.removed_client_files == 0
    ));
    assert!(!root.join("client.sqlite-wal").exists());
    assert!(!root.join("client.sqlite-shm").exists());
}

#[test]
fn final_lock_removal_requires_the_exact_leased_inode() {
    let temporary = private_tempdir();
    let root = temporary.path();
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory {
        state: StateFake {
            replace_lock_on_migrate: true,
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("replacement lock must not be removed");
    assert!(matches!(
        error,
        CutoverError::LegacyClientArtifact(wsnav::cutover::LegacyClientArtifactReason::Changed)
    ));
    assert!(root.join("transition.lock").exists());
}

#[test]
fn missing_lock_before_client_cleanup_refuses_without_deleting_artifacts() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        let path = root.join(name);
        File::create(&path).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("client mode");
        }
    }
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory {
        state: StateFake {
            remove_lock_before_cleanup: true,
            root: Some(root.to_owned()),
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("missing mutation lease must refuse");
    assert!(matches!(
        error,
        CutoverError::LegacyClientArtifact(wsnav::cutover::LegacyClientArtifactReason::Changed)
    ));
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        assert!(root.join(name).exists(), "{name} must remain intact");
    }
    assert!(!state_factory.state.migration_called);
}

#[test]
fn nonregular_client_artifact_refuses_without_deleting_siblings() {
    let temporary = private_tempdir();
    let root = temporary.path();
    fs::create_dir(root.join("client.sqlite")).expect("nonregular artifact");
    File::create(root.join("client.sqlite-wal")).expect("sibling artifact");
    let error = execute_empty_cutover(root).expect_err("directory must refuse");
    assert!(matches!(error, CutoverError::LegacyClientArtifact(_)));
    assert!(root.join("client.sqlite-wal").exists());
}

#[cfg(unix)]
#[test]
fn nonprivate_client_artifact_refuses_as_foreign_boundary() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = private_tempdir();
    let root = temporary.path();
    File::create(root.join("client.sqlite")).expect("client artifact");
    fs::set_permissions(
        root.join("client.sqlite"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("nonprivate artifact");
    let error = execute_empty_cutover(root).expect_err("nonprivate must refuse");
    assert!(matches!(
        error,
        CutoverError::LegacyClientArtifact(wsnav::cutover::LegacyClientArtifactReason::NonPrivate)
    ));
    assert!(root.join("client.sqlite").exists());
}

#[cfg(unix)]
#[test]
fn symlink_client_artifact_is_never_followed_or_removed() {
    use std::os::unix::fs::symlink;

    let temporary = private_tempdir();
    let root = temporary.path();
    let target = root.join("outside");
    File::create(&target).expect("outside target");
    symlink(&target, root.join("client.sqlite")).expect("symlink artifact");
    let error = execute_empty_cutover(root).expect_err("symlink must refuse");
    assert!(matches!(
        error,
        CutoverError::LegacyClientArtifact(wsnav::cutover::LegacyClientArtifactReason::Symlink)
    ));
    assert!(root.join("client.sqlite").exists());
    assert!(target.exists());
}

#[test]
fn pre_d16_handover_orders_freeze_cas_activation_and_termination() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        File::create(root.join(name)).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))
                .expect("client mode");
        }
    }
    let runtime_id = RuntimeId::from(Uuid::from_u128(2));
    let old = observer("old", 101);
    let standby = observer("standby", 202);
    let revision = Revision::INITIAL;
    let handle = make_handle(runtime_id, &old, revision);
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory {
        state: StateFake {
            observers: vec![handle],
            current: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: old.pid,
                birth: old.birth.clone(),
                revision,
            }),
            swapped: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: standby.pid,
                birth: standby.birth.clone(),
                revision: revision.next(),
            }),
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    // The process fake's deterministic standby identity is the same one the
    // state fixture expects.
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let result = orchestrator.execute(
        &plan,
        &CutoverConfirmationInput::confirmed_interactive(),
        &SequenceIds,
    );
    assert!(result.is_ok(), "handover result: {result:?}");
    assert!(state_factory.state.migration_called);
    assert!(state_factory.state.migration_saw_no_client_files);
    assert_eq!(
        process.calls,
        vec![
            "corroborate",
            "observe",
            "start-standby",
            "observe",
            "freeze",
            "observe",
            "observe",
            "activate",
            "observe",
            "terminate",
        ]
    );
}

#[test]
fn handover_failure_keeps_client_files_and_journal_for_restart() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        File::create(root.join(name)).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))
                .expect("client mode");
        }
    }
    let runtime_id = RuntimeId::from(Uuid::from_u128(4));
    let old = observer("old", 101);
    let standby = observer("standby", 202);
    let handle = make_handle(runtime_id, &old, Revision::INITIAL);
    let mut process = NoopProcess {
        fail_at: Some("activate"),
        ..NoopProcess::default()
    };
    let mut state_factory = StateFactory {
        state: StateFake {
            observers: vec![handle],
            current: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: old.pid,
                birth: old.birth.clone(),
                revision: Revision::INITIAL,
            }),
            swapped: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: standby.pid,
                birth: standby.birth.clone(),
                revision: Revision::INITIAL.next(),
            }),
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    assert!(
        orchestrator
            .execute(
                &plan,
                &CutoverConfirmationInput::confirmed_interactive(),
                &SequenceIds,
            )
            .is_err()
    );
    assert!(!state_factory.state.migration_called);
    assert!(root.join("client.sqlite").exists());
    assert!(root.join("d16-observer-handover.json").exists());
    assert!(!process.calls.contains(&"terminate"));
}

#[test]
fn fuzzy_process_identity_signals_nothing_and_preserves_client_files() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        File::create(root.join(name)).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))
                .expect("client mode");
        }
    }
    let runtime_id = RuntimeId::from(Uuid::from_u128(3));
    let old = observer("old", 101);
    let mut process = NoopProcess {
        wrong_observation: true,
        ..NoopProcess::default()
    };
    let handle = make_handle(runtime_id, &old, Revision::INITIAL);
    let mut state_factory = StateFactory {
        state: StateFake {
            observers: vec![handle],
            current: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: old.pid,
                birth: old.birth.clone(),
                revision: Revision::INITIAL,
            }),
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("fuzzy identity must refuse");
    assert!(matches!(error, CutoverError::FuzzyProcessIdentity));
    assert!(!process.calls.contains(&"freeze"));
    assert!(!process.calls.contains(&"terminate"));
    assert!(!state_factory.state.migration_called);
    assert!(root.join("client.sqlite").exists());
}

#[test]
fn process_corroboration_owns_d16_kind_not_persisted_state() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        let path = root.join(name);
        File::create(&path).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("client mode");
        }
    }
    let runtime_id = RuntimeId::from(Uuid::from_u128(30));
    let old = observer("old", 501);
    let handle = make_handle(runtime_id, &old, Revision::INITIAL);
    let mut process = NoopProcess {
        corroborated_kind: OpenCodeObserverKind::D16,
        ..NoopProcess::default()
    };
    let mut state_factory = StateFactory {
        state: StateFake {
            observers: vec![handle],
            current: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: old.pid,
                birth: old.birth.clone(),
                revision: Revision::INITIAL,
            }),
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let result = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect("D16 observer is revalidated in place");
    assert!(matches!(result, CutoverOutcome::Completed(_)));
    assert_eq!(process.calls, vec!["corroborate", "observe"]);
    assert!(!process.calls.contains(&"freeze"));
    assert!(state_factory.state.migration_called);
}

#[test]
fn fuzzy_process_corroboration_signals_nothing() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        let path = root.join(name);
        File::create(&path).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("client mode");
        }
    }
    let runtime_id = RuntimeId::from(Uuid::from_u128(31));
    let old = observer("old", 511);
    let handle = make_handle(runtime_id, &old, Revision::INITIAL);
    let mut process = NoopProcess {
        corroboration_error: Some(CutoverError::FuzzyProcessIdentity),
        ..NoopProcess::default()
    };
    let mut state_factory = StateFactory {
        state: StateFake {
            observers: vec![handle],
            current: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: old.pid,
                birth: old.birth.clone(),
                revision: Revision::INITIAL,
            }),
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let mut orchestrator =
        CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
    let error = orchestrator
        .execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
        .expect_err("fuzzy argv/process proof must refuse");
    assert!(matches!(error, CutoverError::FuzzyProcessIdentity));
    assert_eq!(process.calls, vec!["corroborate"]);
    assert!(!process.calls.contains(&"freeze"));
    assert!(!state_factory.state.migration_called);
    assert!(root.join("client.sqlite").exists());
}

#[test]
fn cas_failure_after_freeze_preserves_old_frozen_journal() {
    let temporary = private_tempdir();
    let root = temporary.path();
    for name in ["client.sqlite", "client.sqlite-wal", "client.sqlite-shm"] {
        let path = root.join(name);
        File::create(&path).expect("client artifact");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("client mode");
        }
    }
    let runtime_id = RuntimeId::from(Uuid::from_u128(22));
    let old = observer("old", 421);
    let handle = make_handle(runtime_id, &old, Revision::INITIAL);
    let mut process = NoopProcess::default();
    let mut state_factory = StateFactory {
        state: StateFake {
            observers: vec![handle],
            current: Some(CurrentObserverHandleProof {
                runtime_id,
                runtime_generation: "generation-a".to_owned(),
                pid: old.pid,
                birth: old.birth.clone(),
                revision: Revision::INITIAL,
            }),
            swapped: None,
            ..StateFake::default()
        },
        ..StateFactory::default()
    };
    let mut presentation = EmptyPresentation {
        root: root.to_owned(),
    };
    let mut planning = LivePresentationProofSource;
    let plan = discover_cutover(&mut planning, root).expect("empty plan");
    let result = {
        let mut orchestrator =
            CutoverOrchestrator::new(&mut presentation, &mut process, &mut state_factory);
        orchestrator.execute(
            &plan,
            &CutoverConfirmationInput::confirmed_interactive(),
            &SequenceIds,
        )
    };
    assert!(result.is_err());
    let lease = open_lease(root);
    let journal = wsnav::state::recover_observer_handover_journal(&lease)
        .expect("recover journal")
        .expect("journal retained");
    assert_eq!(journal.phase, HandoverPhase::OldFrozen);
    assert!(root.join("client.sqlite").exists());
}
