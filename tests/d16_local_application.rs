use std::{cell::Cell, fmt::Debug};

use uuid::Uuid;
use wsnav::{
    application::{
        ApplicationAction, ApplicationError, ApplicationOutcome, AttachEvidence, AttachOutcome,
        AttentionKind, AttentionSnapshot, BrowserEntry, BrowserListing, BrowserPath,
        BrowserRootPath, LocalApplication, LocalApplicationBackend, ObserverIntent,
        ObserverReadiness, ObserverReadinessEvidence, OperationSnapshot, ProjectBrowserSnapshot,
        ProjectRefreshRequest, ProjectSnapshotInput, ProviderCapability, ProviderCapabilityReason,
        ProviderCapabilityStatus, RevisedIdentity, RevisionSubject, RuntimeSnapshot, SnapshotInput,
        SnapshotLimitKind, SnapshotLimits, WorkstreamSnapshotInput, derived_host_label,
    },
    domain::{
        HostId, LocationId, OperationId, OperationKind, OperationPhase, ProjectId, ProviderKind,
        Revision, RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
    },
};

fn id<T: From<Uuid>>(value: u128) -> T {
    Uuid::from_u128(value).into()
}

fn revision(value: i64) -> Revision {
    Revision::try_from(value).expect("valid test revision")
}

fn location(
    project_id: ProjectId,
    location_id: LocationId,
    name: &str,
    is_label_source: bool,
) -> wsnav::application::LocationSnapshot {
    wsnav::application::LocationSnapshot {
        project_id,
        location_id,
        display_name: name.to_owned(),
        revision: Revision::INITIAL,
        repository_fingerprint: None,
        origin_display: None,
        is_label_source,
    }
}

fn workstream(
    project_id: ProjectId,
    location_id: LocationId,
    workstream_id: WorkstreamId,
    activity: i64,
    archived: bool,
) -> WorkstreamSnapshotInput {
    WorkstreamSnapshotInput {
        project_id,
        location_id,
        workstream_id,
        provider: ProviderKind::Codex,
        lifecycle: WorkstreamLifecycle::Open,
        archived,
        last_activity_sequence: activity,
        last_activity_at_millis: Some(activity.saturating_mul(1_000)),
        revision: Revision::INITIAL,
        runtime: Some(RuntimeSnapshot {
            runtime_id: id(0x900),
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
            observer_degraded: false,
        }),
        attention: AttentionSnapshot {
            result_unseen: false,
            recovery_unseen: false,
            revision: Revision::INITIAL,
        },
        native_name: Some("native title".to_owned()),
    }
}

#[derive(Clone)]
struct FakeBackend {
    snapshot: SnapshotInput,
    readiness: ObserverReadinessEvidence,
    action_result: Result<ApplicationOutcome, ApplicationError>,
    attach_result: Result<AttachOutcome, ApplicationError>,
    snapshot_calls: Cell<usize>,
    readiness_calls: Cell<usize>,
    apply_calls: usize,
    attach_calls: usize,
    action: Option<ApplicationAction>,
    evidence: Option<AttachEvidence>,
    external_effects: usize,
}

impl FakeBackend {
    fn new(snapshot: SnapshotInput) -> Self {
        Self {
            snapshot,
            readiness: ObserverReadinessEvidence {
                readiness: ObserverReadiness::Ready,
                integration_revision: Some(Revision::INITIAL),
            },
            action_result: Ok(ApplicationOutcome::Applied {
                identity: RevisedIdentity::Workstream(id(0x10), Revision::INITIAL),
            }),
            attach_result: Ok(AttachOutcome {
                workstream_id: id(0x10),
                runtime_id: id(0x20),
            }),
            snapshot_calls: Cell::new(0),
            readiness_calls: Cell::new(0),
            apply_calls: 0,
            attach_calls: 0,
            action: None,
            evidence: None,
            external_effects: 0,
        }
    }
}

impl LocalApplicationBackend for FakeBackend {
    fn read_snapshot(&self) -> Result<SnapshotInput, ApplicationError> {
        // This fake records no external work here by design: production
        // backends must keep the same passive contract.
        self.snapshot_calls.set(self.snapshot_calls.get() + 1);
        Ok(self.snapshot.clone())
    }

    fn observer_readiness(&self) -> Result<ObserverReadinessEvidence, ApplicationError> {
        self.readiness_calls.set(self.readiness_calls.get() + 1);
        Ok(self.readiness)
    }

    fn apply(
        &mut self,
        action: &ApplicationAction,
    ) -> Result<ApplicationOutcome, ApplicationError> {
        self.apply_calls += 1;
        self.action = Some(action.clone());
        self.action_result.clone()
    }

    fn attach(&mut self, evidence: &AttachEvidence) -> Result<AttachOutcome, ApplicationError> {
        self.attach_calls += 1;
        self.evidence = Some(*evidence);
        self.attach_result.clone()
    }
}

fn base_input() -> SnapshotInput {
    let project_a: ProjectId = id(0xA);
    let project_b: ProjectId = id(0xB);
    let location_a1: LocationId = id(0xA1);
    let location_a2: LocationId = id(0xA2);
    let location_b_primary: LocationId = id(0xB1);
    SnapshotInput {
        projects: vec![
            ProjectSnapshotInput {
                project_id: project_a,
                display_name: "project-a".to_owned(),
                revision: revision(2),
                locations: vec![
                    location(project_a, location_a2, "second", false),
                    location(project_a, location_a1, "first", true),
                ],
                label_location_id: location_a1,
                repository_fingerprint: None,
                origin_display: None,
            },
            ProjectSnapshotInput {
                project_id: project_b,
                display_name: "project-b".to_owned(),
                revision: Revision::INITIAL,
                locations: vec![location(project_b, location_b_primary, "only", true)],
                label_location_id: location_b_primary,
                repository_fingerprint: None,
                origin_display: None,
            },
        ],
        workstreams: vec![
            workstream(project_a, location_a1, id(0xA12), 100, true),
            workstream(project_b, location_b_primary, id(0xB11), 8, false),
            workstream(project_a, location_a2, id(0xA21), 5, false),
            workstream(project_a, location_a2, id(0xA22), 4, false),
            workstream(project_b, location_b_primary, id(0xB12), 1, true),
        ],
        unresolved_operations: vec![
            OperationSnapshot {
                operation_id: id(0x42),
                kind: OperationKind::Start,
                provider: ProviderKind::OpenCode,
                source_workstream_id: None,
                phase: OperationPhase::AwaitingReconciliation,
                revision: Revision::INITIAL,
            },
            OperationSnapshot {
                operation_id: id(0x41),
                kind: OperationKind::Fork,
                provider: ProviderKind::Codex,
                source_workstream_id: Some(id(0xA21)),
                phase: OperationPhase::Prepared,
                revision: Revision::INITIAL,
            },
        ],
        observer_readiness: ObserverReadinessEvidence {
            readiness: ObserverReadiness::Ready,
            integration_revision: Some(Revision::INITIAL),
        },
        project_browser: ProjectBrowserSnapshot {
            root_label: "~".to_owned(),
            revision: Revision::INITIAL,
        },
        provider_capabilities: vec![
            ProviderCapability {
                provider: ProviderKind::OpenCode,
                status: ProviderCapabilityStatus::Available,
                reason: None,
                fresh_launch: true,
                exact_resume: true,
                observe: true,
                metadata_read: true,
                navigator_rename: false,
                fork: false,
            },
            ProviderCapability {
                provider: ProviderKind::Codex,
                status: ProviderCapabilityStatus::Available,
                reason: None,
                fresh_launch: true,
                exact_resume: true,
                observe: true,
                metadata_read: true,
                navigator_rename: true,
                fork: true,
            },
        ],
    }
}

#[test]
fn snapshot_orders_project_locations_children_and_operations_deterministically() {
    let host_id: HostId = id(0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00);
    let fake = FakeBackend::new(base_input());
    let app = LocalApplication::with_hostname(fake, host_id, Some("  devbox  "));
    let snapshot = app.snapshot().expect("bounded snapshot");

    assert_eq!(snapshot.host_display, "devbox");
    // Canonical Project inventory is opaque-ID ordered; page groups are
    // independently ordered by only their included members.
    assert_eq!(snapshot.projects[0].project_id, id(0xA));
    assert_eq!(snapshot.projects[1].project_id, id(0xB));
    assert_eq!(snapshot.active_project_groups[0].project_id, id(0xB));
    assert_eq!(snapshot.active_project_groups[1].project_id, id(0xA));
    assert_eq!(snapshot.archived_project_groups[0].project_id, id(0xA));
    assert_eq!(snapshot.archived_project_groups[1].project_id, id(0xB));
    assert_eq!(
        snapshot.projects[0]
            .locations
            .iter()
            .map(|location| location.location_id)
            .collect::<Vec<_>>(),
        vec![id(0xA1), id(0xA2)]
    );
    assert_eq!(
        snapshot.active_project_groups[1]
            .workstreams
            .iter()
            .map(|workstream| workstream.workstream_id)
            .collect::<Vec<_>>(),
        vec![id(0xA21), id(0xA22)]
    );
    assert_eq!(snapshot.unresolved_operations[0].operation_id, id(0x41));
    assert_eq!(
        snapshot.provider_capabilities[0].provider,
        ProviderKind::Codex
    );
    assert_eq!(snapshot.active_workstreams().count(), 3);
    assert_eq!(snapshot.archived_workstreams().count(), 2);
    assert_eq!(
        snapshot
            .active_workstreams()
            .find(|workstream| workstream.workstream_id == id(0xB11))
            .map(|workstream| workstream.last_activity_at_millis),
        Some(Some(8_000))
    );
    let codex = snapshot
        .provider_capabilities
        .iter()
        .find(|capability| capability.provider == ProviderKind::Codex)
        .expect("Codex capability");
    assert!(codex.eligible_for_new());
    assert!(codex.eligible_for_resume());
    assert!(codex.eligible_for_fork());
    let opencode = snapshot
        .provider_capabilities
        .iter()
        .find(|capability| capability.provider == ProviderKind::OpenCode)
        .expect("OpenCode capability");
    assert!(opencode.eligible_for_new());
    assert!(!opencode.eligible_for_fork());

    let mut not_recoverable = *opencode;
    not_recoverable.exact_resume = false;
    assert!(!not_recoverable.eligible_for_new());
}

#[test]
fn provider_capability_set_requires_known_unique_valid_evidence() {
    let mut missing = base_input();
    missing
        .provider_capabilities
        .retain(|capability| capability.provider == ProviderKind::Codex);
    let app = LocalApplication::new(FakeBackend::new(missing), id(0x55), None);
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::MissingProviderCapability(
            ProviderKind::OpenCode
        ))
    );

    let mut unavailable = base_input();
    unavailable.provider_capabilities[0] = ProviderCapability {
        provider: ProviderKind::OpenCode,
        status: ProviderCapabilityStatus::Unavailable,
        reason: Some(ProviderCapabilityReason::NotInstalled),
        fresh_launch: false,
        exact_resume: false,
        observe: false,
        metadata_read: false,
        navigator_rename: false,
        fork: false,
    };
    let app = LocalApplication::new(FakeBackend::new(unavailable), id(0x55), None);
    let snapshot = app.snapshot().expect("valid unavailable evidence");
    let opencode = snapshot
        .provider_capabilities
        .iter()
        .find(|capability| capability.provider == ProviderKind::OpenCode)
        .expect("OpenCode capability");
    assert!(!opencode.eligible_for_new());

    let mut unsupported_true = base_input();
    unsupported_true.provider_capabilities[0] = ProviderCapability {
        provider: ProviderKind::OpenCode,
        status: ProviderCapabilityStatus::Unknown,
        reason: Some(ProviderCapabilityReason::ProbeFailed),
        fresh_launch: true,
        exact_resume: false,
        observe: false,
        metadata_read: false,
        navigator_rename: false,
        fork: false,
    };
    let app = LocalApplication::new(FakeBackend::new(unsupported_true), id(0x55), None);
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::InvalidProviderCapability(
            ProviderKind::OpenCode
        ))
    );

    let mut reason_on_available = base_input();
    reason_on_available.provider_capabilities[0].reason =
        Some(ProviderCapabilityReason::UnsupportedVersion);
    let app = LocalApplication::new(FakeBackend::new(reason_on_available), id(0x55), None);
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::InvalidProviderCapability(
            ProviderKind::OpenCode
        ))
    );

    let mut unknown_without_reason = base_input();
    unknown_without_reason.provider_capabilities[0] = ProviderCapability {
        provider: ProviderKind::OpenCode,
        status: ProviderCapabilityStatus::Unknown,
        reason: None,
        fresh_launch: false,
        exact_resume: false,
        observe: false,
        metadata_read: false,
        navigator_rename: false,
        fork: false,
    };
    let app = LocalApplication::new(FakeBackend::new(unknown_without_reason), id(0x55), None);
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::InvalidProviderCapability(
            ProviderKind::OpenCode
        ))
    );

    let mut duplicate = base_input();
    duplicate
        .provider_capabilities
        .push(duplicate.provider_capabilities[0]);
    let app = LocalApplication::new(FakeBackend::new(duplicate), id(0x55), None);
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::DuplicateProviderCapability(
            ProviderKind::OpenCode,
        ))
    );
}

#[test]
fn snapshot_refuses_over_bound_projection() {
    let mut input = base_input();
    input.projects.push(ProjectSnapshotInput {
        project_id: id(0xC),
        display_name: "project-c".to_owned(),
        revision: Revision::INITIAL,
        label_location_id: id(0xC1),
        repository_fingerprint: None,
        origin_display: None,
        locations: vec![],
    });
    let app = LocalApplication::new(FakeBackend::new(input), id(0x55), None)
        .with_limits(SnapshotLimits::new(2, 512, 512, 256, 8));
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::SnapshotOverLimit {
            kind: SnapshotLimitKind::Projects,
            limit: 2,
        })
    );
}

#[test]
fn snapshot_rejects_malformed_negative_activity_timestamp() {
    let mut input = base_input();
    input.workstreams[0].last_activity_at_millis = Some(-1);
    let app = LocalApplication::new(FakeBackend::new(input), id(0x55), None);
    assert_eq!(
        app.snapshot(),
        Err(ApplicationError::InvalidSnapshotEntity {
            entity: wsnav::application::SnapshotEntity::Workstream,
        })
    );
}

#[test]
fn snapshot_has_no_passive_external_effects() {
    let fake = FakeBackend::new(base_input());
    let app = LocalApplication::new(fake, id(0x55), None);
    let _ = app.snapshot().expect("snapshot");
    let backend = app.backend();
    assert_eq!(backend.snapshot_calls.get(), 1);
    assert_eq!(backend.apply_calls, 0);
    assert_eq!(backend.attach_calls, 0);
    assert_eq!(backend.readiness_calls.get(), 0);
    assert_eq!(backend.external_effects, 0);
}

#[test]
fn derived_host_label_validates_trimmed_hostname_and_format_chars() {
    let host_id: HostId = id(0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00);
    assert_eq!(derived_host_label(host_id, Some("  local-1  ")), "local-1");
    assert_eq!(
        derived_host_label(host_id, Some("line\nname")),
        "host-11223344"
    );
    assert_eq!(
        derived_host_label(host_id, Some("zero\u{200B}width")),
        "host-11223344"
    );
    assert_eq!(
        derived_host_label(host_id, Some("bidi\u{202A}name")),
        "host-11223344"
    );
    assert_eq!(
        derived_host_label(host_id, Some(&"x".repeat(65))),
        "host-11223344"
    );
    assert_eq!(derived_host_label(host_id, Some("\t")), "host-11223344");
}

#[test]
fn browser_root_path_is_absolute_and_lexically_normalized() {
    assert!(BrowserRootPath::new("/").is_ok());
    assert!(BrowserRootPath::new("/home/bryan/work").is_ok());
    for invalid in [
        "home/bryan",
        "",
        "relative/",
        "/home//work",
        "/home/./work",
        "/home/../work",
        "/home/work/",
    ] {
        assert!(
            BrowserRootPath::new(invalid).is_err(),
            "accepted invalid root {invalid:?}"
        );
    }
    assert!(BrowserRootPath::new(format!("/{}", "x".repeat(4095))).is_ok());
    assert!(BrowserRootPath::new(format!("/{}", "x".repeat(4096))).is_err());
    assert!(BrowserPath::new("child/project").is_ok());
    assert!(BrowserPath::new("../project").is_err());
    assert!(BrowserPath::new("x".repeat(1024)).is_ok());
    assert!(BrowserPath::new("x".repeat(1025)).is_err());
}

#[test]
fn stale_revision_is_a_typed_action_error() {
    let workstream_id: WorkstreamId = id(0x10);
    let mut fake = FakeBackend::new(base_input());
    fake.action_result = Err(ApplicationError::StaleRevision {
        subject: RevisionSubject::Workstream(workstream_id),
        expected: revision(2),
        current: revision(3),
    });
    let mut app = LocalApplication::new(fake, id(0x55), None);
    let result = app.apply(ApplicationAction::Rename {
        workstream_id,
        expected_revision: revision(2),
        name: "new name".to_owned(),
    });
    assert_eq!(
        result,
        Err(ApplicationError::StaleRevision {
            subject: RevisionSubject::Workstream(workstream_id),
            expected: revision(2),
            current: revision(3),
        })
    );
}

#[test]
fn dormant_location_new_is_exact_and_does_not_require_active_workstream() {
    let project_id: ProjectId = id(0xA);
    let location_id: LocationId = id(0xA1);
    let mut fake = FakeBackend::new(base_input());
    fake.action_result = Ok(ApplicationOutcome::Created {
        workstream_id: id(0xD0),
        location_id,
        revision: revision(2),
    });
    let mut app = LocalApplication::new(fake, id(0x55), None);
    let result = app
        .apply(ApplicationAction::NewAtLocation {
            project_id,
            location_id,
            expected_project_revision: revision(2),
            expected_location_revision: Revision::INITIAL,
            provider: ProviderKind::OpenCode,
        })
        .expect("independent New at dormant Location");
    assert!(matches!(result, ApplicationOutcome::Created { .. }));
    assert!(matches!(
        app.backend().action,
        Some(ApplicationAction::NewAtLocation {
            project_id: actual_project,
            location_id: actual_location,
            ..
        }) if actual_project == project_id && actual_location == location_id
    ));
}

#[test]
fn observer_readiness_returns_captured_guide_without_mutation() {
    let workstream_id: WorkstreamId = id(0x10);
    let mut fake = FakeBackend::new(base_input());
    fake.readiness = ObserverReadinessEvidence {
        readiness: ObserverReadiness::TrustReviewRequired,
        integration_revision: Some(revision(3)),
    };
    let mut app = LocalApplication::new(fake, id(0x55), None);
    let result = app
        .apply(ApplicationAction::Start {
            workstream_id,
            expected_revision: revision(7),
            provider: ProviderKind::Codex,
        })
        .expect("typed guide");
    let ApplicationOutcome::ObserverReadinessRequired(guide) = result else {
        panic!("expected observer readiness guide");
    };
    assert_eq!(
        guide.evidence,
        ObserverReadinessEvidence {
            readiness: ObserverReadiness::TrustReviewRequired,
            integration_revision: Some(revision(3)),
        }
    );
    assert!(guide.explicit_interactive_consent_required);
    assert!(guide.native_trust_review_required);
    assert_eq!(
        guide.intent,
        ObserverIntent::Start {
            workstream_id,
            expected_revision: revision(7),
            provider: ProviderKind::Codex,
        }
    );
    assert_eq!(app.backend().apply_calls, 0);
    assert_eq!(app.backend().readiness_calls.get(), 1);
}

#[test]
fn attach_delegates_exact_opaque_local_evidence_and_rejects_mismatch() {
    let workstream_id: WorkstreamId = id(0x10);
    let runtime_id: RuntimeId = id(0x20);
    let evidence = AttachEvidence {
        workstream_id,
        runtime_id,
        expected_workstream_revision: revision(4),
        expected_runtime_revision: revision(9),
    };
    let mut fake = FakeBackend::new(base_input());
    fake.attach_result = Ok(AttachOutcome {
        workstream_id,
        runtime_id,
    });
    let mut app = LocalApplication::new(fake, id(0x55), None);
    assert_eq!(
        app.attach(evidence),
        Ok(AttachOutcome {
            workstream_id,
            runtime_id,
        })
    );
    assert_eq!(app.backend().evidence, Some(evidence));
    assert_eq!(app.backend().attach_calls, 1);

    app.backend_mut().attach_result = Ok(AttachOutcome {
        workstream_id: id(0x11),
        runtime_id,
    });
    assert_eq!(
        app.attach(evidence),
        Err(ApplicationError::AttachmentEvidenceMismatch)
    );
}

#[test]
fn browser_listing_is_bounded_and_deterministically_sorted() {
    let mut fake = FakeBackend::new(base_input());
    fake.action_result = Ok(ApplicationOutcome::BrowserListed(BrowserListing {
        root_label: "~".to_owned(),
        relative_path: BrowserPath::root(),
        include_hidden: true,
        entries: vec![
            BrowserEntry {
                name: "zeta".to_owned(),
                is_git_repository: false,
            },
            BrowserEntry {
                name: ".git-work".to_owned(),
                is_git_repository: true,
            },
            BrowserEntry {
                name: "Alpha".to_owned(),
                is_git_repository: true,
            },
        ],
        revision: Revision::INITIAL,
    }));
    let mut app = LocalApplication::new(fake, id(0x55), None);
    let outcome = app
        .apply(ApplicationAction::ListProjectBrowser {
            relative_path: BrowserPath::root(),
            include_hidden: true,
        })
        .expect("browser listing");
    let ApplicationOutcome::BrowserListed(listing) = outcome else {
        panic!("expected listing");
    };
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec![".git-work", "Alpha", "zeta"]
    );
}

#[test]
fn complete_refresh_carries_only_selected_project_and_revision() {
    let project_id: ProjectId = id(0xA);
    let mut app = LocalApplication::new(FakeBackend::new(base_input()), id(0x55), None);
    let result = app
        .apply(ApplicationAction::RefreshProject(ProjectRefreshRequest {
            project_id,
            expected_project_revision: revision(2),
        }))
        .expect("complete refresh action");
    assert!(matches!(result, ApplicationOutcome::Applied { .. }));
    assert!(matches!(
        app.backend().action,
        Some(ApplicationAction::RefreshProject(ProjectRefreshRequest {
            project_id: actual_project,
            expected_project_revision: actual_revision,
        })) if actual_project == project_id && actual_revision == revision(2)
    ));
}

fn assert_public_ids_are_debug<T: Debug>(value: T) {
    let _ = format!("{value:?}");
}

#[allow(dead_code)]
fn type_mentions_all_local_ids() {
    assert_public_ids_are_debug::<ProjectId>(id(1));
    assert_public_ids_are_debug::<LocationId>(id(2));
    assert_public_ids_are_debug::<OperationId>(id(3));
    assert_public_ids_are_debug::<RuntimeId>(id(4));
    assert_public_ids_are_debug::<WorkstreamId>(id(5));
    assert_public_ids_are_debug::<Revision>(Revision::INITIAL);
    let _ = AttentionKind::Result;
}
