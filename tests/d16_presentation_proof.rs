use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;
use wsnav::domain::WorkstreamId;
use wsnav::presentation::{
    AttachmentPhase, LegacyAttachmentStatusForTest, LegacyFileIdentity,
    LegacyPresentationEvidenceForTest, LegacyPresentationPaneEvidenceForTest,
    LegacyPresentationProof, LegacyPresentationState, PresentationError, PresentationPaneRole,
    PresentationPaths, classify_legacy_evidence, classify_legacy_presentations,
    legacy_presentation_config_for_test, remove_dead_legacy_presentation,
};
use wsnav::state::{TransitionLease, acquire_transition_lease};

fn identity() -> LegacyFileIdentity {
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
    reason = "The deterministic fixture names every independent pane/process/topology field explicitly."
)]
fn pane(
    role: PresentationPaneRole,
    id: &str,
    pid: Option<u32>,
    process_pid: Option<u32>,
    birth: Option<u64>,
    stable: bool,
    arguments: &[&str],
    left: u16,
    top: u16,
    width: u16,
    height: u16,
) -> LegacyPresentationPaneEvidenceForTest {
    LegacyPresentationPaneEvidenceForTest {
        id: id.to_owned(),
        role,
        dead: false,
        pid,
        process_pid,
        birth,
        process_stable: stable,
        executable_path: Some(PathBuf::from("/workspace/wsnav")),
        executable_identity: Some(identity()),
        arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
        left,
        top,
        width,
        height,
        window_width: 128,
        window_height: 24,
    }
}

fn evidence(
    temporary: &TempDir,
    provider_arguments: &[&str],
    utility: bool,
    clients: &[&str],
) -> (PresentationPaths, LegacyPresentationEvidenceForTest) {
    let paths = PresentationPaths::fresh(temporary.path());
    let root = temporary.path().to_str().unwrap();
    let navigator_arguments = [
        "/workspace/wsnav",
        "--state-root",
        root,
        "_navigator",
        "--presentation-socket",
        paths.socket.to_str().unwrap(),
        "--presentation-session",
        paths.session_name.as_str(),
    ];
    let navigator = pane(
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
    let provider = pane(
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
        if utility { 11 } else { 24 },
    );
    let mut panes = vec![navigator, provider];
    if utility {
        panes.push(pane(
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
        ));
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

fn classify(
    temporary: &TempDir,
    evidence: LegacyPresentationEvidenceForTest,
    paths: &PresentationPaths,
) -> LegacyPresentationState {
    classify_legacy_evidence(&paths.directory, temporary.path(), evidence).state()
}

fn dead_presentation_fixture(temporary: &TempDir) -> (PresentationPaths, LegacyPresentationProof) {
    private_mode(temporary.path(), 0o700);
    let presentation_root = temporary.path().join("presentation");
    fs::create_dir(&presentation_root).unwrap();
    private_mode(&presentation_root, 0o700);
    let paths = PresentationPaths::fresh(temporary.path());
    fs::create_dir(&paths.directory).unwrap();
    private_mode(&paths.directory, 0o700);
    fs::write(&paths.config, legacy_presentation_config_for_test()).unwrap();
    private_mode(&paths.config, 0o600);
    let assessment = classify_legacy_presentations(temporary.path()).unwrap();
    assert_eq!(assessment.state(), LegacyPresentationState::DeadOwned);
    (paths, assessment.proof().unwrap().clone())
}

fn transition_lease(temporary: &TempDir) -> TransitionLease {
    let lock = temporary.path().join("transition.lock");
    fs::write(&lock, b"").unwrap();
    private_mode(&lock, 0o600);
    acquire_transition_lease(temporary.path()).unwrap()
}

#[test]
fn ordinary_detached_proof_is_exact_and_reusable() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, evidence) = evidence(&temporary, &provider, false, &[]);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(
        assessment.state(),
        LegacyPresentationState::DetachedOrdinary
    );
    assert_eq!(assessment.proof().unwrap().navigator_pid(), Some(101));
    assert_eq!(
        assessment.proof().unwrap().navigator_process_birth(),
        Some(11)
    );
}

#[test]
fn attached_proof_is_refused_without_losing_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, evidence) = evidence(&temporary, &provider, false, &["/dev/pts/9"]);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Attached);
    assert_eq!(assessment.proof().unwrap().attached_client_count(), 1);
}

#[test]
fn attached_dead_pane_and_shell_claim_stay_refused() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, mut evidence) = evidence(&temporary, &provider, false, &["/dev/pts/9"]);
    evidence.panes[0].dead = true;
    evidence.shell_claim_present = true;
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Attached);
    assert_eq!(assessment.proof().unwrap().attached_client_count(), 1);
}

#[test]
fn utility_shell_is_a_distinct_refusal() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, evidence) = evidence(&temporary, &provider, true, &[]);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::UtilityShell);
    assert!(assessment.proof().unwrap().utility_present());
}

#[test]
fn observer_review_requires_the_exact_native_command() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_observer_review",
    ];
    let (paths, evidence) = evidence(&temporary, &provider, false, &[]);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::ObserverReview);
    assert!(assessment.proof().unwrap().observer_review_present());
}

#[test]
fn observer_review_requires_stable_provider_process_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_observer_review",
    ];
    let (paths, mut evidence) = evidence(&temporary, &provider, false, &[]);
    evidence.panes[1].process_stable = false;
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Foreign);
}

#[test]
fn arbitrary_provider_process_is_never_detached_ordinary() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_unrecognized",
    ];
    let (paths, evidence) = evidence(&temporary, &provider, false, &[]);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Foreign);
}

fn exact_attach_arguments(state_root: &Path, socket: &Path, session: &str) -> Vec<String> {
    [
        "/workspace/wsnav".to_owned(),
        "--state-root".to_owned(),
        state_root.to_str().unwrap().to_owned(),
        "_provider_attach".to_owned(),
        "01234567-89ab-cdef-0123-456789abcdef".to_owned(),
        "--presentation-socket".to_owned(),
        socket.to_str().unwrap().to_owned(),
        "--presentation-session".to_owned(),
        session.to_owned(),
        "--attempt-id".to_owned(),
        "fedcba98-7654-3210-fedc-ba9876543210".to_owned(),
    ]
    .to_vec()
}

#[test]
fn exact_provider_attach_arguments_prove_detached_ordinary() {
    let temporary = tempfile::tempdir().unwrap();
    let (paths, mut evidence) = evidence(&temporary, &[], false, &[]);
    evidence.panes[1].arguments =
        exact_attach_arguments(temporary.path(), &paths.socket, &paths.session_name);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(
        assessment.state(),
        LegacyPresentationState::DetachedOrdinary
    );
}

#[test]
fn fuzzy_provider_attach_arguments_are_foreign() {
    let temporary = tempfile::tempdir().unwrap();
    let (paths, mut evidence) = evidence(&temporary, &[], false, &[]);
    let foreign_socket = temporary.path().join("foreign.sock");
    evidence.panes[1].arguments =
        exact_attach_arguments(temporary.path(), &foreign_socket, &paths.session_name);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Foreign);
}

#[test]
fn pid_birth_and_executable_mismatch_are_foreign() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, mut evidence) = evidence(&temporary, &provider, false, &[]);
    evidence.panes[0].process_pid = Some(999);
    assert_eq!(
        classify(&temporary, evidence.clone(), &paths),
        LegacyPresentationState::Foreign
    );
    evidence.panes[0].process_pid = None;
    evidence.panes[0].process_stable = false;
    assert_eq!(
        classify(&temporary, evidence.clone(), &paths),
        LegacyPresentationState::Foreign
    );
    evidence.panes[0].process_stable = true;
    evidence.panes[0].executable_identity = Some(LegacyFileIdentity {
        inode: 999,
        ..identity()
    });
    assert_eq!(
        classify(&temporary, evidence, &paths),
        LegacyPresentationState::Foreign
    );
}

#[test]
fn legacy_executable_identity_is_established_from_navigator() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, evidence) = evidence(&temporary, &provider, false, &[]);
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(
        assessment.state(),
        LegacyPresentationState::DetachedOrdinary
    );
    assert_eq!(
        assessment.proof().unwrap().legacy_executable_identity(),
        Some(identity())
    );
}

#[test]
fn navigator_and_provider_executable_identity_mismatch_is_foreign() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, mut evidence) = evidence(&temporary, &provider, false, &[]);
    evidence.panes[1].executable_identity = Some(LegacyFileIdentity {
        inode: 999,
        ..identity()
    });
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Foreign);
}

#[test]
fn malformed_topology_is_not_proven() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, mut evidence) = evidence(&temporary, &provider, false, &[]);
    evidence.panes.pop();
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Malformed);
    assert!(assessment.proof().is_none());
}

#[test]
fn dead_owned_requires_the_exact_private_config_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let provider = [
        "/workspace/wsnav",
        "--state-root",
        temporary.path().to_str().unwrap(),
        "_provider_wait",
    ];
    let (paths, mut evidence) = evidence(&temporary, &provider, false, &[]);
    evidence.session_id = None;
    evidence.config_identity = Some(LegacyFileIdentity {
        size: 1,
        digest: Some([0; 32]),
        ..identity()
    });
    let assessment = classify_legacy_evidence(&paths.directory, temporary.path(), evidence);
    assert_eq!(assessment.state(), LegacyPresentationState::Foreign);
    assert!(assessment.proof().is_none());
}

#[cfg(unix)]
fn private_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn private_mode(_path: &Path, _mode: u32) {}

#[test]
fn multiple_presentation_directories_refuse_even_when_dead() {
    let temporary = tempfile::tempdir().unwrap();
    private_mode(temporary.path(), 0o700);
    let root = temporary.path().join("presentation");
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    private_mode(&root, 0o700);
    let first = root.join("presentation-0123456789ab");
    let second = root.join("presentation-abcdefabcdef");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    #[cfg(unix)]
    {
        private_mode(&first, 0o700);
        private_mode(&second, 0o700);
    }
    let result = classify_legacy_presentations(temporary.path());
    assert!(matches!(
        result,
        Err(PresentationError::AmbiguousLegacyPresentations)
    ));
    assert!(first.exists());
    assert!(second.exists());
}

#[test]
fn dead_owned_classification_makes_no_remove_or_close_call() {
    let temporary = tempfile::tempdir().unwrap();
    private_mode(temporary.path(), 0o700);
    let root = temporary.path().join("presentation");
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    private_mode(&root, 0o700);
    let directory = root.join("presentation-0123456789ab");
    fs::create_dir(&directory).unwrap();
    #[cfg(unix)]
    private_mode(&directory, 0o700);
    let config = directory.join("tmux.conf");
    fs::write(&config, legacy_presentation_config_for_test()).unwrap();
    #[cfg(unix)]
    private_mode(&config, 0o600);
    let assessment = classify_legacy_presentations(temporary.path()).unwrap();
    assert_eq!(assessment.state(), LegacyPresentationState::DeadOwned);
    assert!(directory.exists());
    assert!(config.exists());
}

#[test]
fn dead_owned_cleanup_removes_only_exact_artifacts_and_preserves_runtime_sibling() {
    let temporary = tempfile::tempdir().unwrap();
    let (paths, proof) = dead_presentation_fixture(&temporary);
    let runtime_sibling = temporary.path().join("runtime.sock");
    fs::write(&runtime_sibling, "runtime server placeholder").unwrap();
    let lease = transition_lease(&temporary);

    remove_dead_legacy_presentation(temporary.path(), &proof, &lease).unwrap();

    assert!(!paths.directory.exists());
    assert!(runtime_sibling.exists());
}

#[test]
fn dead_owned_cleanup_refuses_unknown_entry_without_removing_known_files() {
    let temporary = tempfile::tempdir().unwrap();
    let (paths, proof) = dead_presentation_fixture(&temporary);
    let unknown = paths.directory.join("unexpected-artifact");
    fs::write(&unknown, "must survive").unwrap();
    let lease = transition_lease(&temporary);

    let result = remove_dead_legacy_presentation(temporary.path(), &proof, &lease);
    assert!(matches!(result, Err(PresentationError::LegacyProofChanged)));
    assert!(paths.directory.exists());
    assert!(paths.config.exists());
    assert!(unknown.exists());
}

#[test]
fn dead_owned_cleanup_requires_a_root_matching_transition_lease() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let (paths, proof) = dead_presentation_fixture(&first);
    private_mode(second.path(), 0o700);
    let lease = transition_lease(&second);

    let result = remove_dead_legacy_presentation(first.path(), &proof, &lease);
    assert!(matches!(
        result,
        Err(PresentationError::LegacyMutationRefused(
            "transition lease root does not match presentation root"
        ))
    ));
    assert!(paths.directory.exists());
    assert!(paths.config.exists());
}

#[test]
fn dead_owned_cleanup_refuses_a_replaced_transition_lock_before_removal() {
    let temporary = tempfile::tempdir().unwrap();
    let (paths, proof) = dead_presentation_fixture(&temporary);
    let lease = transition_lease(&temporary);
    let lock = temporary.path().join("transition.lock");
    fs::remove_file(&lock).unwrap();
    fs::write(&lock, b"replacement").unwrap();
    private_mode(&lock, 0o600);

    let result = remove_dead_legacy_presentation(temporary.path(), &proof, &lease);
    assert!(matches!(
        result,
        Err(PresentationError::LegacyMutationRefused(
            "transition lease is no longer valid for presentation mutation"
        ))
    ));
    assert!(paths.directory.exists());
    assert!(paths.config.exists());
}

#[test]
fn dead_owned_cleanup_is_idempotent_after_partial_known_artifact_disappearance() {
    let temporary = tempfile::tempdir().unwrap();
    let (paths, _) = dead_presentation_fixture(&temporary);
    let status = LegacyAttachmentStatusForTest {
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
    private_mode(&paths.attachment_status, 0o600);
    let assessment = classify_legacy_presentations(temporary.path()).unwrap();
    let proof = assessment.proof().unwrap().clone();
    fs::remove_file(&paths.attachment_status).unwrap();

    let after_partial = classify_legacy_presentations(temporary.path()).unwrap();
    assert_eq!(after_partial.state(), LegacyPresentationState::DeadOwned);
    let lease = transition_lease(&temporary);
    remove_dead_legacy_presentation(temporary.path(), &proof, &lease).unwrap();
    remove_dead_legacy_presentation(temporary.path(), &proof, &lease).unwrap();
    assert!(!paths.directory.exists());
}
