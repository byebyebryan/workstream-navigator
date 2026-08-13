use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::domain::{
    BindingId, DomainError, HostId, IdGenerator, LocationId, OperationId, OperationKind,
    OperationPhase, ProjectId, ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeStatus,
    WorkstreamId, WorkstreamLifecycle, WorkstreamOrigin,
};
use crate::protocol::{Capabilities, HelloResponse};
use crate::provider::codex::profile::{OBSERVER_PROFILE_SCHEMA_VERSION, ProfileOwnership};
use crate::provider::lifecycle::{LifecycleEvent, LifecycleHint, LifecycleObservation};
use crate::provider::names::NameState;

use super::models::{
    OPENCODE_SESSION_CREATION_CLEANUP_UNKNOWN_CODE, OPENCODE_SESSION_CREATION_UNKNOWN_CODE,
};
use super::schema::MAX_NAVIGATOR_WORKSTREAMS;
use super::schema::{CLIENT_SCHEMA_SQL, CLIENT_SCHEMA_VERSION, HOST_SCHEMA_SQL};
use super::{
    ClientCatalog, ClientHostTransport, EXTERNAL_EFFECT_UNKNOWN_CODE, HOST_SCHEMA_VERSION,
    HostIdentity, HostRegistry, IntegrationLifecycle, OpenCodeLifecycleObservation,
    OpenCodeObserverStatus, OpenCodeRuntimeHandle, RuntimeRecord, StateError, StateRoot,
};

#[derive(Default)]
struct SequenceIds(AtomicU64);

impl IdGenerator for SequenceIds {
    fn uuid(&self) -> Uuid {
        Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed) + 1))
    }
}

fn registry() -> (tempfile::TempDir, HostRegistry) {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    (temporary, registry)
}

#[test]
fn project_browser_lists_only_safe_directories_without_exposing_root_paths() {
    let (temporary, mut registry) = registry();
    let browser_root = temporary.path().join("projects");
    let git_project = browser_root.join("navigator");
    let ordinary_directory = browser_root.join("scratch");
    fs::create_dir_all(git_project.join(".git")).unwrap();
    fs::create_dir_all(&ordinary_directory).unwrap();
    fs::create_dir_all(browser_root.join(".hidden-project")).unwrap();
    fs::write(browser_root.join("not-a-directory"), b"ignored").unwrap();
    registry
        .set_project_browser_root(&browser_root.to_string_lossy())
        .unwrap();

    let directories = registry.project_directories("").unwrap();

    assert_eq!(directories.relative_path, "");
    assert_eq!(directories.root_label, "custom root · projects");
    assert!(
        !directories
            .root_label
            .contains(&temporary.path().to_string_lossy().to_string())
    );
    assert_eq!(
        directories
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.is_git_repository))
            .collect::<Vec<_>>(),
        vec![("navigator", true), ("scratch", false)]
    );
    assert!(matches!(
        registry.project_browser_directory("../outside"),
        Err(StateError::InvalidProjectBrowserRelativePath)
    ));
    assert_eq!(
        registry.project_browser_directory("navigator").unwrap(),
        fs::canonicalize(git_project).unwrap()
    );
}

fn settled_runtime(registry: &mut HostRegistry, workstream_id: WorkstreamId) -> RuntimeRecord {
    let initial = registry.reserve_runtime(workstream_id).unwrap();
    let cwd = initial.cwd.to_string_lossy().into_owned();
    registry
        .record_runtime_process_identity(initial.runtime_id, initial.revision, 101, "birth-a")
        .unwrap();
    for event in [
        LifecycleObservation {
            event: LifecycleEvent::SessionStart,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: None,
            source: Some("startup".to_owned()),
        },
        LifecycleObservation {
            event: LifecycleEvent::UserPromptSubmit,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: None,
            source: None,
        },
        LifecycleObservation {
            event: LifecycleEvent::Stop,
            cwd,
            native_session_id: "session-a".to_owned(),
            turn_id: Some("settled-a".to_owned()),
            source: None,
        },
    ] {
        let runtime = registry
            .runtime_for_workstream(workstream_id)
            .unwrap()
            .unwrap();
        registry
            .apply_lifecycle_observation(runtime.runtime_id, &runtime.tmux_generation, event)
            .unwrap();
    }
    registry
        .runtime_for_workstream(workstream_id)
        .unwrap()
        .unwrap()
}

#[test]
fn codex_resume_rotates_binding_generation_only_after_exact_session_start() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let initial = settled_runtime(&mut registry, registered.workstream_id);
    let old_generation = initial.tmux_generation.clone();
    let old_binding = registry
        .binding_for_runtime(initial.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(old_binding.runtime_generation, old_generation);
    assert_eq!(
        registry
            .retained_codex_binding_for_runtime(initial.runtime_id)
            .unwrap(),
        Some(old_binding.clone())
    );

    registry
        .park_runtime(initial.runtime_id, initial.revision)
        .unwrap();
    let replacement = registry.reserve_runtime(registered.workstream_id).unwrap();
    assert_ne!(replacement.tmux_generation, old_generation);
    assert!(matches!(
        registry.binding_for_runtime(replacement.runtime_id),
        Err(StateError::HookEvidenceMismatch)
    ));
    assert!(matches!(
        registry.retained_codex_binding_for_runtime(replacement.runtime_id),
        Err(StateError::HookEvidenceMismatch)
    ));

    registry
        .apply_lifecycle_observation(
            replacement.runtime_id,
            &replacement.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: replacement.cwd.to_string_lossy().into_owned(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("resume".to_owned()),
            },
        )
        .unwrap();
    let rebound = registry
        .binding_for_runtime(replacement.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(rebound.native_session_id.native_id(), "session-a");
    assert_eq!(rebound.runtime_generation, replacement.tmux_generation);
    assert!(rebound.revision > old_binding.revision);
}

#[test]
fn parked_unstarted_codex_replacement_keeps_exact_resume_history_visible() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let initial = settled_runtime(&mut registry, registered.workstream_id);
    let old_generation = initial.tmux_generation.clone();
    let old_binding = registry
        .binding_for_runtime(initial.runtime_id)
        .unwrap()
        .unwrap();

    registry
        .park_runtime(initial.runtime_id, initial.revision)
        .unwrap();
    let replacement = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .park_runtime(replacement.runtime_id, replacement.revision)
        .unwrap();

    assert!(matches!(
        registry.binding_for_runtime(replacement.runtime_id),
        Err(StateError::HookEvidenceMismatch)
    ));
    assert_eq!(
        registry
            .retained_codex_binding_for_runtime(replacement.runtime_id)
            .unwrap()
            .unwrap(),
        old_binding
    );

    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::Parked);
    assert_eq!(overview.runtime.unwrap().status, RuntimeStatus::Stopped);
    let binding = overview.binding.unwrap();
    assert_eq!(binding.native_session_id, old_binding.native_session_id);
    assert_eq!(binding.runtime_generation, old_generation);

    let stale = LifecycleObservation {
        event: LifecycleEvent::UserPromptSubmit,
        cwd: PathBuf::from("/disposable/repository")
            .to_string_lossy()
            .into_owned(),
        native_session_id: "session-a".to_owned(),
        turn_id: None,
        source: None,
    };
    assert!(matches!(
        registry.apply_lifecycle_observation(replacement.runtime_id, &old_generation, stale,),
        Err(StateError::HookEvidenceMismatch)
    ));
}

#[test]
fn recovery_required_unknown_codex_replacement_keeps_exact_resume_history_visible() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let initial = settled_runtime(&mut registry, registered.workstream_id);
    let old_binding = registry
        .binding_for_runtime(initial.runtime_id)
        .unwrap()
        .unwrap();

    registry
        .mark_runtime_recovery_required(initial.runtime_id, initial.revision)
        .unwrap();
    let replacement = registry
        .reserve_runtime_recovery(registered.workstream_id)
        .unwrap();
    registry
        .mark_runtime_recovery_required(replacement.runtime_id, replacement.revision)
        .unwrap();

    assert_eq!(
        registry
            .retained_codex_binding_for_runtime(replacement.runtime_id)
            .unwrap()
            .unwrap()
            .native_session_id,
        old_binding.native_session_id
    );
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
    assert_eq!(overview.runtime.unwrap().status, RuntimeStatus::Unknown);
    assert_eq!(overview.binding.unwrap(), old_binding);
}

#[test]
fn retained_codex_binding_path_rejects_opencode_runtimes() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();

    assert!(matches!(
        registry.retained_codex_binding_for_runtime(runtime.runtime_id),
        Err(StateError::ProviderIdentityMismatch)
    ));
}

#[test]
fn codex_recovery_session_start_rebinds_generation_and_reopens_workstream() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let initial = settled_runtime(&mut registry, registered.workstream_id);
    let old_generation = initial.tmux_generation.clone();
    registry
        .mark_runtime_recovery_required(initial.runtime_id, initial.revision)
        .unwrap();
    let replacement = registry
        .reserve_runtime_recovery(registered.workstream_id)
        .unwrap();
    assert_ne!(replacement.tmux_generation, old_generation);

    registry
        .apply_lifecycle_observation(
            replacement.runtime_id,
            &replacement.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: replacement.cwd.to_string_lossy().into_owned(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("resume".to_owned()),
            },
        )
        .unwrap();
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::Open);
    assert_eq!(overview.runtime.unwrap().status, RuntimeStatus::Idle);
    assert_eq!(
        overview.binding.unwrap().runtime_generation,
        replacement.tmux_generation
    );
    assert_eq!(
        overview.attention.unwrap().recovery_unseen_since_revision,
        None
    );
}

#[test]
fn stale_codex_generation_cannot_update_lifecycle_or_settle_a_turn() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let initial = settled_runtime(&mut registry, registered.workstream_id);
    let old_generation = initial.tmux_generation.clone();
    registry
        .park_runtime(initial.runtime_id, initial.revision)
        .unwrap();
    let replacement = registry.reserve_runtime(registered.workstream_id).unwrap();

    let cwd = replacement.cwd.to_string_lossy().into_owned();
    let prompt = LifecycleObservation {
        event: LifecycleEvent::UserPromptSubmit,
        cwd: cwd.clone(),
        native_session_id: "session-a".to_owned(),
        turn_id: None,
        source: None,
    };
    assert!(matches!(
        registry.apply_lifecycle_observation(
            replacement.runtime_id,
            &replacement.tmux_generation,
            prompt
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
    let stop = LifecycleObservation {
        event: LifecycleEvent::Stop,
        cwd,
        native_session_id: "session-a".to_owned(),
        turn_id: Some("stale-turn".to_owned()),
        source: None,
    };
    assert!(matches!(
        registry.apply_lifecycle_observation(replacement.runtime_id, &old_generation, stop),
        Err(StateError::HookEvidenceMismatch)
    ));
    let current = registry
        .runtime_by_id(replacement.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(current.status, RuntimeStatus::Starting);
    assert!(
        registry
            .binding_for_runtime(replacement.runtime_id)
            .is_err()
    );
}

#[test]
fn newly_reserved_runtime_records_the_complete_private_session_identity() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();

    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();

    assert_eq!(
        runtime.tmux_session,
        format!("wsnav-{}", runtime.runtime_id)
    );
    assert_eq!(runtime.provider_pid, None);
}

#[test]
fn provider_process_identity_persists_as_a_pair_and_reservation_clears_it() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();

    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 4321, "birth-a")
        .unwrap();
    let persisted = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    assert_eq!(persisted.provider_pid, Some(4321));
    assert_eq!(persisted.process_birth.as_deref(), Some("birth-a"));
    registry
        .mark_runtime_recovery_required(runtime.runtime_id, persisted.revision)
        .unwrap();
    let replacement = registry
        .reserve_runtime_recovery(registered.workstream_id)
        .unwrap();
    assert_eq!(replacement.provider_pid, None);
    assert_eq!(replacement.process_birth, None);
}

#[test]
fn provider_pid_backfill_requires_the_exact_live_birth_and_revision() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .connection
        .execute(
            "UPDATE runtimes SET process_birth = 'birth-a' WHERE runtime_id = ?1",
            [runtime.runtime_id.to_string()],
        )
        .unwrap();

    registry
        .backfill_runtime_provider_pid(runtime.runtime_id, runtime.revision, 4321, "birth-a")
        .unwrap();
    let persisted = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();
    assert_eq!(persisted.provider_pid, Some(4321));
    assert_eq!(persisted.revision, runtime.revision.next());

    assert!(matches!(
        registry.backfill_runtime_provider_pid(
            runtime.runtime_id,
            runtime.revision,
            4322,
            "birth-a",
        ),
        Err(StateError::ConcurrentWrite)
    ));
}

#[test]
fn hook_candidates_are_limited_to_live_process_fingerprinted_runtimes() {
    let (_temporary, mut registry) = registry();
    let first = registry
        .register_external_workstream(
            PathBuf::from("/disposable/first"),
            "first-repository".to_owned(),
            "first-commit".to_owned(),
        )
        .unwrap();
    let second = registry
        .register_external_workstream(
            PathBuf::from("/disposable/second"),
            "second-repository".to_owned(),
            "second-commit".to_owned(),
        )
        .unwrap();
    let first_runtime = registry.reserve_runtime(first.workstream_id).unwrap();
    let second_runtime = registry.reserve_runtime(second.workstream_id).unwrap();
    registry
        .record_runtime_process_identity(
            first_runtime.runtime_id,
            first_runtime.revision,
            101,
            "first-birth",
        )
        .unwrap();

    let candidates = registry.hook_runtime_candidates().unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].runtime_id, first_runtime.runtime_id);
    assert_ne!(candidates[0].runtime_id, second_runtime.runtime_id);
}

#[test]
fn request_key_deduplicates_an_ambiguous_fork() {
    let (_temporary, mut registry) = registry();
    let (first, inserted_first) = registry
        .create_or_get_operation("fork-1".to_owned(), OperationKind::Fork, "{}".to_owned())
        .unwrap();
    let transitioned = registry
        .transition_operation(
            first.id,
            first.revision,
            OperationPhase::ExternalEffectStarted,
            Some("before-provider-call".to_owned()),
            None,
        )
        .unwrap();
    let (second, inserted_second) = registry
        .create_or_get_operation("fork-1".to_owned(), OperationKind::Fork, "{}".to_owned())
        .unwrap();

    assert!(inserted_first);
    assert!(!inserted_second);
    assert_eq!(second.id, first.id);
    assert_eq!(second.phase, transitioned.phase);
}

#[test]
fn stale_operation_revision_cannot_commit() {
    let (_temporary, mut registry) = registry();
    let (operation, _) = registry
        .create_or_get_operation("start-1".to_owned(), OperationKind::Start, "{}".to_owned())
        .unwrap();
    let transitioned = registry
        .transition_operation(
            operation.id,
            operation.revision,
            OperationPhase::ExternalEffectStarted,
            None,
            None,
        )
        .unwrap();

    assert!(matches!(
        registry.transition_operation(
            operation.id,
            operation.revision,
            OperationPhase::Committed,
            None,
            Some("{}".to_owned()),
        ),
        Err(StateError::Domain(DomainError::RevisionConflict { .. }))
    ));
    assert_eq!(transitioned.phase, OperationPhase::ExternalEffectStarted);
}

#[test]
fn opencode_session_creation_journals_boundary_and_commits_exact_binding() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();
    assert_eq!(prepared.operation.kind, OperationKind::Start);
    assert_eq!(prepared.operation.phase, OperationPhase::Prepared);
    assert!(
        registry
            .has_unresolved_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap(),
        "an uncommitted Prepared operation still needs cleanup evidence"
    );
    assert_eq!(
        registry
            .opencode_session_creation_for_runtime(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap(),
        Some(prepared.clone())
    );

    let started = registry.begin_opencode_session_creation(&prepared).unwrap();
    assert_eq!(
        started.operation.phase,
        OperationPhase::ExternalEffectStarted
    );
    assert!(
        registry
            .has_unresolved_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap()
    );

    let session = ProviderSessionId::new(ProviderKind::OpenCode, "created-session").unwrap();
    let committed = registry
        .commit_opencode_session_creation(&started, &session)
        .unwrap();
    assert_eq!(committed.operation.phase, OperationPhase::Committed);
    assert_eq!(committed.native_session_id, Some(session.clone()));
    assert!(
        !registry
            .has_unresolved_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap()
    );
    let binding = registry
        .binding_for_runtime(runtime.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(binding.native_session_id, session);
    assert_eq!(binding.runtime_generation, runtime.tmux_generation);
}

#[test]
fn opencode_session_creation_pre_effect_failure_is_terminal_and_bounded() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();
    let failed = registry
        .fail_opencode_session_creation(&prepared, "serve_timeout")
        .unwrap();
    assert_eq!(failed.operation.phase, OperationPhase::Failed);
    assert!(
        !registry
            .has_unresolved_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap()
    );
    let replay = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();
    assert_eq!(replay, failed);
    assert!(
        failed
            .operation
            .outcome_json
            .as_deref()
            .is_some_and(|value| value.contains("serve_timeout"))
    );
}

#[test]
fn opencode_session_creation_cleanup_failure_is_recovery_required_and_not_retryable() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();
    let unknown = registry
        .mark_opencode_session_creation_cleanup_unknown(&prepared)
        .unwrap();
    assert_eq!(unknown.operation.phase, OperationPhase::Failed);
    assert!(
            unknown.operation.outcome_json.as_deref().is_some_and(
                |outcome| outcome.contains(OPENCODE_SESSION_CREATION_CLEANUP_UNKNOWN_CODE)
            )
        );
    let current_runtime = registry
        .runtime_for_workstream(registered.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(current_runtime.status, RuntimeStatus::Unknown);
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
    assert!(matches!(
        registry.prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation),
        Err(StateError::HookEvidenceMismatch)
    ));
}

#[test]
fn opencode_session_creation_unknown_effect_is_recovery_required_and_not_retryable() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();
    let started = registry.begin_opencode_session_creation(&prepared).unwrap();
    let unknown = registry
        .mark_opencode_session_creation_unknown(&started)
        .unwrap();
    assert_eq!(
        unknown.operation.phase,
        OperationPhase::Failed,
        "an unknown provider effect is terminal and cannot be retried"
    );
    assert!(
        unknown
            .operation
            .outcome_json
            .as_deref()
            .is_some_and(|outcome| outcome.contains(OPENCODE_SESSION_CREATION_UNKNOWN_CODE))
    );
    assert!(
        !registry
            .has_unresolved_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap()
    );
    let current_runtime = registry
        .runtime_for_workstream(registered.workstream_id)
        .unwrap()
        .unwrap();
    assert_eq!(current_runtime.status, RuntimeStatus::Unknown);
    let overview = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
    assert!(
        overview
            .attention
            .as_ref()
            .and_then(|attention| attention.recovery_unseen_since_revision)
            .is_some()
    );
    assert!(matches!(
        registry.mark_opencode_session_creation_unknown(&started),
        Err(StateError::Domain(DomainError::RevisionConflict { .. }))
    ));
    assert_eq!(
        registry
            .opencode_session_creation_for_runtime(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap(),
        Some(unknown)
    );
}

#[test]
fn opencode_session_creation_rejects_stale_generation_and_operation_revision() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    assert!(matches!(
        registry.prepare_opencode_session_creation(runtime.runtime_id, "stale-generation"),
        Err(StateError::HookEvidenceMismatch)
    ));
    let prepared = registry
        .prepare_opencode_session_creation(runtime.runtime_id, &runtime.tmux_generation)
        .unwrap();
    let started = registry.begin_opencode_session_creation(&prepared).unwrap();
    assert!(matches!(
        registry.begin_opencode_session_creation(&prepared),
        Err(StateError::Domain(DomainError::RevisionConflict { .. }))
    ));
    let mut forged = started.clone();
    forged.runtime_generation = "forged-generation".to_owned();
    assert!(matches!(
        registry.commit_opencode_session_creation(
            &forged,
            &ProviderSessionId::new(ProviderKind::OpenCode, "created-session").unwrap(),
        ),
        Err(StateError::OpenCodeSessionCreationUnavailable)
    ));
}

#[test]
fn result_attention_stays_unseen_until_the_current_revision_acknowledges_it() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::Codex)
        .unwrap();
    let workstream_id = registered.workstream_id;
    let first = registry
        .mark_result_attention(
            workstream_id,
            ProviderSessionId::codex("session-a").unwrap(),
            "turn-a".to_owned(),
        )
        .unwrap();
    let second = registry
        .mark_result_attention(
            workstream_id,
            ProviderSessionId::codex("session-a").unwrap(),
            "turn-b".to_owned(),
        )
        .unwrap();

    assert_eq!(
        first.result_unseen_since_revision,
        second.result_unseen_since_revision
    );
    assert!(matches!(
        registry.acknowledge_result_attention(workstream_id, first.revision),
        Err(StateError::Domain(DomainError::RevisionConflict { .. }))
    ));
    let acknowledged = registry
        .acknowledge_result_attention(workstream_id, second.revision)
        .unwrap();
    assert_eq!(acknowledged.result_unseen_since_revision, None);
}

#[test]
fn independent_workstream_can_select_a_different_provider_and_replay_rejects_it() {
    let (_temporary, mut registry) = registry();
    let source = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::Codex)
        .unwrap();
    let created = registry
        .create_independent_workstream(
            "independent-opencode",
            source.workstream_id,
            Revision::INITIAL,
            ProviderKind::OpenCode,
        )
        .unwrap();

    assert_eq!(created.provider, ProviderKind::OpenCode);
    assert_eq!(
        registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == created.workstream_id)
            .unwrap()
            .provider,
        ProviderKind::OpenCode
    );
    assert!(matches!(
        registry.create_independent_workstream(
            "independent-opencode",
            source.workstream_id,
            Revision::INITIAL,
            ProviderKind::Codex,
        ),
        Err(StateError::OperationRequestMismatch)
    ));
}

#[test]
fn archive_and_restore_change_only_visibility_with_revision_guards() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir".to_owned(),
            "main".to_owned(),
        )
        .unwrap();
    let attention = registry
        .mark_result_attention(
            registered.workstream_id,
            ProviderSessionId::codex("native-session").unwrap(),
            "settled-turn".to_owned(),
        )
        .unwrap();
    let before = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();

    let archived_revision = registry
        .archive_workstream(registered.workstream_id, before.revision, 1_234)
        .unwrap();
    let archived = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(archived.archived_at_millis, Some(1_234));
    assert_eq!(archived.lifecycle, before.lifecycle);
    assert_eq!(
        archived.project_repository_path,
        before.project_repository_path
    );
    assert_eq!(archived.attention, before.attention);
    assert_eq!(archived.revision, archived_revision);
    assert!(matches!(
        registry.reserve_runtime(registered.workstream_id),
        Err(StateError::WorkstreamArchived(id)) if id == registered.workstream_id
    ));
    let independent = registry
        .create_independent_workstream(
            "archived-location-start",
            registered.workstream_id,
            archived_revision,
            ProviderKind::Codex,
        )
        .unwrap();
    assert_eq!(independent.source_workstream_id, registered.workstream_id);
    assert!(matches!(
        registry.restore_workstream(registered.workstream_id, before.revision),
        Err(StateError::Domain(DomainError::RevisionConflict { .. }))
    ));

    let restored_revision = registry
        .restore_workstream(registered.workstream_id, archived_revision)
        .unwrap();
    let restored = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap();
    assert_eq!(restored.archived_at_millis, None);
    assert_eq!(restored.lifecycle, before.lifecycle);
    assert_eq!(restored.attention, Some(attention));
    assert_eq!(restored.revision, restored_revision);
    assert!(matches!(
        registry.restore_workstream(registered.workstream_id, restored_revision),
        Err(StateError::WorkstreamNotArchived(id)) if id == registered.workstream_id
    ));
}

#[test]
fn state_files_are_private_on_unix() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let _registry = HostRegistry::open(&root).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(temporary.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.host_database_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn client_catalog_uses_its_own_schema() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let catalog = ClientCatalog::open(&root).unwrap();

    assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
}

#[test]
fn client_catalog_groups_a_location_with_an_explicit_project_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    let host = registry.identity().unwrap();
    let location_id = LocationId::new();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let recorded = catalog
        .register_local_project_location(&host, location_id, Path::new("/workspace/wsnav"), "wsnav")
        .unwrap();
    let loaded = catalog
        .local_project_location(host.host_id, location_id)
        .unwrap()
        .unwrap();

    assert_eq!(loaded, recorded);
}

#[test]
fn client_catalog_can_forget_project_locations_without_touching_host_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    let host = registry.identity().unwrap();
    let location_id = LocationId::new();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let project = catalog
        .register_local_project_location(&host, location_id, Path::new("/workspace/wsnav"), "wsnav")
        .unwrap();

    assert_eq!(
        catalog
            .ignore_project_locations(project.project_id)
            .unwrap(),
        1
    );
    assert!(
        catalog
            .project_location_is_ignored(host.host_id, location_id)
            .unwrap()
    );
    assert!(
        !catalog
            .project_location_is_ignored(host.host_id, LocationId::new())
            .unwrap()
    );
    assert!(
        catalog
            .local_project_location(host.host_id, location_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn client_catalog_groups_matching_repository_fingerprints_across_hosts() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    let local_host = registry.identity().unwrap();
    let remote_host = HostIdentity {
        host_id: HostId::new(),
        registry_generation: "remote-generation".to_owned(),
    };
    let local_location = LocationId::new();
    let remote_location = LocationId::new();
    let fingerprint = format!("git-remote-v1:{}", "a".repeat(64));
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let local = catalog
        .register_local_project_location_with_identity(
            &local_host,
            local_location,
            Path::new("/workspace/wsnav"),
            "cubey",
            Some(&fingerprint),
        )
        .unwrap();
    catalog
        .register_ssh_host(
            "snap",
            &remote_host,
            Path::new("/bin/wsnav"),
            "snap",
            Capabilities::default(),
        )
        .unwrap();

    let remote = catalog
        .register_host_project_location(
            remote_host.host_id,
            remote_location,
            "different-checkout-name",
            Some(&fingerprint),
        )
        .unwrap();

    assert_eq!(remote.project_id, local.project_id);
    assert_eq!(remote.display_name, "cubey");
}

#[test]
fn client_catalog_keeps_missing_and_distinct_fingerprints_separate() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    let host = registry.identity().unwrap();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let first = catalog
        .register_local_project_location_with_identity(
            &host,
            LocationId::new(),
            Path::new("/workspace/wsnav"),
            "first",
            Some(&format!("git-remote-v1:{}", "a".repeat(64))),
        )
        .unwrap();
    let second = catalog
        .register_local_project_location_with_identity(
            &host,
            LocationId::new(),
            Path::new("/workspace/wsnav"),
            "second",
            Some(&format!("git-remote-v1:{}", "b".repeat(64))),
        )
        .unwrap();
    let absent = catalog
        .register_local_project_location(
            &host,
            LocationId::new(),
            Path::new("/workspace/wsnav"),
            "absent",
        )
        .unwrap();

    assert_ne!(first.project_id, second.project_id);
    assert_ne!(first.project_id, absent.project_id);
    assert_ne!(second.project_id, absent.project_id);
}

#[test]
fn repository_metadata_backfill_refreshes_a_single_location_label() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    let host = registry.identity().unwrap();
    let location_id = LocationId::new();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let legacy = catalog
        .register_local_project_location(
            &host,
            location_id,
            Path::new("/bin/wsnav"),
            "cubey-worktree1",
        )
        .unwrap();
    let fingerprint = format!("git-remote-v1:{}", "e".repeat(64));

    let refreshed = catalog
        .register_local_project_location_with_identity(
            &host,
            location_id,
            Path::new("/bin/wsnav"),
            "cubey",
            Some(&fingerprint),
        )
        .unwrap();

    assert_eq!(refreshed.project_id, legacy.project_id);
    assert_eq!(refreshed.display_name, "cubey");
}

#[test]
fn legacy_metadata_schema_requires_an_explicit_reset() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 4;")
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::HostStateResetRequired(4))
    ));
}

#[test]
fn host_schema_eight_migrates_to_the_project_browser_schema() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    drop(registry);
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch(
            "DROP TABLE project_browser_settings;
                 DROP TABLE opencode_runtime_handles;
                 PRAGMA user_version = 8;
                 UPDATE host_identity SET schema_version = 8 WHERE singleton = 1;",
        )
        .unwrap();
    drop(connection);

    let mut registry = HostRegistry::open(&root).unwrap();

    assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    let provider_pid_column: String = registry
        .connection
        .query_row(
            "SELECT type FROM pragma_table_info('runtimes') WHERE name = 'provider_pid'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provider_pid_column, "INTEGER");
    registry
        .set_project_browser_root(&temporary.path().to_string_lossy())
        .unwrap();
    assert_eq!(
        registry.project_browser_root().unwrap(),
        fs::canonicalize(temporary.path()).unwrap()
    );
}

#[test]
fn host_schema_eleven_preserves_birth_for_exact_live_pid_backfill() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 4321, "birth-a")
        .unwrap();
    drop(registry);
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE runtimes DROP COLUMN provider_pid;
                 PRAGMA user_version = 11;
                 UPDATE host_identity SET schema_version = 11 WHERE singleton = 1;",
        )
        .unwrap();
    drop(connection);

    let registry = HostRegistry::open(&root).unwrap();
    let migrated = registry.runtime_by_id(runtime.runtime_id).unwrap().unwrap();

    assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    assert_eq!(migrated.provider_pid, None);
    assert_eq!(migrated.process_birth.as_deref(), Some("birth-a"));
}

fn legacy_host_schema_sql() -> String {
    let table_start = HOST_SCHEMA_SQL
        .find("    CREATE TABLE opencode_runtime_handles (")
        .unwrap();
    let table_end = HOST_SCHEMA_SQL[table_start..]
        .find("    CREATE TABLE provider_bindings (")
        .map(|offset| table_start + offset)
        .unwrap();
    format!(
            "{}{}",
            &HOST_SCHEMA_SQL[..table_start],
            &HOST_SCHEMA_SQL[table_end..]
        )
            .replace("        provider_pid INTEGER,\n", "")
            .replace(
                "        location_id TEXT NOT NULL REFERENCES project_locations(location_id),\n        provider TEXT NOT NULL,\n        origin",
                "        location_id TEXT NOT NULL REFERENCES project_locations(location_id),\n        origin",
            )
            .replace(
                "        runtime_id TEXT NOT NULL UNIQUE REFERENCES runtimes(runtime_id),\n        provider TEXT NOT NULL,\n        native_session_id",
                "        runtime_id TEXT NOT NULL UNIQUE REFERENCES runtimes(runtime_id),\n        native_session_id",
            )
            .replace(
                "        latest_native_session_id TEXT,\n        latest_native_session_provider TEXT,\n",
                "        latest_native_session_id TEXT,\n",
            )
}

#[test]
fn host_schema_nine_migrates_existing_codex_rows_with_explicit_provider_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let legacy_sql = legacy_host_schema_sql();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection.execute_batch(&legacy_sql).unwrap();
    let host_id = HostId::new();
    let location_id = LocationId::new();
    let workstream_id = WorkstreamId::new();
    let runtime_id = RuntimeId::new();
    connection
        .execute(
            "INSERT INTO host_identity VALUES (1, ?1, 'generation', 9)",
            [host_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO project_locations VALUES (?1, '/project', 'project', '', '', 1)",
            [location_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO workstreams VALUES (?1, ?2, 'external', NULL, 'open', NULL, 1, 0, 1)",
            params![workstream_id.to_string(), location_id.to_string()],
        )
        .unwrap();
    connection
            .execute(
                "INSERT INTO runtimes VALUES (?1, ?2, 'codex', 'generation', 'session', '/project', NULL, 'stopped', 1)",
                params![runtime_id.to_string(), workstream_id.to_string()],
            )
            .unwrap();
    connection
            .execute(
                "INSERT INTO provider_bindings VALUES (?1, ?2, 'session-id', 'startup', NULL, NULL, 'unavailable', NULL, NULL, NULL, 'generation', 1)",
                params![BindingId::new().to_string(), runtime_id.to_string()],
            )
            .unwrap();
    connection
        .execute_batch("PRAGMA user_version = 9;")
        .unwrap();
    drop(connection);

    let registry = HostRegistry::open(&root).unwrap();

    assert_eq!(registry.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.provider, ProviderKind::Codex);
    assert_eq!(overview.runtime.unwrap().provider, ProviderKind::Codex);
    assert_eq!(
        overview.binding.unwrap().native_session_id,
        ProviderSessionId::codex("session-id").unwrap()
    );
}

#[test]
fn host_schema_ten_rejects_conflicting_opencode_handle_table() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    drop(registry);
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch(
            "DROP TABLE opencode_runtime_handles;
                 CREATE TABLE opencode_runtime_handles (runtime_id TEXT PRIMARY KEY);
                 PRAGMA user_version = 10;
                 UPDATE host_identity SET schema_version = 10 WHERE singleton = 1;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::InvalidPersistedValue(_))
    ));
    let connection = Connection::open(root.host_database_path()).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 10);
}

#[test]
fn host_schema_ten_rejects_even_a_matching_preexisting_opencode_table() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    drop(registry);
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 10;
                 UPDATE host_identity SET schema_version = 10 WHERE singleton = 1;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::InvalidPersistedValue(_))
    ));
    let connection = Connection::open(root.host_database_path()).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        10
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT schema_version FROM host_identity WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        10
    );
}

#[test]
fn unknown_legacy_runtime_provider_rejects_and_rolls_back_schema_migration() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection.execute_batch(&legacy_host_schema_sql()).unwrap();
    let host_id = HostId::new();
    let location_id = LocationId::new();
    let workstream_id = WorkstreamId::new();
    let runtime_id = RuntimeId::new();
    connection
        .execute(
            "INSERT INTO host_identity VALUES (1, ?1, 'generation', 9)",
            [host_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO project_locations VALUES (?1, '/project', 'project', '', '', 1)",
            [location_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO workstreams VALUES (?1, ?2, 'external', NULL, 'open', NULL, 1, 0, 1)",
            params![workstream_id.to_string(), location_id.to_string()],
        )
        .unwrap();
    connection
            .execute(
                "INSERT INTO runtimes VALUES (?1, ?2, 'unknown', 'generation', 'session', '/project', NULL, 'stopped', 1)",
                params![runtime_id.to_string(), workstream_id.to_string()],
            )
            .unwrap();
    connection
        .execute_batch("PRAGMA user_version = 9;")
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::InvalidPersistedValue(_))
    ));
    let connection = Connection::open(root.host_database_path()).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 9);
    let host_schema_version: i64 = connection
        .query_row(
            "SELECT schema_version FROM host_identity WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(host_schema_version, 9);
    let has_provider_column: bool = connection
        .query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info('workstreams') WHERE name = 'provider'
                 )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!has_provider_column);
}

#[test]
fn fresh_schema_requires_explicit_provider_columns_and_persists_kind() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    for table in ["workstreams", "provider_bindings"] {
        let (not_null, default_value): (i64, Option<String>) = registry
                .connection
                .query_row(
                    &format!(
                        "SELECT \"notnull\", dflt_value FROM pragma_table_info('{table}') WHERE name = 'provider'"
                    ),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
        assert_eq!(not_null, 1);
        assert_eq!(default_value, None);
    }
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.provider, ProviderKind::OpenCode);
    let runtime = registry
        .reserve_runtime_with_provider(registered.workstream_id, ProviderKind::OpenCode)
        .unwrap();
    assert_eq!(runtime.provider, ProviderKind::OpenCode);
}

#[test]
fn opencode_handle_is_private_and_generation_bound() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry
        .reserve_runtime_with_provider(registered.workstream_id, ProviderKind::OpenCode)
        .unwrap();
    let session = ProviderSessionId::new(ProviderKind::OpenCode, "root-session").unwrap();
    registry
        .bind_opencode_session(
            runtime.runtime_id,
            &runtime.tmux_generation,
            &session,
            "new",
        )
        .unwrap();
    let handle = registry
        .record_opencode_runtime_handle(
            runtime.runtime_id,
            &runtime.tmux_generation,
            4321,
            "contract-build-a",
            &session,
        )
        .unwrap();
    assert_eq!(handle.endpoint_host, "127.0.0.1");
    assert_eq!(
        registry
            .opencode_runtime_handle(runtime.runtime_id)
            .unwrap(),
        Some(handle.clone())
    );
    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.provider, ProviderKind::OpenCode);
    // The loopback endpoint/observer row is separate from the bounded
    // Workstream overview projection; it is available only through the
    // host-private handle accessor above.
    assert!(matches!(
        registry.record_opencode_observer_started(
            runtime.runtime_id,
            "wrong-generation",
            handle.revision,
            77,
            "birth",
        ),
        Err(StateError::ConcurrentWrite)
    ));
    let starting = registry
        .record_opencode_observer_started(
            runtime.runtime_id,
            &runtime.tmux_generation,
            handle.revision,
            77,
            "birth",
        )
        .unwrap();
    registry
        .mark_opencode_observer_ready(
            runtime.runtime_id,
            &runtime.tmux_generation,
            starting.revision,
            77,
            "birth",
        )
        .unwrap();
    assert_eq!(
        registry
            .opencode_runtime_handle(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .observer_status,
        OpenCodeObserverStatus::Ready
    );
    let ready = registry
        .opencode_runtime_handle(runtime.runtime_id)
        .unwrap()
        .unwrap();
    registry
        .mark_opencode_observer_unknown_exact(
            runtime.runtime_id,
            &runtime.tmux_generation,
            ready.revision,
            ready.observer_pid.unwrap(),
            ready.observer_birth.as_deref().unwrap(),
        )
        .unwrap();
    assert_eq!(
        registry
            .opencode_runtime_handle(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .observer_status,
        OpenCodeObserverStatus::Unknown
    );
}

#[test]
fn opencode_observer_lifecycle_is_revision_guarded_and_bounded() {
    let (_temporary, mut registry, runtime, session, ready) = opencode_lifecycle_fixture();
    let cwd = PathBuf::from("/disposable/repository");
    let started = OpenCodeLifecycleObservation {
        generation: runtime.tmux_generation.clone(),
        cwd: cwd.clone(),
        runtime_revision: runtime.revision,
        session: session.clone(),
        observer_pid: 77,
        observer_birth: "observer-birth".to_owned(),
        hint: LifecycleHint::Started,
    };
    let next = registry
        .apply_opencode_lifecycle_observation(runtime.runtime_id, &started)
        .unwrap();
    assert_eq!(
        registry
            .runtime_by_id(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeStatus::Idle
    );
    assert!(matches!(
        registry.apply_opencode_lifecycle_observation(runtime.runtime_id, &started),
        Err(StateError::HookEvidenceMismatch)
    ));
    let working = OpenCodeLifecycleObservation {
        runtime_revision: next,
        hint: LifecycleHint::Working,
        ..started.clone()
    };
    let next = registry
        .apply_opencode_lifecycle_observation(runtime.runtime_id, &working)
        .unwrap();
    let working_activity = registry.workstream_overviews().unwrap()[0]
        .last_activity_at_millis
        .expect("first working observation establishes wall-clock activity");
    let uncorroborated = OpenCodeLifecycleObservation {
        runtime_revision: next,
        hint: LifecycleHint::Settled { message_id: None },
        ..working.clone()
    };
    assert!(matches!(
        registry.apply_opencode_lifecycle_observation(runtime.runtime_id, &uncorroborated),
        Err(StateError::HookEvidenceMismatch)
    ));
    assert_eq!(
        registry
            .runtime_by_id(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeStatus::Working
    );
    let settled = OpenCodeLifecycleObservation {
        runtime_revision: next,
        hint: LifecycleHint::Settled {
            message_id: Some("completed-message".to_owned()),
        },
        ..working.clone()
    };
    let next = registry
        .apply_opencode_lifecycle_observation(runtime.runtime_id, &settled)
        .unwrap();
    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert!(overview.last_activity_at_millis.unwrap() >= working_activity);
    assert_eq!(
        overview.binding.unwrap().last_settled_turn_id.as_deref(),
        Some("completed-message")
    );
    assert!(
        overview
            .attention
            .unwrap()
            .result_unseen_since_revision
            .is_some()
    );
    let ended = OpenCodeLifecycleObservation {
        runtime_revision: next,
        hint: LifecycleHint::Ended,
        ..settled
    };
    registry
        .apply_opencode_lifecycle_observation(runtime.runtime_id, &ended)
        .unwrap();
    assert_eq!(
        registry
            .runtime_by_id(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeStatus::Stopped
    );
    let latest_handle = registry
        .opencode_runtime_handle(runtime.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(latest_handle, ready);
}

fn opencode_recovery_lifecycle_fixture() -> (
    tempfile::TempDir,
    HostRegistry,
    RuntimeRecord,
    ProviderSessionId,
    Revision,
) {
    let (temporary, mut registry, runtime, session, _ready) = opencode_lifecycle_fixture();
    registry
        .mark_runtime_recovery_required(runtime.runtime_id, runtime.revision)
        .unwrap();
    let recovery = registry
        .reserve_runtime_recovery_with_provider(runtime.workstream_id, ProviderKind::OpenCode)
        .unwrap();
    registry
        .bind_opencode_session(
            recovery.runtime_id,
            &recovery.tmux_generation,
            &session,
            "resume",
        )
        .unwrap();
    let handle = registry
        .record_opencode_runtime_handle(
            recovery.runtime_id,
            &recovery.tmux_generation,
            4322,
            "contract-build-b",
            &session,
        )
        .unwrap();
    let starting = registry
        .record_opencode_observer_started(
            recovery.runtime_id,
            &recovery.tmux_generation,
            handle.revision,
            78,
            "observer-birth-2",
        )
        .unwrap();
    registry
        .mark_opencode_observer_ready(
            recovery.runtime_id,
            &recovery.tmux_generation,
            starting.revision,
            78,
            "observer-birth-2",
        )
        .unwrap();
    let runtime_revision = registry
        .runtime_by_id(recovery.runtime_id)
        .unwrap()
        .unwrap()
        .revision;
    (temporary, registry, recovery, session, runtime_revision)
}

#[test]
fn opencode_recovery_started_rejects_wrong_session_and_source() {
    let (_temporary, mut registry, recovery, session, runtime_revision) =
        opencode_recovery_lifecycle_fixture();
    let wrong_session = ProviderSessionId::new(ProviderKind::OpenCode, "other-session").unwrap();
    assert!(matches!(
        registry.apply_opencode_lifecycle_observation(
            recovery.runtime_id,
            &OpenCodeLifecycleObservation {
                generation: recovery.tmux_generation.clone(),
                cwd: recovery.cwd.clone(),
                runtime_revision,
                session: wrong_session,
                observer_pid: 78,
                observer_birth: "observer-birth-2".to_owned(),
                hint: LifecycleHint::Started,
            },
        ),
        Err(StateError::ProviderIdentityMismatch)
    ));
    registry
        .connection
        .execute(
            "UPDATE provider_bindings SET start_source = 'new' WHERE runtime_id = ?1",
            [recovery.runtime_id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        registry.apply_opencode_lifecycle_observation(
            recovery.runtime_id,
            &OpenCodeLifecycleObservation {
                generation: recovery.tmux_generation.clone(),
                cwd: recovery.cwd.clone(),
                runtime_revision,
                session: session.clone(),
                observer_pid: 78,
                observer_birth: "observer-birth-2".to_owned(),
                hint: LifecycleHint::Started,
            },
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
}

#[test]
fn opencode_recovery_started_reopens_workstream_and_clears_attention() {
    let (_temporary, mut registry, recovery, session, runtime_revision) =
        opencode_recovery_lifecycle_fixture();
    registry
        .apply_opencode_lifecycle_observation(
            recovery.runtime_id,
            &OpenCodeLifecycleObservation {
                generation: recovery.tmux_generation,
                cwd: recovery.cwd,
                runtime_revision,
                session,
                observer_pid: 78,
                observer_birth: "observer-birth-2".to_owned(),
                hint: LifecycleHint::Started,
            },
        )
        .unwrap();
    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::Open);
    assert_eq!(
        overview
            .attention
            .and_then(|attention| attention.recovery_unseen_since_revision),
        None
    );
}

#[test]
fn opencode_unknown_fork_effect_is_terminal_and_deduplicated() {
    let (_temporary, mut registry, runtime, session, _ready) = opencode_lifecycle_fixture();
    let cwd = PathBuf::from("/disposable/repository");
    let next = registry
        .apply_opencode_lifecycle_observation(
            runtime.runtime_id,
            &OpenCodeLifecycleObservation {
                generation: runtime.tmux_generation.clone(),
                cwd: cwd.clone(),
                runtime_revision: runtime.revision,
                session: session.clone(),
                observer_pid: 77,
                observer_birth: "observer-birth".to_owned(),
                hint: LifecycleHint::Started,
            },
        )
        .unwrap();
    let _settled_revision = registry
        .apply_opencode_lifecycle_observation(
            runtime.runtime_id,
            &OpenCodeLifecycleObservation {
                generation: runtime.tmux_generation,
                cwd,
                runtime_revision: next,
                session,
                observer_pid: 77,
                observer_birth: "observer-birth".to_owned(),
                hint: LifecycleHint::Settled {
                    message_id: Some("settled-message".to_owned()),
                },
            },
        )
        .unwrap();
    let source_before = registry.workstream_overviews().unwrap().remove(0);
    let prepared = registry
        .prepare_fork_with_provider(
            "opencode-unknown-fork".to_owned(),
            OperationKind::Fork,
            source_before.workstream_id,
            source_before.revision,
            ProviderKind::OpenCode,
        )
        .unwrap();
    let marked = registry.record_fork_attempt(&prepared.plan).unwrap();
    registry.mark_fork_external_effect_unknown(&marked).unwrap();
    let failed = registry.fork_plan(marked.operation.id).unwrap();
    assert_eq!(failed.operation.phase, OperationPhase::Failed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            failed.operation.outcome_json.as_deref().unwrap()
        )
        .unwrap()
        .get("code")
        .and_then(serde_json::Value::as_str),
        Some(EXTERNAL_EFFECT_UNKNOWN_CODE)
    );
    assert!(
        registry
            .unresolved_operation_overviews()
            .unwrap()
            .is_empty()
    );
    assert_eq!(registry.workstream_overviews().unwrap().len(), 1);
    let source_after = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == source_before.workstream_id)
        .unwrap();
    assert_eq!(source_after, source_before);
    registry.mark_fork_external_effect_unknown(&marked).unwrap();
    registry.mark_fork_external_effect_unknown(&failed).unwrap();
    let mut mutated = marked.clone();
    mutated.source_native_name = Some("different-source-name".to_owned());
    assert!(matches!(
        registry.mark_fork_external_effect_unknown(&mutated),
        Err(StateError::ForkOperationUnavailable)
    ));
}

fn opencode_lifecycle_fixture() -> (
    tempfile::TempDir,
    HostRegistry,
    RuntimeRecord,
    ProviderSessionId,
    OpenCodeRuntimeHandle,
) {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry
        .reserve_runtime_with_provider(registered.workstream_id, ProviderKind::OpenCode)
        .unwrap();
    let session = ProviderSessionId::new(ProviderKind::OpenCode, "root-session").unwrap();
    registry
        .bind_opencode_session(
            runtime.runtime_id,
            &runtime.tmux_generation,
            &session,
            "new",
        )
        .unwrap();
    let handle = registry
        .record_opencode_runtime_handle(
            runtime.runtime_id,
            &runtime.tmux_generation,
            4321,
            "contract-build-a",
            &session,
        )
        .unwrap();
    let starting = registry
        .record_opencode_observer_started(
            runtime.runtime_id,
            &runtime.tmux_generation,
            handle.revision,
            77,
            "observer-birth",
        )
        .unwrap();
    let ready = registry
        .mark_opencode_observer_ready(
            runtime.runtime_id,
            &runtime.tmux_generation,
            starting.revision,
            77,
            "observer-birth",
        )
        .unwrap();
    (temporary, registry, runtime, session, ready)
}

#[test]
fn mismatched_workstream_runtime_and_binding_provider_fails_closed() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .connection
        .execute(
            "UPDATE workstreams SET provider = 'opencode' WHERE workstream_id = ?1",
            [registered.workstream_id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        registry.runtime_for_workstream(registered.workstream_id),
        Err(StateError::ProviderIdentityMismatch)
    ));
    registry
        .connection
        .execute(
            "UPDATE workstreams SET provider = 'codex' WHERE workstream_id = ?1",
            [registered.workstream_id.to_string()],
        )
        .unwrap();
    registry
        .connection
        .execute(
            "INSERT INTO provider_bindings (
                    binding_id, runtime_id, provider, native_session_id, start_source,
                    last_settled_turn_id, observed_thread_name, name_state,
                    name_observed_at, predecessor_native_session_id,
                    predecessor_effective_name, runtime_generation, revision
                 ) VALUES (?1, ?2, 'opencode', 'same-id', 'startup', NULL, NULL,
                    'unavailable', NULL, NULL, NULL, ?3, 1)",
            params![
                BindingId::new().to_string(),
                runtime.runtime_id.to_string(),
                runtime.tmux_generation,
            ],
        )
        .unwrap();
    assert!(matches!(
        registry.binding_for_runtime(runtime.runtime_id),
        Err(StateError::ProviderIdentityMismatch)
    ));
    registry
        .connection
        .execute(
            "INSERT INTO attention_states (
                    workstream_id, result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_native_session_provider, latest_turn_id, revision
                 ) VALUES (?1, NULL, NULL, 'native-only', NULL, NULL, 1)",
            [registered.workstream_id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        registry.attention(registered.workstream_id),
        Err(StateError::Sqlite(_))
    ));
    registry
        .connection
        .execute(
            "UPDATE attention_states SET latest_native_session_id = NULL,
                    latest_native_session_provider = 'codex' WHERE workstream_id = ?1",
            [registered.workstream_id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        registry.attention(registered.workstream_id),
        Err(StateError::Sqlite(_))
    ));
}

#[test]
fn attention_provider_mismatch_fails_result_writes_and_overview_hydration() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::Codex)
        .unwrap();
    assert!(matches!(
        registry.mark_result_attention(
            registered.workstream_id,
            ProviderSessionId::new(ProviderKind::OpenCode, "same-native-id").unwrap(),
            "turn".to_owned(),
        ),
        Err(StateError::ProviderIdentityMismatch)
    ));
    assert!(matches!(
        registry.mark_result_attention(
            WorkstreamId::new(),
            ProviderSessionId::codex("nonexistent").unwrap(),
            "turn".to_owned(),
        ),
        Err(StateError::UnknownOpenWorkstream(_))
    ));

    registry
        .connection
        .execute(
            "INSERT INTO attention_states (
                    workstream_id, result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_native_session_provider, latest_turn_id, revision
                 ) VALUES (?1, NULL, NULL, 'same-native-id', 'opencode', 'turn', 1)",
            [registered.workstream_id.to_string()],
        )
        .unwrap();
    assert!(matches!(
        registry.workstream_overviews(),
        Err(StateError::ProviderIdentityMismatch)
    ));
}

#[test]
fn legacy_remote_metadata_schema_requires_an_explicit_reset() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 6;")
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::HostStateResetRequired(6))
    ));
}

#[test]
fn client_catalog_migrates_legacy_local_host_records() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.client_database_path()).unwrap();
    connection
            .execute_batch(
                "CREATE TABLE hosts (
                    host_alias TEXT PRIMARY KEY,
                    host_id TEXT NOT NULL UNIQUE,
                    executable_path TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 INSERT INTO hosts VALUES ('local', '00000000-0000-0000-0000-000000000001', '/bin/wsnav', 1);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
    drop(connection);

    let catalog = ClientCatalog::open(&root).unwrap();
    let migrated = catalog.host("local").unwrap().unwrap();

    assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
    assert_eq!(migrated.registry_generation, "");
    assert!(matches!(migrated.transport, ClientHostTransport::Local));
    assert_eq!(migrated.capabilities, Capabilities::default());
}

#[test]
fn client_catalog_migrates_d6_projects_without_losing_associations() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let legacy_sql = CLIENT_SCHEMA_SQL
            .replace("        repository_fingerprint TEXT,\n", "")
            .replace(
                "    CREATE UNIQUE INDEX project_location_identity_idx\n        ON project_locations(host_id, location_id);\n",
                "",
            )
            .replace(
                "    CREATE UNIQUE INDEX project_repository_fingerprint_idx\n        ON projects(repository_fingerprint)\n        WHERE repository_fingerprint IS NOT NULL;\n",
                "",
            );
    let connection = Connection::open(root.client_database_path()).unwrap();
    connection.execute_batch(&legacy_sql).unwrap();
    let host_id = HostId::new();
    let location_id = LocationId::new();
    let project_id = ProjectId::new();
    connection
        .execute(
            "INSERT INTO hosts VALUES (
                    'local', ?1, 'generation', '/bin/wsnav', 'local', NULL,
                    '{\"codex\":false,\"git\":false,\"tmux\":false}', 1
                 )",
            [host_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO projects VALUES (?1, 'cubey', 1)",
            [project_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO project_locations VALUES (?1, ?2, ?3)",
            params![
                project_id.to_string(),
                host_id.to_string(),
                location_id.to_string(),
            ],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA user_version = 2;")
        .unwrap();
    drop(connection);

    let mut catalog = ClientCatalog::open(&root).unwrap();
    let fingerprint = format!("git-remote-v1:{}", "d".repeat(64));
    let grouped = catalog
        .register_local_project_location_with_identity(
            &HostIdentity {
                host_id,
                registry_generation: "generation".to_owned(),
            },
            location_id,
            Path::new("/bin/wsnav"),
            "cubey",
            Some(&fingerprint),
        )
        .unwrap();

    assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
    assert_eq!(grouped.project_id, project_id);
}

#[test]
fn client_schema_four_migration_strips_codex_without_losing_catalog_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.client_database_path()).unwrap();
    connection.execute_batch(CLIENT_SCHEMA_SQL).unwrap();
    let host_id = HostId::new();
    let location_id = LocationId::new();
    let project_id = ProjectId::new();
    connection
        .execute(
            "INSERT INTO hosts VALUES (
                    'snap', ?1, 'generation', '/bin/wsnav', 'ssh', 'snap',
                    '{\"codex\":true,\"git\":true,\"tmux\":false}', 7
                 )",
            [host_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO projects VALUES (?1, 'project', NULL, 3)",
            [project_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO project_locations VALUES (?1, ?2, ?3)",
            params![
                project_id.to_string(),
                host_id.to_string(),
                location_id.to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO ignored_project_locations VALUES (?1, ?2)",
            params![host_id.to_string(), location_id.to_string()],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA user_version = 4;")
        .unwrap();
    drop(connection);

    let catalog = ClientCatalog::open(&root).unwrap();
    assert_eq!(catalog.schema_version().unwrap(), CLIENT_SCHEMA_VERSION);
    let host = catalog.host("snap").unwrap().unwrap();
    assert_eq!(host.revision, Revision::try_from(7).unwrap());
    assert_eq!(
        host.capabilities,
        Capabilities {
            git: true,
            tmux: false
        }
    );
    let persisted: String = catalog
        .connection
        .query_row(
            "SELECT capabilities_json FROM hosts WHERE host_alias = 'snap'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted, r#"{"git":true,"tmux":false}"#);
    assert!(!persisted.contains("codex"));
    assert_eq!(
        catalog
            .local_project_location(host_id, location_id)
            .unwrap()
            .unwrap()
            .project_id,
        project_id
    );
    assert!(
        catalog
            .project_location_is_ignored(host_id, location_id)
            .unwrap()
    );
}

#[test]
fn registered_ssh_host_refuses_identity_generation_and_capability_drift() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let identity = HostIdentity {
        host_id: HostId::new(),
        registry_generation: "generation-a".to_owned(),
    };
    let capabilities = Capabilities {
        git: true,
        tmux: true,
    };
    let registered = catalog
        .register_ssh_host(
            "snap",
            &identity,
            Path::new("/home/bryan/.local/bin/wsnav"),
            "snap",
            capabilities.clone(),
        )
        .unwrap();

    assert!(matches!(
        registered.transport,
        ClientHostTransport::Ssh { ref destination } if destination == "snap"
    ));
    let persisted: String = catalog
        .connection
        .query_row(
            "SELECT capabilities_json FROM hosts WHERE host_alias = 'snap'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!persisted.contains("codex"));
    assert_eq!(catalog.ssh_hosts().unwrap(), vec![registered.clone()]);
    assert_eq!(
        catalog
            .verify_hello(
                "snap",
                &HelloResponse {
                    host_id: identity.host_id,
                    registry_generation: identity.registry_generation.clone(),
                    wsnav_version: "0.1.0".to_owned(),
                    capabilities: capabilities.clone(),
                }
            )
            .unwrap(),
        registered
    );
    assert!(matches!(
        catalog.verify_hello(
            "snap",
            &HelloResponse {
                host_id: HostId::new(),
                registry_generation: identity.registry_generation.clone(),
                wsnav_version: "0.1.0".to_owned(),
                capabilities: capabilities.clone(),
            }
        ),
        Err(StateError::ClientHostIdentityMismatch)
    ));
    assert!(matches!(
        catalog.verify_hello(
            "snap",
            &HelloResponse {
                host_id: identity.host_id,
                registry_generation: "generation-b".to_owned(),
                wsnav_version: "0.1.0".to_owned(),
                capabilities: capabilities.clone(),
            }
        ),
        Err(StateError::ClientHostGenerationMismatch)
    ));
    assert!(matches!(
        catalog.verify_hello(
            "snap",
            &HelloResponse {
                host_id: identity.host_id,
                registry_generation: identity.registry_generation,
                wsnav_version: "0.1.0".to_owned(),
                capabilities: Capabilities::default(),
            }
        ),
        Err(StateError::ClientHostCapabilitiesMismatch)
    ));
}

#[test]
fn explicit_ssh_reset_removes_only_client_registration_and_associations() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let identity = HostIdentity {
        host_id: HostId::new(),
        registry_generation: "generation".to_owned(),
    };
    catalog
        .register_ssh_host(
            "snap",
            &identity,
            Path::new("/home/bryan/.local/bin/wsnav"),
            "snap",
            Capabilities::default(),
        )
        .unwrap();

    catalog.reset_ssh_host("snap").unwrap();

    assert!(catalog.host("snap").unwrap().is_none());
    assert!(catalog.ssh_hosts().unwrap().is_empty());
}

#[test]
fn fresh_registry_identity_is_stable_and_uses_the_injected_source() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let ids = SequenceIds::default();
    let first = HostRegistry::open_with_id_generator(&root, &ids).unwrap();
    let first_identity = first.identity().unwrap();
    let second = HostRegistry::open_with_id_generator(&root, &ids).unwrap();

    assert_eq!(first.schema_version().unwrap(), HOST_SCHEMA_VERSION);
    assert_eq!(first_identity.host_id, HostId::from(Uuid::from_u128(1)));
    assert_eq!(
        first_identity.registry_generation,
        Uuid::from_u128(2).to_string()
    );
    assert_eq!(second.identity().unwrap(), first_identity);
}

#[test]
fn future_schema_versions_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 99;")
        .unwrap();

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::UnsupportedSchemaVersion(99))
    ));
}

#[test]
fn v1_host_schema_requires_an_explicit_reset() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE host_identity (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    host_id TEXT NOT NULL UNIQUE,
                    registry_generation TEXT NOT NULL,
                    schema_version INTEGER NOT NULL
                 );
                 INSERT INTO host_identity VALUES (1, 'host', 'generation', 1);
                 CREATE TABLE workstreams (
                    workstream_id TEXT PRIMARY KEY,
                    location_id TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    source_workstream_id TEXT,
                    checkout_id TEXT NOT NULL UNIQUE,
                    lifecycle TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::HostStateResetRequired(1))
    ));
}

#[test]
fn pre_worktree_free_host_schema_requires_an_explicit_reset() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE host_identity (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    host_id TEXT NOT NULL UNIQUE,
                    registry_generation TEXT NOT NULL,
                    schema_version INTEGER NOT NULL
                 );
                 INSERT INTO host_identity VALUES (1, 'host', 'generation', 2);
                 CREATE TABLE workstreams (
                    workstream_id TEXT PRIMARY KEY,
                    location_id TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    source_workstream_id TEXT,
                    checkout_id TEXT NOT NULL UNIQUE,
                    lifecycle TEXT NOT NULL,
                    last_activity_sequence INTEGER NOT NULL CHECK (last_activity_sequence >= 0),
                    revision INTEGER NOT NULL CHECK (revision > 0)
                 );
                 PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::HostStateResetRequired(2))
    ));
}

#[test]
fn legacy_host_state_is_never_mutated_to_adopt_the_worktree_free_design() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection.execute_batch(HOST_SCHEMA_SQL).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 3;")
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::HostStateResetRequired(3))
    ));
}

#[test]
fn later_legacy_host_schema_also_requires_a_reset() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let legacy_sql = HOST_SCHEMA_SQL.replace("        archived_at_millis INTEGER,\n", "");
    let connection = Connection::open(root.host_database_path()).unwrap();
    connection.execute_batch(&legacy_sql).unwrap();
    connection
        .execute(
            "INSERT INTO host_identity VALUES (1, ?1, 'generation', 5)",
            [HostId::new().to_string()],
        )
        .unwrap();
    connection
        .execute_batch("PRAGMA user_version = 5;")
        .unwrap();
    drop(connection);

    assert!(matches!(
        HostRegistry::open(&root),
        Err(StateError::HostStateResetRequired(5))
    ));
}

#[test]
fn deterministic_operation_identity_is_persisted_on_first_request() {
    let (_temporary, mut registry) = registry();
    let ids = SequenceIds::default();
    let (operation, inserted) = registry
        .create_or_get_operation_with_id_generator(
            "deterministic-start".to_owned(),
            OperationKind::Start,
            "{}".to_owned(),
            &ids,
        )
        .unwrap();

    assert!(inserted);
    assert_eq!(operation.id, OperationId::from(Uuid::from_u128(1)));
}

#[test]
fn external_workstream_reserves_one_runtime_until_it_is_stopped() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let first = registry.reserve_runtime(registered.workstream_id).unwrap();

    assert_eq!(first.status, RuntimeStatus::Starting);
    assert!(matches!(
        registry.reserve_runtime(registered.workstream_id),
        Err(StateError::RuntimeAlreadyLive(id)) if id == registered.workstream_id
    ));
    registry
        .mark_runtime_stopped(first.runtime_id, first.revision)
        .unwrap();
    let resumed = registry.reserve_runtime(registered.workstream_id).unwrap();

    assert_eq!(resumed.runtime_id, first.runtime_id);
    assert_ne!(resumed.tmux_generation, first.tmux_generation);
    assert_eq!(resumed.status, RuntimeStatus::Starting);
}

#[test]
fn fresh_host_state_has_no_checkout_or_worktree_ownership_tables() {
    let (_temporary, registry) = registry();
    for table in ["checkouts", "managed_worktrees"] {
        let exists: bool = registry
            .connection
            .query_row(
                "SELECT EXISTS(
                        SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
                     )",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists, "unexpected retired ownership table {table}");
    }
}

#[test]
fn provider_fork_plan_is_durable_and_deduplicated_at_the_project_root() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    assert!(matches!(
        registry.prepare_fork(
            "independent-1".to_owned(),
            OperationKind::Start,
            registered.workstream_id,
            Revision::INITIAL,
        ),
        Err(StateError::InvalidForkPlanShape)
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn fork_requires_one_live_source_binding_with_a_settled_turn() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    assert!(matches!(
        registry.prepare_fork(
            "fork-no-boundary".to_owned(),
            OperationKind::Fork,
            registered.workstream_id,
            Revision::INITIAL,
        ),
        Err(StateError::ForkBoundaryUnavailable)
    ));

    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "source-session".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "source-session".to_owned(),
                turn_id: None,
                source: None,
            },
        )
        .unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::Stop,
                cwd,
                native_session_id: "source-session".to_owned(),
                turn_id: Some("settled-turn".to_owned()),
                source: None,
            },
        )
        .unwrap();
    let source_revision = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap()
        .revision;
    let prepared = registry
        .prepare_fork(
            "fork-settled".to_owned(),
            OperationKind::Fork,
            registered.workstream_id,
            source_revision,
        )
        .unwrap();

    assert_eq!(prepared.plan.origin, WorkstreamOrigin::Fork);
    assert_eq!(prepared.plan.source_runtime_id, Some(runtime.runtime_id));
    assert_eq!(
        prepared
            .plan
            .source_native_session_id
            .as_ref()
            .map(ProviderSessionId::native_id),
        Some("source-session")
    );
    assert_eq!(
        prepared.plan.last_settled_turn_id.as_deref(),
        Some("settled-turn")
    );
    let operations = registry.unresolved_operation_overviews().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].kind, OperationKind::Fork);
    assert_eq!(
        operations[0].source_workstream_id,
        Some(registered.workstream_id)
    );
    let created = registry
        .commit_fork(&prepared.plan, "destination-session")
        .unwrap();
    let destination_runtime = registry
        .runtime_for_workstream(created.workstream_id)
        .unwrap()
        .unwrap();
    let destination_binding = registry
        .binding_for_runtime(destination_runtime.runtime_id)
        .unwrap()
        .unwrap();

    assert_eq!(destination_runtime.status, RuntimeStatus::Stopped);
    assert_eq!(
        destination_runtime.cwd,
        PathBuf::from("/disposable/repository")
    );
    assert_eq!(destination_binding.start_source, "resume");
    assert_eq!(
        destination_binding.native_session_id.native_id(),
        "destination-session"
    );
}

#[test]
fn only_provider_forks_can_create_a_recoverable_operation() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    assert!(matches!(
        registry.prepare_fork(
            "independent-recovery".to_owned(),
            OperationKind::Start,
            registered.workstream_id,
            Revision::INITIAL,
        ),
        Err(StateError::InvalidForkPlanShape)
    ));
}

#[test]
fn fork_attempt_marker_is_atomic_and_prevents_a_second_provider_call() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "source-session".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::Stop,
                cwd,
                native_session_id: "source-session".to_owned(),
                turn_id: Some("settled-turn".to_owned()),
                source: None,
            },
        )
        .unwrap();
    let source_revision = registry
        .workstream_overviews()
        .unwrap()
        .into_iter()
        .find(|overview| overview.workstream_id == registered.workstream_id)
        .unwrap()
        .revision;
    let prepared = registry
        .prepare_fork(
            "fork-attempt".to_owned(),
            OperationKind::Fork,
            registered.workstream_id,
            source_revision,
        )
        .unwrap();

    let marked = registry.record_fork_attempt(&prepared.plan).unwrap();
    assert!(marked.fork_attempted_at_millis.is_some());
    assert!(matches!(
        registry.record_fork_attempt(&marked),
        Err(StateError::ForkOperationUnavailable)
    ));
}

#[test]
fn explicit_park_is_distinct_from_an_observed_runtime_stop() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let unexpected_stop = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .mark_runtime_stopped(unexpected_stop.runtime_id, unexpected_stop.revision)
        .unwrap();
    assert!(
        !registry
            .runtime_is_deliberately_parked(unexpected_stop.runtime_id, registered.workstream_id,)
            .unwrap()
    );

    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .park_runtime(runtime.runtime_id, runtime.revision)
        .unwrap();
    assert!(
        registry
            .runtime_is_deliberately_parked(runtime.runtime_id, registered.workstream_id)
            .unwrap()
    );
    assert_eq!(
        registry.workstream_overviews().unwrap()[0].lifecycle,
        WorkstreamLifecycle::Parked
    );

    registry.reserve_runtime(registered.workstream_id).unwrap();
    assert_eq!(
        registry.workstream_overviews().unwrap()[0].lifecycle,
        WorkstreamLifecycle::Open
    );
}

#[test]
fn conversation_activity_time_survives_park_and_resume() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();

    assert_eq!(
        registry.workstream_overviews().unwrap()[0].last_activity_at_millis,
        None
    );
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    assert_eq!(
        registry.workstream_overviews().unwrap()[0].last_activity_at_millis,
        None
    );
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
        )
        .unwrap();
    let activity_at_millis = registry.workstream_overviews().unwrap()[0]
        .last_activity_at_millis
        .unwrap();
    let live = registry
        .runtime_for_workstream(registered.workstream_id)
        .unwrap()
        .unwrap();
    registry
        .park_runtime(live.runtime_id, live.revision)
        .unwrap();
    registry.reserve_runtime(registered.workstream_id).unwrap();

    assert_eq!(
        registry.workstream_overviews().unwrap()[0].last_activity_at_millis,
        Some(activity_at_millis)
    );
}

#[test]
fn navigator_overview_joins_only_bounded_workstream_metadata() {
    let (_temporary, mut registry) = registry();
    let fingerprint = format!("git-remote-v1:{}", "a".repeat(64));
    let registered = registry
        .register_external_workstream_with_metadata(
            Path::new("/disposable/repository"),
            "repository",
            Some(&fingerprint),
            Some("github.com/owner/repository"),
            ProviderKind::Codex,
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: runtime.cwd.to_string_lossy().into_owned(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    registry
        .record_thread_metadata(
            runtime.runtime_id,
            &ProviderSessionId::codex("session-a").unwrap(),
            Some("Native title"),
        )
        .unwrap();
    let overview = registry.workstream_overviews().unwrap();

    assert_eq!(overview.len(), 1);
    assert_eq!(overview[0].workstream_id, registered.workstream_id);
    assert_eq!(overview[0].lifecycle, WorkstreamLifecycle::Open);
    assert_eq!(
        overview[0].remote_identity_display.as_deref(),
        Some("github.com/owner/repository")
    );
    assert_eq!(
        overview[0]
            .binding
            .as_ref()
            .and_then(|binding| binding.observed_thread_name.as_deref()),
        Some("Native title")
    );
    assert_eq!(
        overview[0]
            .binding
            .as_ref()
            .map(|binding| binding.name_state),
        Some(NameState::Named)
    );
}

#[test]
fn lost_runtime_requires_verified_native_resume_to_reopen() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let lost = settled_runtime(&mut registry, registered.workstream_id);
    registry
        .mark_runtime_recovery_required(lost.runtime_id, lost.revision)
        .unwrap();

    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::RecoveryRequired);
    assert_eq!(overview.runtime.unwrap().status, RuntimeStatus::Unknown);
    assert_eq!(
        overview.binding.unwrap().native_session_id.native_id(),
        "session-a"
    );
    assert!(
        overview
            .attention
            .as_ref()
            .and_then(|attention| attention.recovery_unseen_since_revision)
            .is_some()
    );
    assert!(
        overview
            .attention
            .as_ref()
            .and_then(|attention| attention.result_unseen_since_revision)
            .is_some()
    );

    let recovery = registry
        .reserve_runtime_recovery(registered.workstream_id)
        .unwrap();
    let cwd = recovery.cwd.to_string_lossy().into_owned();
    assert!(matches!(
        registry.apply_lifecycle_observation(
            recovery.runtime_id,
            &recovery.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
    registry
        .apply_lifecycle_observation(
            recovery.runtime_id,
            &recovery.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd,
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("resume".to_owned()),
            },
        )
        .unwrap();

    let reopened = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(reopened.lifecycle, WorkstreamLifecycle::Open);
    assert_eq!(reopened.runtime.unwrap().status, RuntimeStatus::Idle);
    let attention = reopened.attention.unwrap();
    assert_eq!(attention.recovery_unseen_since_revision, None);
    assert!(attention.result_unseen_since_revision.is_some());
}

#[test]
fn unbound_runtime_recovery_accepts_only_a_native_resume_picker_selection() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 102, "birth-a")
        .unwrap();
    let launched = registry
        .runtime_for_workstream(registered.workstream_id)
        .unwrap()
        .unwrap();
    registry
        .mark_runtime_recovery_required(launched.runtime_id, launched.revision)
        .unwrap();
    let recovery = registry
        .reserve_runtime_recovery(registered.workstream_id)
        .unwrap();
    let observation = |source: &str| LifecycleObservation {
        event: LifecycleEvent::SessionStart,
        cwd: recovery.cwd.to_string_lossy().into_owned(),
        native_session_id: "selected-session".to_owned(),
        turn_id: None,
        source: Some(source.to_owned()),
    };

    assert!(matches!(
        registry.apply_lifecycle_observation(
            recovery.runtime_id,
            &recovery.tmux_generation,
            observation("startup"),
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
    registry
        .apply_lifecycle_observation(
            recovery.runtime_id,
            &recovery.tmux_generation,
            observation("resume"),
        )
        .unwrap();
    let overview = registry.workstream_overviews().unwrap().remove(0);
    assert_eq!(overview.lifecycle, WorkstreamLifecycle::Open);
    assert_eq!(
        overview.binding.unwrap().native_session_id.native_id(),
        "selected-session"
    );
}

#[test]
fn navigator_overview_orders_latest_observed_activity_first() {
    let (_temporary, mut registry) = registry();
    let first = registry
        .register_external_workstream(
            PathBuf::from("/disposable/first"),
            "first-repository".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let second = registry
        .register_external_workstream(
            PathBuf::from("/disposable/second"),
            "second-repository".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(first.workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    for observation in [
        LifecycleObservation {
            event: LifecycleEvent::SessionStart,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: None,
            source: Some("startup".to_owned()),
        },
        LifecycleObservation {
            event: LifecycleEvent::UserPromptSubmit,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: Some("turn-a".to_owned()),
            source: None,
        },
        LifecycleObservation {
            event: LifecycleEvent::Stop,
            cwd,
            native_session_id: "session-a".to_owned(),
            turn_id: Some("turn-a".to_owned()),
            source: None,
        },
    ] {
        registry
            .apply_lifecycle_observation(runtime.runtime_id, &runtime.tmux_generation, observation)
            .unwrap();
    }

    let overview = registry.workstream_overviews().unwrap();

    assert_eq!(
        overview
            .iter()
            .map(|entry| entry.workstream_id)
            .collect::<Vec<_>>(),
        vec![first.workstream_id, second.workstream_id]
    );
    assert!(overview[0].last_activity_sequence > overview[1].last_activity_sequence);
    assert!(overview[0].last_activity_at_millis.is_some());
    assert_eq!(overview[1].last_activity_at_millis, None);
}

#[test]
fn navigator_snapshot_pages_past_its_per_page_workstream_limit() {
    let (_temporary, mut registry) = registry();
    for index in 0..=MAX_NAVIGATOR_WORKSTREAMS {
        registry
            .register_external_workstream(
                PathBuf::from(format!("/disposable/repository-{index}")),
                format!("common-dir-identity-{index}"),
                "deadbeef".to_owned(),
            )
            .unwrap();
    }

    let first = registry
        .workstream_overview_page(0, MAX_NAVIGATOR_WORKSTREAMS)
        .unwrap();
    assert_eq!(first.workstreams.len(), MAX_NAVIGATOR_WORKSTREAMS);
    assert_eq!(
        first.next_cursor,
        Some(u32::try_from(MAX_NAVIGATOR_WORKSTREAMS).unwrap())
    );
    let second = registry
        .workstream_overview_page(first.next_cursor.unwrap(), MAX_NAVIGATOR_WORKSTREAMS)
        .unwrap();
    assert_eq!(second.workstreams.len(), 1);
    assert_eq!(second.next_cursor, None);
    assert_eq!(
        registry.workstream_overviews().unwrap().len(),
        MAX_NAVIGATOR_WORKSTREAMS + 1
    );
}

#[test]
fn hook_process_birth_must_match_the_prepared_runtime_generation() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();

    assert!(matches!(
        registry.expected_hook_process_birth(runtime.runtime_id, &runtime.tmux_generation),
        Err(StateError::HookEvidenceMismatch)
    ));
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 103, "birth-1")
        .unwrap();
    assert_eq!(
        registry
            .expected_hook_process_birth(runtime.runtime_id, &runtime.tmux_generation)
            .unwrap(),
        "birth-1"
    );
    assert!(matches!(
        registry.expected_hook_process_birth(runtime.runtime_id, "stale-generation"),
        Err(StateError::HookEvidenceMismatch)
    ));
}

#[test]
fn matching_hook_lifecycle_binds_and_sets_sticky_result_attention_atomically() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let start = LifecycleObservation {
        event: LifecycleEvent::SessionStart,
        cwd: runtime.cwd.to_string_lossy().into_owned(),
        native_session_id: "session-a".to_owned(),
        turn_id: None,
        source: Some("startup".to_owned()),
    };
    registry
        .apply_lifecycle_observation(runtime.runtime_id, &runtime.tmux_generation, start)
        .unwrap();
    let prompt = LifecycleObservation {
        event: LifecycleEvent::UserPromptSubmit,
        cwd: runtime.cwd.to_string_lossy().into_owned(),
        native_session_id: "session-a".to_owned(),
        turn_id: Some("turn-a".to_owned()),
        source: None,
    };
    registry
        .apply_lifecycle_observation(runtime.runtime_id, &runtime.tmux_generation, prompt)
        .unwrap();
    let stop = LifecycleObservation {
        event: LifecycleEvent::Stop,
        cwd: runtime.cwd.to_string_lossy().into_owned(),
        native_session_id: "session-a".to_owned(),
        turn_id: Some("turn-a".to_owned()),
        source: None,
    };
    registry
        .apply_lifecycle_observation(runtime.runtime_id, &runtime.tmux_generation, stop)
        .unwrap();

    assert_eq!(
        registry
            .binding_for_runtime(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .last_settled_turn_id
            .as_deref(),
        Some("turn-a")
    );
    assert_eq!(
        registry
            .attention(registered.workstream_id)
            .unwrap()
            .unwrap()
            .latest_turn_id
            .as_deref(),
        Some("turn-a")
    );
}

#[test]
fn proven_idle_clear_rotates_only_the_current_conversation_tip() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    for observation in [
        LifecycleObservation {
            event: LifecycleEvent::SessionStart,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: None,
            source: Some("startup".to_owned()),
        },
        LifecycleObservation {
            event: LifecycleEvent::UserPromptSubmit,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: Some("turn-a".to_owned()),
            source: None,
        },
        LifecycleObservation {
            event: LifecycleEvent::Stop,
            cwd: cwd.clone(),
            native_session_id: "session-a".to_owned(),
            turn_id: Some("turn-a".to_owned()),
            source: None,
        },
    ] {
        registry
            .apply_lifecycle_observation(runtime.runtime_id, &runtime.tmux_generation, observation)
            .unwrap();
    }

    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd,
                native_session_id: "session-b".to_owned(),
                turn_id: None,
                source: Some("clear".to_owned()),
            },
        )
        .unwrap();

    let binding = registry
        .binding_for_runtime(runtime.runtime_id)
        .unwrap()
        .unwrap();
    assert_eq!(binding.native_session_id.native_id(), "session-b");
    assert_eq!(binding.start_source, "clear");
    assert_eq!(binding.last_settled_turn_id, None);
    assert_eq!(
        binding
            .predecessor_native_session_id
            .as_ref()
            .map(ProviderSessionId::native_id),
        Some("session-a")
    );
    assert_eq!(
        registry
            .runtime_for_workstream(registered.workstream_id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeStatus::Idle
    );
    assert_eq!(
        registry
            .attention(registered.workstream_id)
            .unwrap()
            .unwrap()
            .latest_native_session_id
            .as_ref()
            .map(ProviderSessionId::native_id),
        Some("session-a")
    );
}

#[test]
fn changed_session_start_rejects_unproven_sources_and_working_turns() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let cwd = runtime.cwd.to_string_lossy().into_owned();
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        )
        .unwrap();
    assert!(matches!(
        registry.apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd: cwd.clone(),
                native_session_id: "session-b".to_owned(),
                turn_id: None,
                source: Some("startup".to_owned()),
            },
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
    registry
        .apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::UserPromptSubmit,
                cwd: cwd.clone(),
                native_session_id: "session-a".to_owned(),
                turn_id: Some("turn-a".to_owned()),
                source: None,
            },
        )
        .unwrap();
    assert!(matches!(
        registry.apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            LifecycleObservation {
                event: LifecycleEvent::SessionStart,
                cwd,
                native_session_id: "session-b".to_owned(),
                turn_id: None,
                source: Some("clear".to_owned()),
            },
        ),
        Err(StateError::HookEvidenceMismatch)
    ));
    assert_eq!(
        registry
            .binding_for_runtime(runtime.runtime_id)
            .unwrap()
            .unwrap()
            .native_session_id,
        ProviderSessionId::codex("session-a").unwrap()
    );
}

#[test]
fn stale_or_rebound_hook_cannot_replace_a_managed_session() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_external_workstream(
            PathBuf::from("/disposable/repository"),
            "common-dir-identity".to_owned(),
            "deadbeef".to_owned(),
        )
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let forged = LifecycleObservation {
        event: LifecycleEvent::SessionStart,
        cwd: runtime.cwd.to_string_lossy().into_owned(),
        native_session_id: "forged-session".to_owned(),
        turn_id: None,
        source: Some("startup".to_owned()),
    };

    assert!(matches!(
        registry.apply_lifecycle_observation(runtime.runtime_id, "stale", forged),
        Err(StateError::HookEvidenceMismatch)
    ));
    assert_eq!(
        registry.binding_for_runtime(runtime.runtime_id).unwrap(),
        None
    );
}

#[test]
fn neutral_lifecycle_observation_still_rejects_non_codex_runtime_identity() {
    let (_temporary, mut registry) = registry();
    let registered = registry
        .register_project_root(Path::new("/disposable/repository"), ProviderKind::OpenCode)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    let observation = LifecycleObservation {
        event: LifecycleEvent::SessionStart,
        cwd: runtime.cwd.to_string_lossy().into_owned(),
        native_session_id: "opencode-session".to_owned(),
        turn_id: None,
        source: Some("startup".to_owned()),
    };

    assert!(matches!(
        registry.apply_lifecycle_observation(
            runtime.runtime_id,
            &runtime.tmux_generation,
            observation,
        ),
        Err(StateError::ProviderIdentityMismatch)
    ));
}

#[test]
fn observer_ownership_is_stable_and_lifecycle_is_explicit() {
    let (_temporary, mut registry) = registry();
    let ownership = ProfileOwnership {
        canonical_path: PathBuf::from("/private/codex/wsnav-observer.config.toml"),
        owner_id: "owner".to_owned(),
        profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
        hook_executable: PathBuf::from("/private/bin/wsnav"),
        content_hash: "hash".to_owned(),
    };
    let pending = registry
        .record_codex_integration(ownership.clone(), IntegrationLifecycle::TrustPending)
        .unwrap();
    let ready = registry
        .record_codex_integration(ownership, IntegrationLifecycle::Ready)
        .unwrap();

    assert_eq!(pending.lifecycle, IntegrationLifecycle::TrustPending);
    assert_eq!(ready.lifecycle, IntegrationLifecycle::Ready);
    assert!(ready.revision > pending.revision);
}

#[test]
fn explicit_profile_update_rotates_exact_ownership_back_to_trust_pending() {
    let (_temporary, mut registry) = registry();
    let original = ProfileOwnership {
        canonical_path: PathBuf::from("/private/codex/wsnav-observer.config.toml"),
        owner_id: "owner".to_owned(),
        profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
        hook_executable: PathBuf::from("/private/bin/wsnav-old"),
        content_hash: "old-hash".to_owned(),
    };
    let ready = registry
        .record_codex_integration(original.clone(), IntegrationLifecycle::Ready)
        .unwrap();
    let replacement = ProfileOwnership {
        hook_executable: PathBuf::from("/private/bin/wsnav-new"),
        content_hash: "new-hash".to_owned(),
        ..original.clone()
    };

    let updated = registry
        .replace_codex_integration(
            &original,
            replacement.clone(),
            IntegrationLifecycle::TrustPending,
        )
        .unwrap();

    assert_eq!(updated.ownership, replacement);
    assert_eq!(updated.lifecycle, IntegrationLifecycle::TrustPending);
    assert!(updated.revision > ready.revision);
    assert!(matches!(
        registry.replace_codex_integration(
            &original,
            replacement,
            IntegrationLifecycle::TrustPending,
        ),
        Err(StateError::IntegrationOwnershipMismatch)
    ));
}
