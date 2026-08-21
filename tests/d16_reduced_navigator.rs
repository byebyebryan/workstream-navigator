//! Pure controller/model coverage for the D16 navigator surface.

#![allow(clippy::too_many_lines)]

use crossterm::event::KeyCode;
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    layout::Rect,
    style::Color,
};
use uuid::Uuid;
use wsnav::{
    application::{
        ApplicationAction, ApplicationOutcome, AttachEvidence, AttentionSnapshot, BrowserEntry,
        BrowserListing, BrowserPath, LocationSnapshot, ObserverIntent, ObserverReadiness,
        ObserverReadinessEvidence, ObserverReadinessGuide, OperationSnapshot,
        ProjectBrowserSnapshot, ProjectSnapshot, ProjectWorkstreamGroup, ProviderCapability,
        ProviderCapabilityStatus, RevisedIdentity, RuntimeSnapshot, WorkstreamSnapshot,
    },
    domain::{
        LocationId, OperationKind, OperationPhase, ProjectId, ProviderKind, Revision,
        RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
    },
    navigator::{D16Command, D16Modal, D16Model, D16Navigator, D16Page, D16Row, D16RowId},
};

fn id<T: From<Uuid>>(value: u128) -> T {
    Uuid::from_u128(value).into()
}

fn revision(value: i64) -> Revision {
    Revision::try_from(value).expect("valid revision")
}

fn snapshot() -> wsnav::application::ApplicationSnapshot {
    let project_a: ProjectId = id(0xA);
    let project_b: ProjectId = id(0xB);
    let location_a: LocationId = id(0xA1);
    let location_b: LocationId = id(0xB1);
    let active_id: WorkstreamId = id(0xA11);
    let archived_id: WorkstreamId = id(0xB11);
    let active = WorkstreamSnapshot {
        project_id: project_a,
        location_id: location_a,
        workstream_id: active_id,
        provider: ProviderKind::Codex,
        lifecycle: WorkstreamLifecycle::Open,
        archived: false,
        last_activity_sequence: 20,
        last_activity_at_millis: Some(1_000_000),
        revision: revision(3),
        runtime: Some(RuntimeSnapshot {
            runtime_id: id(0xA101),
            status: RuntimeStatus::Idle,
            revision: revision(4),
            observer_degraded: false,
        }),
        attention: AttentionSnapshot {
            result_unseen: true,
            recovery_unseen: false,
            revision: revision(5),
        },
        native_name: Some("active native name".to_owned()),
    };
    let archived = WorkstreamSnapshot {
        project_id: project_b,
        location_id: location_b,
        workstream_id: archived_id,
        provider: ProviderKind::OpenCode,
        lifecycle: WorkstreamLifecycle::Parked,
        archived: true,
        last_activity_sequence: 10,
        last_activity_at_millis: Some(1_000),
        revision: revision(6),
        runtime: None,
        attention: AttentionSnapshot {
            result_unseen: false,
            recovery_unseen: false,
            revision: Revision::INITIAL,
        },
        native_name: Some("archived native name".to_owned()),
    };
    wsnav::application::ApplicationSnapshot {
        host_id: id(0x1234_5678_9abc_def0_0000_0000_0000_0000),
        host_display: "devbox".to_owned(),
        projects: vec![
            ProjectSnapshot {
                project_id: project_a,
                display_name: "alpha".to_owned(),
                revision: revision(7),
                label_location_id: location_a,
                repository_fingerprint: None,
                origin_display: None,
                locations: vec![LocationSnapshot {
                    project_id: project_a,
                    location_id: location_a,
                    display_name: "alpha checkout".to_owned(),
                    revision: revision(8),
                    repository_fingerprint: None,
                    origin_display: None,
                    is_label_source: true,
                }],
            },
            ProjectSnapshot {
                project_id: project_b,
                display_name: "beta".to_owned(),
                revision: revision(9),
                label_location_id: location_b,
                repository_fingerprint: None,
                origin_display: None,
                locations: vec![LocationSnapshot {
                    project_id: project_b,
                    location_id: location_b,
                    display_name: "beta checkout".to_owned(),
                    revision: revision(10),
                    repository_fingerprint: None,
                    origin_display: None,
                    is_label_source: true,
                }],
            },
        ],
        active_project_groups: vec![ProjectWorkstreamGroup {
            project_id: project_a,
            max_activity_sequence: 20,
            workstreams: vec![active],
        }],
        archived_project_groups: vec![ProjectWorkstreamGroup {
            project_id: project_b,
            max_activity_sequence: 10,
            workstreams: vec![archived],
        }],
        unresolved_operations: vec![OperationSnapshot {
            operation_id: id(0xC1),
            kind: OperationKind::Start,
            provider: ProviderKind::Codex,
            source_workstream_id: Some(id(0xA11)),
            phase: OperationPhase::RecoveryRequired,
            revision: revision(11),
        }],
        observer_readiness: ObserverReadinessEvidence {
            readiness: ObserverReadiness::Ready,
            integration_revision: Some(revision(12)),
        },
        project_browser: ProjectBrowserSnapshot {
            root_label: "workspace".to_owned(),
            revision: revision(13),
        },
        provider_capabilities: vec![
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
        ],
    }
}

fn render_buffer(navigator: &D16Navigator, now_millis: Option<i64>) -> Buffer {
    render_buffer_with_area(navigator, now_millis, 90, 18)
}

fn render_buffer_with_area(
    navigator: &D16Navigator,
    now_millis: Option<i64>,
    width: u16,
    height: u16,
) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| navigator.render_at(frame, Rect::new(0, 0, width, height), now_millis))
        .expect("render D16");
    terminal.backend().buffer().clone()
}

fn rendered_text(buffer: &Buffer) -> String {
    buffer
        .content()
        .iter()
        .map(Cell::symbol)
        .collect::<String>()
}

fn first_cell_for_text(buffer: &Buffer, needle: &str) -> Option<Cell> {
    first_position_for_text(buffer, needle).map(|(_, _, cell)| cell)
}

fn first_position_for_text(buffer: &Buffer, needle: &str) -> Option<(u16, u16, Cell)> {
    let width = usize::from(buffer.area.width);
    let needle = needle.chars().collect::<Vec<_>>();
    for row in 0..usize::from(buffer.area.height) {
        let cells = &buffer.content()[row * width..(row + 1) * width];
        for start in 0..cells.len() {
            if needle.iter().enumerate().all(|(offset, expected)| {
                cells
                    .get(start + offset)
                    .is_some_and(|cell| cell.symbol() == expected.to_string())
            }) {
                return cells.get(start).cloned().map(|cell| {
                    (
                        u16::try_from(start).expect("buffer width fits u16"),
                        u16::try_from(row).expect("buffer height fits u16"),
                        cell,
                    )
                });
            }
        }
    }
    None
}

fn rendered_row_segment(buffer: &Buffer, y: u16, x: u16, width: u16) -> String {
    (x..x.saturating_add(width))
        .filter_map(|column| buffer.cell((column, y)))
        .map(Cell::symbol)
        .collect()
}

fn project_catalog_snapshot(count: usize) -> wsnav::application::ApplicationSnapshot {
    let mut snapshot = snapshot();
    snapshot.active_project_groups.clear();
    snapshot.archived_project_groups.clear();
    snapshot.unresolved_operations.clear();
    snapshot.projects = (0..count)
        .map(|index| {
            let project_id: ProjectId = id(0x1_000 + u128::try_from(index).expect("bounded id"));
            let location_id: LocationId = id(0x2_000 + u128::try_from(index).expect("bounded id"));
            ProjectSnapshot {
                project_id,
                display_name: format!("project-{index:02}"),
                revision: Revision::INITIAL,
                label_location_id: location_id,
                repository_fingerprint: None,
                origin_display: None,
                locations: vec![LocationSnapshot {
                    project_id,
                    location_id,
                    display_name: format!("location-{index:02}"),
                    revision: Revision::INITIAL,
                    repository_fingerprint: None,
                    origin_display: None,
                    is_label_source: true,
                }],
            }
        })
        .collect();
    snapshot
}

#[test]
fn pages_are_direct_and_horizontal_keys_are_inert() {
    let mut model = D16Model::new(snapshot());
    assert_eq!(model.page(), D16Page::Workstreams);
    assert!(matches!(
        model.handle_key(KeyCode::Char(',')),
        D16Command::None
    ));
    assert_eq!(model.page(), D16Page::Projects);
    assert!(matches!(
        model.handle_key(KeyCode::Char(',')),
        D16Command::None
    ));
    assert_eq!(model.page(), D16Page::Workstreams);
    assert!(matches!(
        model.handle_key(KeyCode::Char('.')),
        D16Command::None
    ));
    assert_eq!(model.page(), D16Page::Archived);
    assert!(matches!(
        model.handle_key(KeyCode::Char('.')),
        D16Command::None
    ));
    assert_eq!(model.page(), D16Page::Workstreams);
    assert!(matches!(model.handle_key(KeyCode::Left), D16Command::None));
    assert!(matches!(model.handle_key(KeyCode::Right), D16Command::None));
    assert_eq!(model.page(), D16Page::Workstreams);
    model.select_page(D16Page::Archived);
    assert!(matches!(model.handle_key(KeyCode::Esc), D16Command::None));
    assert_eq!(model.page(), D16Page::Workstreams);
}

#[test]
fn project_headers_are_display_only_and_locations_are_exact_targets() {
    let mut input = snapshot();
    input.projects[0].locations.push(LocationSnapshot {
        project_id: id(0xA),
        location_id: id(0xA2),
        display_name: "alpha secondary".to_owned(),
        revision: revision(14),
        repository_fingerprint: None,
        origin_display: None,
        is_label_source: false,
    });
    let mut model = D16Model::new(input);
    model.select_page(D16Page::Projects);
    let rows = model.rows();
    assert!(matches!(rows[0], D16Row::ProjectHeader(_)));
    assert!(!rows[0].is_actionable());
    assert_eq!(model.selected_id(), Some(D16RowId::Location(id(0xA1))));
    let command = model.handle_key(KeyCode::Char('n'));
    assert!(command.is_none());
    assert_eq!(
        model
            .provider_chooser()
            .map(|chooser| chooser.providers.clone()),
        Some(vec![ProviderKind::Codex, ProviderKind::OpenCode])
    );
    model.handle_key(KeyCode::Down);
    let command = model.handle_key(KeyCode::Enter);
    assert!(matches!(
        command,
        D16Command::Apply(ApplicationAction::NewAtLocation {
            project_id,
            location_id,
            expected_project_revision,
            expected_location_revision,
            provider: ProviderKind::OpenCode,
        }) if project_id == id(0xA)
            && location_id == id(0xA1)
            && expected_project_revision == revision(7)
            && expected_location_revision == revision(8)
    ));
}

#[test]
fn active_n_is_same_location_fast_path_and_enter_only_emits_attach() {
    let mut model = D16Model::new(snapshot());
    let active_id: WorkstreamId = id(0xA11);
    assert_eq!(model.selected_id(), Some(D16RowId::Workstream(active_id)));
    let command = model.handle_key(KeyCode::Char('n'));
    assert!(command.is_none());
    assert_eq!(
        model.provider_chooser().map(|chooser| chooser.selected),
        Some(0)
    );
    let command = model.handle_key(KeyCode::Enter);
    assert!(matches!(
        command,
        D16Command::Apply(ApplicationAction::NewAtSameLocation {
            source_workstream_id,
            expected_workstream_revision,
            provider: ProviderKind::Codex,
        }) if source_workstream_id == active_id && expected_workstream_revision == revision(3)
    ));
    let attach = D16Model::new(snapshot()).handle_key(KeyCode::Enter);
    assert!(matches!(
        attach,
        D16Command::Attach(AttachEvidence {
            workstream_id,
            runtime_id,
            expected_workstream_revision,
            expected_runtime_revision,
        }) if workstream_id == active_id
            && runtime_id == id(0xA101)
            && expected_workstream_revision == revision(3)
            && expected_runtime_revision == revision(4)
    ));
}

#[test]
fn restore_success_returns_to_workstreams_without_attachment() {
    let archived_id: WorkstreamId = id(0xB11);
    let mut model = D16Model::new(snapshot());
    model.select_page(D16Page::Archived);
    model.select_next();
    assert_eq!(model.selected_id(), Some(D16RowId::Workstream(archived_id)));
    let command = model.handle_key(KeyCode::Char('u'));
    assert!(matches!(
        command,
        D16Command::Apply(ApplicationAction::Restore { workstream_id, expected_revision })
            if workstream_id == archived_id && expected_revision == revision(6)
    ));
    model.accept_outcome(ApplicationOutcome::Applied {
        identity: RevisedIdentity::Workstream(archived_id, revision(7)),
    });
    assert_eq!(model.page(), D16Page::Workstreams);
    assert_eq!(model.selected_id(), Some(D16RowId::Workstream(archived_id)));
    assert!(matches!(model.handle_key(KeyCode::Enter), D16Command::None));
}

#[test]
fn observer_guide_is_operation_local_and_does_not_create_a_page() {
    let mut model = D16Model::new(snapshot());
    let command = model.handle_key(KeyCode::Char('n'));
    assert!(command.is_none());
    let command = model.handle_key(KeyCode::Enter);
    assert!(matches!(command, D16Command::Apply(_)));
    let guide = ObserverReadinessGuide {
        evidence: ObserverReadinessEvidence {
            readiness: ObserverReadiness::SetupRequired,
            integration_revision: None,
        },
        intent: ObserverIntent::NewAtSameLocation {
            source_workstream_id: id(0xA11),
            expected_workstream_revision: revision(3),
            provider: ProviderKind::Codex,
        },
        explicit_interactive_consent_required: true,
        native_trust_review_required: true,
    };
    model.accept_outcome(ApplicationOutcome::ObserverReadinessRequired(guide));
    assert_eq!(model.page(), D16Page::Workstreams);
    assert_eq!(model.observer_guide(), Some(guide));
    model.dismiss_observer_guide();
    assert_eq!(model.observer_guide(), None);
}

#[test]
fn browser_horizontal_navigation_is_modal_local_and_passive() {
    let mut model = D16Model::new(snapshot());
    model.select_page(D16Page::Projects);
    let command = model.handle_key(KeyCode::Char('a'));
    assert!(matches!(
        command,
        D16Command::Apply(ApplicationAction::ListProjectBrowser {
            relative_path,
            include_hidden: false,
        }) if relative_path == BrowserPath::root()
    ));
    model.accept_outcome(ApplicationOutcome::BrowserListed(BrowserListing {
        relative_path: BrowserPath::root(),
        include_hidden: false,
        entries: vec![
            BrowserEntry {
                name: "repo".to_owned(),
                is_git_repository: true,
            },
            BrowserEntry {
                name: "nested".to_owned(),
                is_git_repository: false,
            },
        ],
        root_label: "workspace".to_owned(),
        revision: revision(12),
    }));
    model.handle_key(KeyCode::Down);
    let child = model.handle_key(KeyCode::Right);
    assert!(matches!(
        child,
        D16Command::Apply(ApplicationAction::ListProjectBrowser {
            relative_path,
            include_hidden: false,
        }) if relative_path.as_str() == "nested"
    ));
    model.accept_outcome(ApplicationOutcome::BrowserListed(BrowserListing {
        relative_path: BrowserPath::new("nested").unwrap(),
        include_hidden: false,
        entries: Vec::new(),
        root_label: "workspace".to_owned(),
        revision: revision(13),
    }));
    let parent = model.handle_key(KeyCode::Left);
    assert!(matches!(
        parent,
        D16Command::Apply(ApplicationAction::ListProjectBrowser {
            relative_path,
            include_hidden: false,
        }) if relative_path == BrowserPath::root()
    ));
    assert!(model.browser().is_some());
    assert!(matches!(model.handle_key(KeyCode::Esc), D16Command::None));
    assert!(model.browser().is_none());
}

#[test]
fn browser_letters_filter_including_j_and_k_instead_of_moving() {
    let mut model = D16Model::new(snapshot());
    model.select_page(D16Page::Projects);
    model.handle_key(KeyCode::Char('a'));
    model.accept_outcome(ApplicationOutcome::BrowserListed(BrowserListing {
        relative_path: BrowserPath::root(),
        include_hidden: false,
        entries: vec![
            BrowserEntry {
                name: "alpha".to_owned(),
                is_git_repository: true,
            },
            BrowserEntry {
                name: "jupiter".to_owned(),
                is_git_repository: true,
            },
        ],
        root_label: "workspace".to_owned(),
        revision: revision(12),
    }));
    assert!(model.handle_key(KeyCode::Char('j')).is_none());
    assert_eq!(
        model.browser().map(|browser| browser.filter.as_str()),
        Some("j")
    );
    assert!(model.handle_key(KeyCode::Enter).is_none());
    assert!(matches!(
        model.provider_chooser().map(|chooser| &chooser.request),
        Some(wsnav::navigator::D16ProviderRequest::RegisterLocation {
            relative_path,
            ..
        }) if relative_path.as_str() == "jupiter"
    ));
}

#[test]
fn rendering_removes_banner_but_keeps_page_title_in_border() {
    let mut navigator = D16Navigator::new(snapshot());
    for page_key in [None, Some(KeyCode::Char(',')), Some(KeyCode::Char('.'))] {
        if let Some(key) = page_key {
            navigator.handle_key(key);
        }
        let rendered = rendered_text(&render_buffer(&navigator, Some(1_000_000)));
        assert!(rendered.contains(navigator.model().page().title()));
        assert!(!rendered.contains("host: devbox"));
        assert!(!rendered.contains("active Workstreams grouped by"));
    }
}

#[test]
fn stale_or_header_selection_never_remains_cursor_authority() {
    let mut model = D16Model::new(snapshot());
    assert_eq!(model.selected_id(), Some(D16RowId::Workstream(id(0xA11))));
    let mut replacement = snapshot();
    replacement.active_project_groups.clear();
    replacement.archived_project_groups.clear();
    replacement.unresolved_operations.clear();
    model.replace_snapshot(replacement);
    assert_eq!(model.selected_id(), None);
    assert!(model.rows().is_empty());
}

#[test]
fn empty_workstreams_new_routes_to_locations_or_registration_browser() {
    let mut with_locations = snapshot();
    with_locations.active_project_groups.clear();
    with_locations.unresolved_operations.clear();
    let mut model = D16Model::new(with_locations);
    assert!(model.handle_key(KeyCode::Char('n')).is_none());
    assert_eq!(model.page(), D16Page::Projects);
    assert!(model.browser().is_none());

    let mut without_locations = snapshot();
    without_locations.active_project_groups.clear();
    without_locations.archived_project_groups.clear();
    without_locations.unresolved_operations.clear();
    for project in &mut without_locations.projects {
        project.locations.clear();
    }
    let mut model = D16Model::new(without_locations);
    assert!(matches!(
        model.handle_key(KeyCode::Char('n')),
        D16Command::Apply(ApplicationAction::ListProjectBrowser {
            relative_path,
            include_hidden: false,
        }) if relative_path == BrowserPath::root()
    ));
    assert_eq!(model.page(), D16Page::Projects);
    assert!(model.browser().is_some());
}

#[test]
fn registration_uses_the_same_process_local_provider_chooser() {
    let mut model = D16Model::new(snapshot());
    model.select_page(D16Page::Projects);
    model.handle_key(KeyCode::Char('a'));
    model.accept_outcome(ApplicationOutcome::BrowserListed(BrowserListing {
        root_label: "workspace".to_owned(),
        relative_path: BrowserPath::root(),
        include_hidden: false,
        entries: vec![BrowserEntry {
            name: "repo".to_owned(),
            is_git_repository: true,
        }],
        revision: revision(14),
    }));
    assert!(model.handle_key(KeyCode::Enter).is_none());
    assert_eq!(
        model.provider_chooser().map(|chooser| chooser.selected),
        Some(0)
    );
    let action = model.handle_key(KeyCode::Enter);
    assert!(matches!(
        action,
        D16Command::Apply(ApplicationAction::RegisterLocation {
            relative_path,
            expected_browser_revision,
            provider: ProviderKind::Codex,
        }) if relative_path.as_str() == "repo" && expected_browser_revision == revision(14)
    ));
}

#[test]
fn no_eligible_provider_is_a_bounded_refusal_and_never_silent_fallback() {
    let mut input = snapshot();
    for capability in &mut input.provider_capabilities {
        capability.status = ProviderCapabilityStatus::Unavailable;
        capability.fresh_launch = false;
        capability.exact_resume = false;
        capability.observe = false;
    }
    let mut model = D16Model::new(input);
    assert!(model.handle_key(KeyCode::Char('n')).is_none());
    assert!(model.message().is_some());
    assert!(model.provider_chooser().is_none());
}

#[test]
fn recovery_uses_the_operation_provider_not_a_capability_guess() {
    let mut input = snapshot();
    input.unresolved_operations[0].provider = ProviderKind::OpenCode;
    input.provider_capabilities[0].status = ProviderCapabilityStatus::Unavailable;
    input.provider_capabilities[0].exact_resume = false;
    input.provider_capabilities[0].observe = false;
    let mut model = D16Model::new(input);
    model.select_next();
    assert!(matches!(
        model.handle_key(KeyCode::Char('r')),
        D16Command::Apply(ApplicationAction::RecoverOperation {
            operation_id,
            provider: ProviderKind::OpenCode,
            ..
        }) if operation_id == id(0xC1)
    ));
}

#[test]
fn observer_accept_command_carries_exact_evidence_and_stays_operation_local() {
    let mut model = D16Model::new(snapshot());
    let guide = ObserverReadinessGuide {
        evidence: ObserverReadinessEvidence {
            readiness: ObserverReadiness::TrustReviewRequired,
            integration_revision: Some(revision(17)),
        },
        intent: ObserverIntent::Start {
            workstream_id: id(0xA11),
            expected_revision: revision(3),
            provider: ProviderKind::Codex,
        },
        explicit_interactive_consent_required: true,
        native_trust_review_required: true,
    };
    model.accept_outcome(ApplicationOutcome::ObserverReadinessRequired(guide));
    assert!(matches!(
        model.accept_observer_guide(),
        D16Command::AcceptObserverGuide(received) if received == guide
    ));
    assert_eq!(model.page(), D16Page::Workstreams);
    assert!(matches!(
        model.handle_key(KeyCode::Enter),
        D16Command::AcceptObserverGuide(received) if received == guide
    ));
}

#[test]
fn bounded_main_and_browser_scroll_keep_selection_visible() {
    let mut input = snapshot();
    let base = input.active_project_groups[0].workstreams[0].clone();
    input.active_project_groups[0].workstreams = (0..18)
        .map(|index| {
            let mut row = base.clone();
            row.workstream_id = id(0xA11 + index);
            row.last_activity_sequence = i64::try_from(index).expect("bounded index");
            row
        })
        .collect();
    let mut model = D16Model::new(input);
    for _ in 0..14 {
        model.select_next();
    }
    assert!(model.scroll() > 0);
    let (_, visible) = model.visible_rows(3);
    assert!(visible.iter().any(|row| row.id() == model.selected_id()));

    model.select_page(D16Page::Projects);
    model.open_project_browser();
    model.accept_outcome(ApplicationOutcome::BrowserListed(BrowserListing {
        root_label: "workspace".to_owned(),
        relative_path: BrowserPath::root(),
        include_hidden: false,
        entries: (0..24)
            .map(|index| BrowserEntry {
                name: format!("repo-{index}"),
                is_git_repository: true,
            })
            .collect(),
        revision: revision(18),
    }));
    for _ in 0..18 {
        model.handle_key(KeyCode::Down);
    }
    let browser = model.browser().expect("browser remains open");
    assert!(browser.scroll > 0);
    assert!(browser.selected < browser.scroll + 10);
}

#[test]
fn help_is_process_local_modal_and_direct_actions_are_inert_while_open() {
    let mut model = D16Model::new(snapshot());
    assert!(model.handle_key(KeyCode::Char('?')).is_none());
    assert!(model.help_visible());
    assert!(model.handle_key(KeyCode::Char(',')).is_none());
    assert_eq!(model.page(), D16Page::Workstreams);
    assert!(model.handle_key(KeyCode::Char('n')).is_none());
    assert!(model.help_visible());
    model.handle_key(KeyCode::Esc);
    assert!(!model.help_visible());
}

#[test]
fn archive_requires_action_local_confirmation() {
    let mut model = D16Model::new(snapshot());
    assert!(model.handle_key(KeyCode::Char('x')).is_none());
    assert!(matches!(
        model.modal(),
        Some(D16Modal::ConfirmArchive {
            workstream_id,
            expected_revision,
        }) if *workstream_id == id(0xA11) && *expected_revision == revision(3)
    ));
    assert!(model.handle_key(KeyCode::Char(',')).is_none());
    assert_eq!(model.page(), D16Page::Workstreams);
    assert!(matches!(
        model.handle_key(KeyCode::Enter),
        D16Command::Apply(ApplicationAction::Archive {
            workstream_id,
            expected_revision,
        }) if workstream_id == id(0xA11) && expected_revision == revision(3)
    ));
    assert!(model.modal().is_none());
}

#[test]
fn rename_and_browser_root_are_bounded_action_specific_forms() {
    let mut model = D16Model::new(snapshot());
    assert!(model.handle_key(KeyCode::Char('r')).is_none());
    assert!(matches!(model.modal(), Some(D16Modal::Rename { .. })));
    for _ in 0.."active native name".chars().count() {
        model.handle_key(KeyCode::Backspace);
    }
    for character in "new native name".chars() {
        model.handle_key(KeyCode::Char(character));
    }
    assert!(matches!(
        model.handle_key(KeyCode::Enter),
        D16Command::Apply(ApplicationAction::Rename {
            workstream_id,
            expected_revision,
            name,
        }) if workstream_id == id(0xA11)
            && expected_revision == revision(3)
            && name == "new native name"
    ));

    model.select_page(D16Page::Projects);
    assert!(model.handle_key(KeyCode::Char('b')).is_none());
    for character in "/srv/work".chars() {
        model.handle_key(KeyCode::Char(character));
    }
    assert!(matches!(
        model.handle_key(KeyCode::Enter),
        D16Command::Apply(ApplicationAction::SetProjectBrowserRoot {
            root_path,
            expected_revision,
        }) if root_path.as_str() == "/srv/work" && expected_revision == revision(13)
    ));
}

#[test]
fn workstreams_render_name_only_projects_and_full_width_minimal_cards() {
    let navigator = D16Navigator::new(snapshot());
    let buffer = render_buffer(&navigator, Some(1_000_000));
    let rendered = rendered_text(&buffer);
    let project_line = rendered_row_segment(&buffer, 1, 1, 88);
    let context_line = rendered_row_segment(&buffer, 2, 1, 88);
    let title_line = rendered_row_segment(&buffer, 3, 1, 88);

    assert_eq!(project_line.trim_end(), "alpha");
    assert!(context_line.starts_with("└ Codex"));
    assert!(context_line.ends_with("now"));
    assert!(title_line.starts_with("  ✓ active native name"));
    assert!(!title_line.contains("now"));
    assert!(!rendered.contains("devbox"));
    assert!(!context_line.starts_with("└─"));

    let (age_x, age_y, _) = first_position_for_text(&buffer, "now").expect("activity age");
    assert_eq!(age_y, 2);
    assert_eq!(age_x + 3, 89, "age ends at the inner right edge");
}

#[test]
fn projects_flatten_single_checkouts_and_keep_multi_checkout_trees() {
    let mut single = snapshot();
    single.projects[0].display_name = "cubey".to_owned();
    single.projects[0].locations[0].display_name = "cubey".to_owned();
    let mut single_navigator = D16Navigator::new(single);
    single_navigator.model_mut().select_page(D16Page::Projects);
    let single_rows = single_navigator.model().rows();
    assert!(matches!(single_rows[0], D16Row::Location(_)));
    let single_rendered = render_buffer(&single_navigator, Some(1_000_000));
    assert_eq!(
        rendered_row_segment(&single_rendered, 1, 1, 88).trim_end(),
        "cubey"
    );
    let single_text = rendered_text(&single_rendered);
    assert_eq!(single_text.matches("cubey").count(), 1);
    assert!(!single_text.contains("Location "));
    assert!(!single_text.contains("[label]"));

    let mut multiple = snapshot();
    multiple.projects[0].locations.push(LocationSnapshot {
        project_id: id(0xA),
        location_id: id(0xA2),
        display_name: "alpha secondary".to_owned(),
        revision: revision(14),
        repository_fingerprint: None,
        origin_display: None,
        is_label_source: false,
    });
    let mut multiple_navigator = D16Navigator::new(multiple);
    multiple_navigator
        .model_mut()
        .select_page(D16Page::Projects);
    let multiple_rendered = render_buffer(&multiple_navigator, Some(1_000_000));
    assert_eq!(
        rendered_row_segment(&multiple_rendered, 1, 1, 88).trim_end(),
        "alpha"
    );
    assert_eq!(
        rendered_row_segment(&multiple_rendered, 2, 1, 88).trim_end(),
        "├ alpha checkout"
    );
    assert_eq!(
        rendered_row_segment(&multiple_rendered, 3, 1, 88).trim_end(),
        "└ alpha secondary"
    );
}

#[test]
fn narrow_footer_packs_complete_hints_on_whole_lines() {
    let navigator = D16Navigator::new(snapshot());
    let footer = render_buffer_with_area(&navigator, Some(1_000_000), 32, 18);
    for hint in [
        "↑↓ select",
        "n new",
        "f fork",
        "p park",
        "r rename",
        "x archive",
        ", projects",
        ". archived",
        "? help",
    ] {
        assert!(
            first_cell_for_text(&footer, hint).is_some(),
            "complete footer hint {hint:?}"
        );
    }
    let (_, first_row, _) = first_position_for_text(&footer, "↑↓ select").expect("first hint");
    let (_, last_row, _) = first_position_for_text(&footer, "? help").expect("last hint");
    assert!(
        last_row > first_row,
        "narrow footer uses multiple packed lines"
    );
}

#[test]
fn unnamed_workstream_uses_only_its_stable_short_id() {
    let mut input = snapshot();
    let workstream = &mut input.active_project_groups[0].workstreams[0];
    workstream.native_name = None;
    let short_id = workstream.workstream_id.short();

    let rendered = rendered_text(&render_buffer(&D16Navigator::new(input), Some(1_000_000)));
    assert!(rendered.contains(&format!("✓ {short_id}")));
    assert!(!rendered.contains("Workstream "));
}

#[test]
fn rendering_restores_semantic_colors_and_selection_only_changes_background() {
    let navigator = D16Navigator::new(snapshot());
    let selected = render_buffer(&navigator, Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&selected, "Workstreams")
            .expect("page title")
            .fg,
        Color::Cyan
    );
    assert_eq!(selected.cell((0, 0)).expect("top border").fg, Color::Cyan);
    assert_eq!(
        first_cell_for_text(&selected, "Codex")
            .expect("Codex context")
            .fg,
        Color::Indexed(209)
    );
    assert_eq!(
        first_cell_for_text(&selected, "active native name")
            .expect("Workstream title")
            .fg,
        Color::White
    );
    assert_eq!(
        first_cell_for_text(&selected, "✓")
            .expect("attention indicator")
            .fg,
        Color::Green
    );
    assert_eq!(
        first_cell_for_text(&selected, "now")
            .expect("activity age")
            .fg,
        Color::Indexed(255)
    );
    assert_eq!(
        first_cell_for_text(&selected, "└").expect("tree branch").fg,
        Color::Indexed(245)
    );
    assert_eq!(
        first_cell_for_text(&selected, "n new")
            .expect("key hint")
            .fg,
        Color::Yellow
    );
    let selected_codex = first_cell_for_text(&selected, "Codex").expect("selected Codex");
    assert_eq!(selected_codex.bg, Color::Indexed(236));

    let mut unselected_navigator = D16Navigator::new(snapshot());
    unselected_navigator.model_mut().select_next();
    let unselected = render_buffer(&unselected_navigator, Some(1_000_000));
    let unselected_codex = first_cell_for_text(&unselected, "Codex").expect("unselected Codex");
    assert_eq!(unselected_codex.fg, selected_codex.fg);
    assert_eq!(unselected_codex.modifier, selected_codex.modifier);
    assert_ne!(unselected_codex.bg, selected_codex.bg);
    for needle in ["✓", "active native name", "now", "└"] {
        let selected_cell = first_cell_for_text(&selected, needle).expect("selected semantic cell");
        let unselected_cell =
            first_cell_for_text(&unselected, needle).expect("unselected semantic cell");
        assert_eq!(
            selected_cell.fg, unselected_cell.fg,
            "foreground for {needle}"
        );
        assert_eq!(
            selected_cell.modifier, unselected_cell.modifier,
            "modifier for {needle}"
        );
        assert_eq!(
            selected_cell.bg,
            Color::Indexed(236),
            "selection background for {needle}"
        );
    }

    let mut project_navigator = D16Navigator::new(snapshot());
    project_navigator.model_mut().select_page(D16Page::Projects);
    let selected_project = render_buffer(&project_navigator, Some(1_000_000));
    project_navigator.model_mut().select_next();
    let unselected_project = render_buffer(&project_navigator, Some(1_000_000));
    let selected_project_cell =
        first_cell_for_text(&selected_project, "alpha checkout").expect("selected Project");
    let unselected_project_cell =
        first_cell_for_text(&unselected_project, "alpha checkout").expect("unselected Project");
    assert_eq!(selected_project_cell.fg, unselected_project_cell.fg);
    assert_eq!(
        selected_project_cell.modifier,
        unselected_project_cell.modifier
    );
    assert_eq!(selected_project_cell.bg, Color::Indexed(236));
    assert_ne!(unselected_project_cell.bg, selected_project_cell.bg);

    let mut status_navigator = D16Navigator::new(snapshot());
    status_navigator
        .model_mut()
        .set_message("bounded status for style proof");
    let status = render_buffer(&status_navigator, Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&status, "Status")
            .expect("status title")
            .fg,
        Color::Yellow
    );
    assert_eq!(
        status.cell((0, 14)).expect("status border").fg,
        Color::Yellow
    );
}

#[test]
fn narrow_status_and_help_preserve_wrapped_words() {
    let mut status_navigator = D16Navigator::new(snapshot());
    status_navigator
        .model_mut()
        .set_message("attachment status is unavailable; native helper was not assumed successful");
    let status = render_buffer_with_area(&status_navigator, Some(1_000_000), 32, 18);
    assert!(
        rendered_text(&status).contains("successful"),
        "the complete wrapped status remains visible"
    );

    let mut help_navigator = D16Navigator::new(snapshot());
    help_navigator.handle_key(KeyCode::Char('?'));
    for _ in 0..6 {
        help_navigator.handle_key(KeyCode::Down);
    }
    let help = render_buffer_with_area(&help_navigator, Some(1_000_000), 32, 18);
    assert!(
        first_cell_for_text(&help, "attention").is_some(),
        "the compact attention action remains visible"
    );
    assert_eq!(
        first_cell_for_text(&help, "Workstreams keys")
            .expect("styled help title")
            .fg,
        Color::Cyan
    );
    assert_eq!(
        first_cell_for_text(&help, "Enter")
            .expect("styled help key")
            .fg,
        Color::Yellow
    );
    assert_eq!(
        first_cell_for_text(&help, "Open")
            .expect("styled help action")
            .fg,
        Color::Green
    );
}

#[test]
fn project_colors_are_collision_resolved_over_the_actual_scrolled_window() {
    let mut navigator = D16Navigator::new(project_catalog_snapshot(15));
    navigator.model_mut().select_page(D16Page::Projects);
    for _ in 0..12 {
        navigator.model_mut().select_next();
    }
    let area = Rect::new(0, 0, 120, 15);
    let geometry = navigator.list_geometry(area);
    let (_, visible) = navigator.model().visible_rows(geometry.viewport_rows);
    let visible_project_ids = visible
        .iter()
        .filter_map(|row| match row {
            D16Row::ProjectHeader(row) => Some(row.project_id),
            D16Row::Location(row) => Some(row.project_id),
            D16Row::Workstream(row) => Some(row.workstream.project_id),
            D16Row::Operation(_) => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(visible_project_ids.len(), 12);
    assert!(!visible_project_ids.contains(&id(0x1_000)));

    let rendered = render_buffer_with_area(&navigator, Some(1_000_000), area.width, area.height);
    let rerendered = render_buffer_with_area(&navigator, Some(1_000_000), area.width, area.height);
    let colors = visible_project_ids
        .iter()
        .map(|project_id| {
            let index = usize::try_from(project_id.as_uuid().as_u128() - 0x1_000)
                .expect("fixture project index");
            let label = format!("location-{index:02}");
            (
                project_id,
                first_cell_for_text(&rendered, &label)
                    .expect("visible Project label")
                    .fg,
            )
        })
        .collect::<Vec<_>>();
    let color_values = colors.iter().map(|(_, color)| *color).collect::<Vec<_>>();
    let unique = color_values
        .iter()
        .enumerate()
        .filter(|(index, color)| !color_values[..*index].contains(color))
        .count();
    assert_eq!(unique, 12);
    assert!(color_values.iter().all(|color| matches!(
        color,
        Color::Indexed(96 | 97 | 98 | 133 | 134 | 139 | 140 | 141 | 146 | 147 | 176 | 177)
    )));

    let first_project = visible_project_ids.iter().next().expect("visible Project");
    let first_index = usize::try_from(first_project.as_uuid().as_u128() - 0x1_000)
        .expect("fixture project index");
    let location = first_cell_for_text(&rendered, &format!("location-{first_index:02}"))
        .expect("flattened Location");
    assert_ne!(location.fg, Color::Reset);
    for project_id in visible_project_ids {
        let index = usize::try_from(project_id.as_uuid().as_u128() - 0x1_000)
            .expect("fixture project index");
        let label = format!("location-{index:02}");
        assert_eq!(
            first_cell_for_text(&rendered, &label)
                .expect("first stable Project label")
                .fg,
            first_cell_for_text(&rerendered, &label)
                .expect("second stable Project label")
                .fg
        );
    }
}

#[test]
fn provider_status_and_activity_age_palettes_remain_distinct_and_deterministic() {
    let mut opencode = snapshot();
    opencode.active_project_groups[0].workstreams[0].provider = ProviderKind::OpenCode;
    let opencode_rendered = render_buffer(&D16Navigator::new(opencode), Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&opencode_rendered, "OpenCode")
            .expect("OpenCode provider")
            .fg,
        Color::Indexed(80)
    );

    let mut statuses = snapshot();
    let workstream = &mut statuses.active_project_groups[0].workstreams[0];
    workstream.attention = AttentionSnapshot {
        result_unseen: false,
        recovery_unseen: false,
        revision: Revision::INITIAL,
    };
    workstream.lifecycle = WorkstreamLifecycle::Open;
    workstream.runtime = Some(RuntimeSnapshot {
        runtime_id: id(0xA101),
        status: RuntimeStatus::Working,
        revision: Revision::INITIAL,
        observer_degraded: false,
    });
    let working = render_buffer(&D16Navigator::new(statuses.clone()), Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&working, "●")
            .expect("working marker")
            .fg,
        Color::Yellow
    );

    statuses.active_project_groups[0].workstreams[0].lifecycle = WorkstreamLifecycle::Parked;
    statuses.active_project_groups[0].workstreams[0].runtime = None;
    statuses.active_project_groups[0].workstreams[0]
        .attention
        .result_unseen = true;
    statuses.active_project_groups[0].workstreams[0]
        .attention
        .recovery_unseen = true;
    let parked = render_buffer(&D16Navigator::new(statuses.clone()), Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&parked, "p active native name")
            .expect("parked marker")
            .fg,
        Color::Indexed(110)
    );

    statuses.active_project_groups[0].workstreams[0].lifecycle =
        WorkstreamLifecycle::RecoveryRequired;
    let recovery = render_buffer(&D16Navigator::new(statuses.clone()), Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&recovery, "! active native name")
            .expect("recovery marker")
            .fg,
        Color::Red
    );

    statuses.active_project_groups[0].workstreams[0].lifecycle = WorkstreamLifecycle::Open;
    statuses.active_project_groups[0].workstreams[0]
        .attention
        .recovery_unseen = false;
    statuses.active_project_groups[0].workstreams[0]
        .attention
        .result_unseen = true;
    let attention = render_buffer(&D16Navigator::new(statuses), Some(1_000_000));
    assert_eq!(
        first_cell_for_text(&attention, "✓ active native name")
            .expect("attention marker")
            .fg,
        Color::Green
    );

    let now_millis = 1_000_000_000;
    let buckets = [
        (None, "activity unknown", Color::Indexed(244)),
        (Some(1_000_000_000), "now", Color::Indexed(255)),
        (Some(999_940_000), "1 min ago", Color::Indexed(251)),
        (Some(996_400_000), "1 hr ago", Color::Indexed(247)),
        (Some(913_600_000), "1 day ago", Color::Indexed(244)),
        (Some(395_200_000), "7 days ago", Color::Indexed(241)),
    ];
    for (timestamp, label, color) in buckets {
        let mut input = snapshot();
        input.active_project_groups[0].workstreams[0].last_activity_at_millis = timestamp;
        let rendered = render_buffer(&D16Navigator::new(input), Some(now_millis));
        assert_eq!(
            first_cell_for_text(&rendered, label)
                .expect("activity age")
                .fg,
            color,
            "activity bucket {label}"
        );
    }
}

#[test]
fn list_geometry_reclaims_banner_rows_without_changing_mouse_targets() {
    let navigator = D16Navigator::new(snapshot());
    let area = Rect::new(0, 0, 90, 18);
    let geometry = navigator.list_geometry(area);
    assert_eq!(geometry.outer.y, area.y);
    assert_eq!(geometry.inner.y, area.y + 1);
    assert_eq!(
        navigator.row_at(area, geometry.inner.x, geometry.inner.y),
        None
    );
    let workstream = D16RowId::Workstream(id(0xA11));
    assert_eq!(
        navigator.row_at(area, geometry.inner.x, geometry.inner.y + 1),
        Some(workstream)
    );
    assert_eq!(
        navigator.row_at(area, geometry.inner.x, geometry.inner.y + 2),
        Some(workstream)
    );
}

#[test]
fn observer_degraded_runtime_renders_unknown_without_rewriting_durable_status() {
    let mut input = snapshot();
    let workstream = &mut input.active_project_groups[0].workstreams[0];
    workstream.attention.result_unseen = false;
    let runtime = workstream.runtime.as_mut().expect("fixture Runtime");
    assert_eq!(runtime.status, RuntimeStatus::Idle);
    runtime.observer_degraded = true;

    let navigator = D16Navigator::new(input);
    let backend = TestBackend::new(90, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| navigator.render(frame, Rect::new(0, 0, 90, 18)))
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("? active native name"));
}

#[test]
fn both_workstream_card_lines_resolve_to_one_primary_mouse_target() {
    let mut model = D16Model::new(snapshot());
    let workstream = D16RowId::Workstream(id(0xA11));
    // One non-actionable Project header precedes the two-line card.
    assert_eq!(model.row_id_at_render_line(10, 0), None);
    assert_eq!(model.row_id_at_render_line(10, 1), Some(workstream));
    assert_eq!(model.row_id_at_render_line(10, 2), Some(workstream));
    assert!(matches!(
        model.activate_row(workstream),
        D16Command::Attach(AttachEvidence { workstream_id, .. }) if workstream_id == id(0xA11)
    ));

    model.handle_key(KeyCode::Char('?'));
    assert!(model.activate_row(workstream).is_none());
}
