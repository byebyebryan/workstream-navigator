use ratatui::backend::TestBackend;
use ratatui::{
    Terminal,
    style::{Color, Style},
    text::Line,
};
use std::{
    cell::Cell,
    cmp::Ordering,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    build_info::BuildInfoError,
    domain::{
        AttentionState, HostId, LocationId, OperationId, OperationKind, OperationPhase, ProjectId,
        ProviderKind, Revision, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
    },
    presentation::{AttachmentPhase, AttachmentStatus},
    protocol::{
        ObserverStatus, ProjectDirectoriesResponse, ProjectDirectoryEntry, ProviderCapability,
        SnapshotResponse,
    },
    state::{ClientCatalog, HostRegistry, StateRoot},
};

use super::{controller::*, model::*, render::*, snapshot::*, view::*};

#[test]
fn local_snapshot_projects_durable_runtime_state_without_a_tmux_probe() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut registry = HostRegistry::open(&root).unwrap();
    let registered = registry
        .register_project_root(&project, crate::domain::ProviderKind::Codex)
        .unwrap();
    let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
    registry
        .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 42, "birth-a")
        .unwrap();
    drop(registry);

    let snapshot = local_snapshot(&root).unwrap();

    assert_eq!(snapshot.workstreams.len(), 1);
    assert_eq!(
        snapshot.workstreams[0].runtime_status,
        NavigatorRuntimeStatus::Starting
    );
}

#[test]
fn selection_wraps_without_persisting_focus() {
    let first = WorkstreamId::new();
    let second = WorkstreamId::new();
    let snapshot = LocalNavigatorSnapshot {
        workstreams: vec![
            row(first, NavigatorRuntimeStatus::Idle),
            row(second, NavigatorRuntimeStatus::Parked),
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    };
    let mut view = NavigatorView::new(snapshot);
    view.select_previous();
    assert_eq!(view.selected().map(|row| row.workstream_id), Some(second));
    view.select_next();
    assert_eq!(view.selected().map(|row| row.workstream_id), Some(first));
}

#[test]
fn archived_view_filters_rows_and_keeps_selection_visible() {
    let active_id = WorkstreamId::new();
    let archived_id = WorkstreamId::new();
    let mut archived = row(archived_id, NavigatorRuntimeStatus::Parked);
    archived.archived = true;
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(active_id, NavigatorRuntimeStatus::Idle), archived],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    assert_eq!(
        view.selected().map(|row| row.workstream_id),
        Some(active_id)
    );
    assert_eq!(
        view.list_entries()
            .iter()
            .filter_map(NavigatorListEntry::workstream_index)
            .collect::<Vec<_>>(),
        vec![0]
    );

    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Project);
    assert_eq!(
        view.list_entries()
            .iter()
            .filter_map(NavigatorListEntry::workstream_index)
            .collect::<Vec<_>>(),
        vec![0]
    );
    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Host);
    assert_eq!(
        view.list_entries()
            .iter()
            .filter_map(NavigatorListEntry::workstream_index)
            .collect::<Vec<_>>(),
        vec![0]
    );
    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Archived);
    assert_eq!(
        view.selected().map(|row| row.workstream_id),
        Some(archived_id)
    );
    assert_eq!(
        view.list_entries()
            .iter()
            .filter_map(NavigatorListEntry::workstream_index)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert!(matches!(
        view.list_entries().as_slice(),
        [NavigatorListEntry::Workstream {
            context: WorkstreamRowContext::Archived,
            ..
        }]
    ));
    assert_eq!(view.list_entries()[0].height(), 2);

    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Recent);
    assert_eq!(
        view.selected().map(|row| row.workstream_id),
        Some(active_id)
    );
}

#[test]
fn archive_confirmation_is_local_to_the_navigator_pane() {
    let mut workstream = row(WorkstreamId::new(), NavigatorRuntimeStatus::Working);
    workstream.display_name = "important native work".to_owned();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream.clone()],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.begin_archive_confirmation(workstream);
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Archive working Workstream"));
    assert!(rendered.contains("important native work"));
    assert!(view.modal_visible());
    assert!(matches!(
        view.confirm_modal(),
        Some(NavigatorModal::ConfirmArchive(_))
    ));
    assert!(!view.modal_visible());
}

#[test]
fn workstream_status_detail_uses_only_bounded_navigator_metadata() {
    let workstream_id = WorkstreamId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(workstream_id, NavigatorRuntimeStatus::Working)],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.open_selected_detail();
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Workstream status"));
    assert!(rendered.contains("Provider: Codex"));
    assert!(rendered.contains("Runtime: working"));
    assert!(rendered.contains("Visibility: active"));
    assert!(!rendered.contains(&workstream_id.to_string()));
}

#[test]
fn workstream_detail_shows_the_full_open_code_provider_label() {
    let workstream_id = WorkstreamId::new();
    let workstream = NavigatorWorkstream {
        provider: ProviderKind::OpenCode,
        ..row(workstream_id, NavigatorRuntimeStatus::Idle)
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream],
        ..LocalNavigatorSnapshot::default()
    });
    view.open_selected_detail();
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Provider: OpenCode"));
}

#[test]
fn rename_modal_keeps_the_title_entry_inside_the_navigator() {
    let workstream = row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle);
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream.clone()],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.begin_rename(workstream);
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Rename Workstream"));
    assert!(rendered.contains("Set the canonical Codex thread title"));
    assert!(view.modal_visible());
}

#[test]
fn provider_known_modals_keep_open_code_feedback_exact() {
    let workstream = NavigatorWorkstream {
        provider: ProviderKind::OpenCode,
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Working)
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream.clone()],
        ..LocalNavigatorSnapshot::default()
    });

    view.begin_archive_confirmation(workstream.clone());
    let (_, archive_lines) = navigator_modal_content(view.modal.clone().unwrap(), 48);
    let archive = archive_lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert!(archive.contains("OpenCode Runtime"));

    view.dismiss_modal();
    view.begin_rename(workstream);
    let (_, rename_lines) = navigator_modal_content(view.modal.clone().unwrap(), 48);
    let rename = rename_lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert!(rename.contains("canonical OpenCode thread title"));

    assert_eq!(
        CreationAction::Independent.success_message(Some(ProviderKind::OpenCode)),
        "new OpenCode Workstream started; use the native OpenCode UI directly"
    );
}

#[test]
fn project_registration_uses_a_navigator_local_host_picker_then_a_path_free_browser() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: Vec::new(),
        hosts: vec![
            NavigatorHostOverview {
                alias: "local".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
            NavigatorHostOverview {
                alias: "snap".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
        ],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.begin_checkout_registration();
    view.select_registration_host_next();
    let Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) = view.confirm_modal()
    else {
        panic!("registration host picker should be active");
    };
    assert_eq!(hosts[selected].alias(), "snap");
    view.modal = Some(NavigatorModal::ProjectBrowser {
        host: hosts[selected].clone(),
        directories: ProjectDirectoriesResponse {
            root_label: "~/code".to_owned(),
            relative_path: String::new(),
            entries: vec![ProjectDirectoryEntry {
                name: "switchboard".to_owned(),
                is_git_repository: true,
            }],
        },
        selected: 0,
        scroll: 0,
        filter: String::new(),
    });
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Add Project"));
    assert!(rendered.contains("snap"));
    assert!(rendered.contains("~/code"));
    assert!(rendered.contains("switchboard"));
    assert!(!rendered.contains("/private/checkout"));
    assert!(!rendered.contains("provider pane"));
}

#[test]
fn project_browser_keeps_the_selected_directory_inside_its_viewport() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: Vec::new(),
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.modal = Some(NavigatorModal::ProjectBrowser {
        host: NavigatorHost::Local,
        directories: ProjectDirectoriesResponse {
            root_label: "~/code".to_owned(),
            relative_path: String::new(),
            entries: (0..12)
                .map(|index| ProjectDirectoryEntry {
                    name: format!("project-{index:02}"),
                    is_git_repository: false,
                })
                .collect(),
        },
        selected: 0,
        scroll: 0,
        filter: String::new(),
    });
    for _ in 0..10 {
        view.select_project_browser_next();
    }

    let Some(NavigatorModal::ProjectBrowser {
        selected, scroll, ..
    }) = view.modal.as_ref()
    else {
        panic!("project browser should remain open");
    };
    assert_eq!((*selected, *scroll), (10, 1));
    let (_, lines) = navigator_modal_content(view.modal.clone().unwrap(), 30);
    let rendered = lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert!(rendered.contains("> project-10"));
    assert!(!rendered.contains("project-00"));
}

#[test]
fn attachment_marker_is_exact_and_clears_after_park() {
    let first = WorkstreamId::new();
    let second = WorkstreamId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            row(first, NavigatorRuntimeStatus::Idle),
            row(second, NavigatorRuntimeStatus::Idle),
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    let first_row = view.selected().unwrap().clone();
    let second_row = view.snapshot.workstreams[1].clone();

    view.observe_attachment(&AttachmentStatus {
        attempt_id: uuid::Uuid::new_v4(),
        host_alias: first_row.host.alias().to_owned(),
        workstream_id: first_row.workstream_id,
        phase: AttachmentPhase::Running,
    });
    assert!(view.is_attached_to(&first_row));
    assert!(!view.is_attached_to(&second_row));

    view.replace_snapshot(LocalNavigatorSnapshot {
        workstreams: vec![
            row(first, NavigatorRuntimeStatus::Parked),
            row(second, NavigatorRuntimeStatus::Idle),
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    assert!(!view.is_attached_to(view.selected().unwrap()));
}

#[test]
fn unchanged_snapshot_does_not_request_a_navigator_redraw() {
    let snapshot = LocalNavigatorSnapshot {
        workstreams: vec![row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    };
    let mut view = NavigatorView::new(snapshot.clone());

    assert!(!view.replace_snapshot(snapshot));
}

#[test]
fn attached_confirmation_expires_without_detaching_the_provider() {
    let workstream_id = WorkstreamId::new();
    let workstream = row(workstream_id, NavigatorRuntimeStatus::Idle);
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream.clone()],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    view.observe_attachment(&AttachmentStatus {
        attempt_id: uuid::Uuid::new_v4(),
        host_alias: "local".to_owned(),
        workstream_id,
        phase: AttachmentPhase::Running,
    });
    assert_eq!(
        view.footer_status(),
        "Codex attached; use the native Codex UI directly"
    );
    assert!(view.is_attached_to(&workstream));

    assert!(view.expire_transient_message(
        Instant::now() + ATTACHMENT_READY_MESSAGE_DURATION + Duration::from_millis(1)
    ));
    assert_eq!(view.footer_status(), "");
    assert!(view.is_attached_to(&workstream));
}

#[test]
fn attachment_feedback_uses_the_resolved_open_code_provider_label() {
    let workstream_id = WorkstreamId::new();
    let workstream = NavigatorWorkstream {
        provider: ProviderKind::OpenCode,
        ..row(workstream_id, NavigatorRuntimeStatus::Idle)
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream],
        ..LocalNavigatorSnapshot::default()
    });

    view.observe_attachment(&AttachmentStatus {
        attempt_id: uuid::Uuid::new_v4(),
        host_alias: "local".to_owned(),
        workstream_id,
        phase: AttachmentPhase::Running,
    });

    assert_eq!(
        view.footer_status(),
        "OpenCode attached; use the native OpenCode UI directly"
    );
}

#[test]
fn terminal_attachment_outcome_allows_an_exact_same_row_retry() {
    let workstream_id = WorkstreamId::new();
    let workstream = row(workstream_id, NavigatorRuntimeStatus::Idle);
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![workstream.clone()],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    let attempt_id = uuid::Uuid::new_v4();
    let running = AttachmentStatus {
        attempt_id,
        host_alias: "local".to_owned(),
        workstream_id,
        phase: AttachmentPhase::Running,
    };

    view.observe_attachment(&running);
    assert!(view.is_attached_to(&workstream));
    view.replace_snapshot(LocalNavigatorSnapshot {
        workstreams: vec![row(workstream_id, NavigatorRuntimeStatus::Unknown)],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    assert!(!view.is_attached_to(&workstream));
    view.observe_attachment(&running);
    assert!(view.is_attached_to(&workstream));

    view.observe_attachment(&AttachmentStatus {
        attempt_id,
        host_alias: "local".to_owned(),
        workstream_id,
        phase: AttachmentPhase::Failed,
    });
    assert!(!view.is_attached_to(&workstream));
    assert_eq!(
        view.message.as_deref(),
        Some("attachment failed; press Enter or click row to retry")
    );
}

#[test]
fn mouse_click_retains_blank_focus_and_row_activation_intent() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    view.begin_mouse_click(None);
    assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Blank));

    view.begin_mouse_click(Some(0));
    assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Row));
    assert_eq!(view.take_mouse_click(), None);
}

#[test]
fn row_mapping_covers_all_three_recent_lines_of_each_workstream() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle),
            row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked),
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    assert_eq!(view.row_from_y(0), None);
    assert_eq!(view.row_from_y(1), Some(0));
    assert_eq!(view.row_from_y(2), Some(0));
    assert_eq!(view.row_from_y(3), Some(0));
    assert_eq!(view.row_from_y(4), Some(1));
    assert_eq!(view.row_from_y(5), Some(1));
    assert_eq!(view.row_from_y(6), Some(1));
    assert_eq!(view.row_from_y(7), None);
}

#[test]
fn mouse_row_mapping_includes_the_rendered_scroll_offset() {
    let workstreams = (0..6)
        .map(|_| row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle))
        .collect::<Vec<_>>();
    let expected = workstreams[5].workstream_id;
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams,
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.selected = 5;
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    assert!(view.rendered_offset > 0);
    let clicked = view
        .rendered_mouse_rows
        .iter()
        .find_map(|(row_y, snapshot_index)| (*snapshot_index == 5).then_some(*row_y))
        .and_then(|row_y| view.row_from_y(row_y))
        .unwrap();
    view.begin_mouse_click(Some(clicked));
    assert_eq!(view.selected().map(|row| row.workstream_id), Some(expected));
}

#[test]
fn remote_projection_uses_the_host_project_display_name() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let registry = HostRegistry::open(&root).unwrap();
    let host = registry.identity().unwrap();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let remote = crate::protocol::SnapshotWorkstream {
        workstream_id: WorkstreamId::new(),
        location_id: crate::domain::LocationId::new(),
        provider: ProviderKind::Codex,
        project_display_name: "dms-power-status".to_owned(),
        repository_fingerprint: None,
        remote_identity_display: None,
        display_name: "thread".to_owned(),
        runtime_id: None,
        runtime_status: RuntimeStatus::Idle,
        lifecycle: WorkstreamLifecycle::Open,
        archived: false,
        result_ready: false,
        recovery_required: false,
        attention_revision: None,
        activity_sequence: 0,
        last_activity_at_millis: Some(1_000),
        revision: Revision::INITIAL.value(),
    };
    catalog
        .register_local_project_location(
            &host,
            remote.location_id,
            Path::new("/bin/wsnav"),
            "dms-power-status",
        )
        .unwrap();

    let projected = project_remote_workstream(&mut catalog, host.host_id, "snap", &remote, true)
        .unwrap()
        .unwrap();

    assert_eq!(projected.project_label, "dms-power-status");
    assert_eq!(projected.last_activity_at_millis, Some(1_000));
}

#[test]
fn activity_age_is_compact_and_safe_for_clock_skew() {
    assert_eq!(
        relative_activity_age(Some(60_000), Some(60_999)),
        Some("now".to_owned())
    );
    assert_eq!(
        relative_activity_age(Some(60_000), Some(180_000)),
        Some("2m ago".to_owned())
    );
    assert_eq!(
        relative_activity_age(Some(60_000), Some(60_000 + 86_400_000 * 3)),
        Some("3d ago".to_owned())
    );
    assert_eq!(
        relative_activity_age(Some(60_000), Some(59_000)),
        Some("now".to_owned())
    );
    assert_eq!(relative_activity_age(None, Some(60_000)), None);
    assert_eq!(activity_label(None, Some(60_000)), "activity unknown");
}

#[test]
fn activity_age_color_is_brightest_for_recent_work_and_dims_with_age() {
    assert_eq!(activity_age_color(None, Some(60_000)), AGE_UNKNOWN_COLOR);
    assert_eq!(activity_age_color(Some(0), Some(59_000)), AGE_RECENT_COLOR);
    assert_eq!(
        activity_age_color(Some(0), Some(3_599_000)),
        AGE_HOURLY_COLOR
    );
    assert_eq!(
        activity_age_color(Some(0), Some(3_600_000)),
        AGE_DAILY_COLOR
    );
    assert_eq!(
        activity_age_color(Some(0), Some(86_400_000)),
        AGE_WEEKLY_COLOR
    );
    assert_eq!(
        activity_age_color(Some(0), Some(604_800_000)),
        AGE_STALE_COLOR
    );
    assert!(
        [
            AGE_RECENT_COLOR,
            AGE_HOURLY_COLOR,
            AGE_DAILY_COLOR,
            AGE_WEEKLY_COLOR,
            AGE_STALE_COLOR,
        ]
        .windows(2)
        .all(|pair| match pair {
            [Color::Indexed(newer), Color::Indexed(older)] => newer > older,
            _ => false,
        })
    );
}

#[test]
fn parked_indicator_has_its_own_readable_state_color() {
    let row = row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked);
    let (indicator, style) = status_indicator(&row);

    assert_eq!(indicator, "p");
    assert_eq!(style.fg, Some(PARKED_INDICATOR_COLOR));
    assert_ne!(PARKED_INDICATOR_COLOR, Color::DarkGray);
}

#[test]
fn renderer_shows_done_indicator_without_provider_content() {
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            project_label: "project".to_owned(),
            display_name: "native thread".to_owned(),
            result_ready: true,
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    let expected_project_color = *visible_project_colors(&view.snapshot)
        .get(&view.snapshot.workstreams[0].project_id)
        .unwrap();
    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();

    assert!(rendered.contains("project"));
    assert!(rendered.contains("native thread"));
    assert!(rendered.contains('✓'));
    assert!(!rendered.contains("done"));
    assert!(!rendered.contains("prompt"));
    assert!(!rendered.contains('•'));
    let project_name_cell = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .find(|cell| cell.symbol() == "p")
        .unwrap();
    assert_eq!(project_name_cell.fg, expected_project_color);
    let host_cell = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .find(|cell| cell.symbol() == "l")
        .unwrap();
    assert_eq!(host_cell.fg, Color::LightBlue);
}

#[test]
fn recent_context_justifies_provider_against_host() {
    let snapshot = LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            project_label: "project".to_owned(),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    };
    let row = &snapshot.workstreams[0];
    let project_colors = visible_project_colors(&snapshot);

    let line = workstream_context_line(
        row,
        WorkstreamRowContext::Recent,
        "   ",
        &project_colors,
        30,
    );

    let rendered = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(line.width(), 30);
    assert!(rendered.starts_with("   Codex"));
    assert!(rendered.ends_with("local"));
    assert!(!rendered.contains("project"));
    assert_eq!(
        line.spans
            .iter()
            .find(|span| span.content == "Codex")
            .unwrap()
            .style
            .fg,
        Some(provider_color(ProviderKind::Codex))
    );
    assert!(
        line.spans
            .iter()
            .any(|span| span.style.fg == Some(Color::LightBlue))
    );
    assert!(!rendered.contains(" · "));
    assert!(!rendered.contains('•'));
}

#[test]
fn recent_context_preserves_provider_when_host_is_long() {
    let snapshot = LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            host: NavigatorHost::Remote {
                alias: "remote-terminal".to_owned(),
                reachability: RemoteHostReachability::Reachable,
            },
            project_label: "project with a long name".to_owned(),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    };
    let row = &snapshot.workstreams[0];
    let project_colors = visible_project_colors(&snapshot);

    let line = workstream_context_line(
        row,
        WorkstreamRowContext::Recent,
        "   ",
        &project_colors,
        30,
    );
    let rendered = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(line.width(), 30);
    assert!(rendered.starts_with("   Codex"), "{rendered:?}");
    assert!(rendered.contains('…'));
    assert!(rendered.ends_with("remote-term…"), "{rendered:?}");
    assert!(!rendered.contains("project"));
}

#[test]
fn recent_card_renders_project_environment_and_thread_on_separate_rows() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            provider: ProviderKind::OpenCode,
            project_label: "project-name".to_owned(),
            display_name: "thread-name".to_owned(),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        ..LocalNavigatorSnapshot::default()
    });
    let mut terminal = Terminal::new(TestBackend::new(32, 8)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rows = terminal
        .backend()
        .buffer()
        .content()
        .chunks(32)
        .map(|cells| {
            cells
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert!(rows[1].contains("project-name"));
    assert!(!rows[1].contains("OpenCode"));
    assert!(rows[2].contains("OpenCode"));
    assert!(rows[2].contains("local"));
    assert!(!rows[2].contains("project-name"));
    assert!(!rows[2].contains("thread-name"));
    assert!(rows[3].contains("thread-n…"));
    assert!(rows[3].contains("activity unknown"));
}

#[test]
fn grouped_contexts_justify_the_requested_identity_axes_without_separators() {
    let snapshot = LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            provider: ProviderKind::OpenCode,
            project_label: "project".to_owned(),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        ..LocalNavigatorSnapshot::default()
    };
    let row = &snapshot.workstreams[0];
    let project_colors = visible_project_colors(&snapshot);

    let by_project = workstream_context_line(
        row,
        WorkstreamRowContext::Project,
        " └─ ",
        &project_colors,
        30,
    );
    let by_project_rendered = by_project
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(by_project.width(), 30);
    assert!(by_project_rendered.starts_with(" └─ OpenCode"));
    assert!(by_project_rendered.ends_with("local"));
    assert!(!by_project_rendered.contains(" · "));
    assert!(!by_project_rendered.contains('•'));

    let by_host =
        workstream_context_line(row, WorkstreamRowContext::Host, " └─ ", &project_colors, 30);
    let by_host_rendered = by_host
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(by_host.width(), 30);
    assert!(by_host_rendered.starts_with(" └─ project"));
    assert!(by_host_rendered.ends_with("OpenCode"));
    assert!(!by_host_rendered.contains(" · "));
    assert!(!by_host_rendered.contains('•'));
}

#[test]
fn every_workstream_context_keeps_open_code_visible_at_supported_narrow_width() {
    let snapshot = LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            provider: ProviderKind::OpenCode,
            host: NavigatorHost::Remote {
                alias: "remote-host-with-a-long-label".to_owned(),
                reachability: RemoteHostReachability::Reachable,
            },
            project_label: "project-with-a-long-label".to_owned(),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        ..LocalNavigatorSnapshot::default()
    };
    let row = &snapshot.workstreams[0];
    let project_colors = visible_project_colors(&snapshot);

    for (context, prefix) in [
        (WorkstreamRowContext::Recent, "   "),
        (WorkstreamRowContext::Archived, "   "),
        (WorkstreamRowContext::Host, " └─ "),
        (WorkstreamRowContext::Project, " └─ "),
    ] {
        let line = workstream_context_line(row, context, prefix, &project_colors, 30);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(line.width() <= 30);
        assert!(rendered.contains("OpenCode"), "{context:?}: {rendered:?}");
        assert!(rendered.contains('…'), "{context:?}: {rendered:?}");
    }
}

#[test]
fn host_and_project_accents_use_separate_color_families() {
    assert_eq!(host_color("local"), HOST_LABEL_PALETTE[0]);
    for host_color in HOST_LABEL_PALETTE {
        assert!(!PROJECT_MARKER_PALETTE.contains(&host_color));
        assert!(!PROVIDER_LABEL_PALETTE.contains(&host_color));
    }
    for provider_color in PROVIDER_LABEL_PALETTE {
        assert!(!PROJECT_MARKER_PALETTE.contains(&provider_color));
    }
    assert_ne!(provider_color(ProviderKind::Codex), Color::White);
    assert_ne!(provider_color(ProviderKind::OpenCode), Color::White);
    assert_ne!(
        provider_color(ProviderKind::Codex),
        provider_color(ProviderKind::OpenCode)
    );
}

#[test]
fn selected_row_background_never_masks_semantic_row_foregrounds() {
    let semantic_colors = [
        Color::White,
        Color::Gray,
        Color::DarkGray,
        Color::Red,
        Color::Yellow,
        Color::Cyan,
        Color::Green,
        PARKED_INDICATOR_COLOR,
        PROJECT_TREE_COLOR,
        PROJECT_ORIGIN_ICON_COLOR,
        PROJECT_ORIGIN_LABEL_COLOR,
        PROJECT_ARCHIVED_COLOR,
        AGE_UNKNOWN_COLOR,
        AGE_RECENT_COLOR,
        AGE_HOURLY_COLOR,
        AGE_DAILY_COLOR,
        AGE_WEEKLY_COLOR,
        AGE_STALE_COLOR,
    ];

    assert!(!semantic_colors.contains(&SELECTED_ROW_BACKGROUND));
    assert!(!HOST_LABEL_PALETTE.contains(&SELECTED_ROW_BACKGROUND));
    assert!(!PROJECT_MARKER_PALETTE.contains(&SELECTED_ROW_BACKGROUND));
    assert!(!PROVIDER_LABEL_PALETTE.contains(&SELECTED_ROW_BACKGROUND));
    assert_ne!(PROJECT_TREE_COLOR, Color::DarkGray);
    assert_ne!(PROJECT_ORIGIN_ICON_COLOR, Color::DarkGray);
    assert_ne!(PROJECT_ORIGIN_LABEL_COLOR, Color::DarkGray);
    assert_ne!(PROJECT_ARCHIVED_COLOR, Color::DarkGray);
    assert_eq!(
        selected_row_style().bg,
        Some(SELECTED_ROW_BACKGROUND),
        "selection must preserve the parked marker and quiet activity age"
    );
}

#[test]
fn visible_projects_receive_distinct_stable_marker_colors() {
    let workstreams = (0..PROJECT_MARKER_PALETTE.len())
        .map(|index| NavigatorWorkstream {
            project_label: format!("project-{index}"),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        })
        .collect::<Vec<_>>();
    let project_ids = workstreams
        .iter()
        .map(|workstream| workstream.project_id)
        .collect::<Vec<_>>();
    let snapshot = LocalNavigatorSnapshot {
        workstreams,
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    };

    let colors = visible_project_colors(&snapshot);
    let assigned = project_ids
        .iter()
        .map(|project_id| *colors.get(project_id).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(colors.len(), PROJECT_MARKER_PALETTE.len());
    for (index, color) in assigned.iter().enumerate() {
        assert!(!assigned[..index].contains(color));
    }
    assert_eq!(colors, visible_project_colors(&snapshot));
}

#[test]
fn renderer_makes_unresolved_operation_recovery_visible() {
    let mut terminal = Terminal::new(TestBackend::new(200, 8)).unwrap();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        unresolved_operation_count: 1,
        unresolved_operations: Vec::new(),
        ..LocalNavigatorSnapshot::default()
    });

    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();

    assert!(rendered.contains("unfinished Fork"));
    assert!(rendered.contains("No Workstreams yet"));
    assert!(rendered.contains("? keys"));
}

#[test]
fn repeated_fork_routes_only_matching_unfinished_forks_to_reconciliation() {
    let source_id = WorkstreamId::new();
    let unrelated_source_id = WorkstreamId::new();
    let first_operation_id = OperationId::new();
    let second_operation_id = OperationId::new();
    let unrelated_operation_id = OperationId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(source_id, NavigatorRuntimeStatus::Idle)],
        hosts: vec![NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: SnapshotResponse::default().provider_capabilities,
        }],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 3,
        unresolved_operations: vec![
            NavigatorOperation {
                host: NavigatorHost::Local,
                operation_id: first_operation_id,
                kind: OperationKind::Fork,
                source_workstream_id: Some(source_id),
                phase: OperationPhase::AwaitingReconciliation,
                revision: Revision::INITIAL,
            },
            NavigatorOperation {
                host: NavigatorHost::Local,
                operation_id: second_operation_id,
                kind: OperationKind::Fork,
                source_workstream_id: Some(source_id),
                phase: OperationPhase::RecoveryRequired,
                revision: Revision::INITIAL.next(),
            },
            NavigatorOperation {
                host: NavigatorHost::Remote {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                operation_id: unrelated_operation_id,
                kind: OperationKind::Fork,
                source_workstream_id: Some(unrelated_source_id),
                phase: OperationPhase::RecoveryRequired,
                revision: Revision::INITIAL.next(),
            },
        ],
    });
    let source = view.selected().unwrap().clone();
    assert!(view.begin_fork_recovery(&source));
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Unfinished forks"));
    assert!(rendered.contains("local · Fork · awaiting reconciliation"));
    assert!(rendered.contains("local · Fork · recovery required"));
    assert!(!rendered.contains("snap · Fork"));
    assert!(!rendered.contains(&first_operation_id.to_string()));
    assert!(!rendered.contains(&second_operation_id.to_string()));
    assert!(!rendered.contains(&unrelated_operation_id.to_string()));
    assert_eq!(view.selected_host_alias(), Some("local"));

    view.select_next();
    assert_eq!(view.selected_operation, 1);
    assert_eq!(view.selected_host_alias(), Some("local"));

    view.replace_snapshot(LocalNavigatorSnapshot::default());
    assert_eq!(view.detail, None);
}

#[test]
fn repeated_fork_prompts_before_reconciling_one_exact_operation() {
    let source_id = WorkstreamId::new();
    let operation_id = OperationId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(source_id, NavigatorRuntimeStatus::Idle)],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 1,
        unresolved_operations: vec![NavigatorOperation {
            host: NavigatorHost::Local,
            operation_id,
            kind: OperationKind::Fork,
            source_workstream_id: Some(source_id),
            phase: OperationPhase::RecoveryRequired,
            revision: Revision::INITIAL,
        }],
    });
    let source = view.selected().unwrap().clone();

    assert!(view.begin_fork_recovery(&source));
    assert!(matches!(
        view.modal,
        Some(NavigatorModal::ConfirmForkRecovery { ref operation, .. })
            if operation.operation_id == operation_id
    ));
    let (title, lines) = navigator_modal_content(view.confirm_modal().unwrap(), 50);
    let rendered = lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert_eq!(title, " Finish earlier Fork ");
    assert!(rendered.contains("did not finish confirming its destination"));
    assert!(rendered.contains("reconcile it"));
    assert!(rendered.contains("start another Fork"));
    assert!(!rendered.contains(&operation_id.to_string()));
}

#[test]
fn empty_navigator_requires_an_explicit_project_registration() {
    let view = NavigatorView::new(LocalNavigatorSnapshot::default());

    assert_eq!(
        view.footer_status(),
        "No Workstreams yet · n adds a Project"
    );
}

#[test]
fn help_toggle_is_navigator_local_state() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());

    assert!(!view.help_visible());
    view.toggle_help();
    assert!(view.help_visible());
    assert_eq!(view.help_scroll, 0);

    view.dismiss_help();
    assert!(!view.help_visible());
}

#[test]
fn view_mode_cycles_without_changing_the_selected_workstream() {
    let workstream_id = WorkstreamId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(workstream_id, NavigatorRuntimeStatus::Idle)],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    assert_eq!(view.view_mode(), NavigatorViewMode::Recent);
    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Project);
    assert_eq!(view.footer_status(), "");
    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Host);
    view.cycle_view_mode_previous();
    assert_eq!(view.view_mode(), NavigatorViewMode::Project);
    view.cycle_view_mode_previous();

    assert_eq!(view.view_mode(), NavigatorViewMode::Recent);
    view.cycle_view_mode_previous();
    assert_eq!(view.view_mode(), NavigatorViewMode::Archived);
    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Recent);
    assert_eq!(
        view.selected().map(|row| row.workstream_id),
        Some(workstream_id)
    );
}

#[test]
fn grouped_views_add_non_actionable_headers_and_keep_group_order_recent_first() {
    let shared_project = ProjectId::new();
    let other_project = ProjectId::new();
    let snap_workstream = WorkstreamId::new();
    let local_shared = WorkstreamId::new();
    let local_other = WorkstreamId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            NavigatorWorkstream {
                host: NavigatorHost::Remote {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                project_id: shared_project,
                project_label: "shared".to_owned(),
                ..row(snap_workstream, NavigatorRuntimeStatus::Working)
            },
            NavigatorWorkstream {
                project_id: shared_project,
                project_label: "shared".to_owned(),
                ..row(local_shared, NavigatorRuntimeStatus::Idle)
            },
            NavigatorWorkstream {
                project_id: other_project,
                project_label: "other".to_owned(),
                ..row(local_other, NavigatorRuntimeStatus::Parked)
            },
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    view.cycle_view_mode_next();
    assert_eq!(
        view.list_entries(),
        vec![
            NavigatorListEntry::ProjectHeader {
                project_id: shared_project,
                label: "shared".to_owned(),
            },
            NavigatorListEntry::Workstream {
                snapshot_index: 0,
                context: WorkstreamRowContext::Project,
                tree_branch: Some(TreeBranch { is_last: false }),
            },
            NavigatorListEntry::Workstream {
                snapshot_index: 1,
                context: WorkstreamRowContext::Project,
                tree_branch: Some(TreeBranch { is_last: true }),
            },
            NavigatorListEntry::ProjectHeader {
                project_id: other_project,
                label: "other".to_owned(),
            },
            NavigatorListEntry::Workstream {
                snapshot_index: 2,
                context: WorkstreamRowContext::Project,
                tree_branch: Some(TreeBranch { is_last: true }),
            },
        ]
    );
    view.cycle_view_mode_next();
    assert_eq!(
        view.list_entries(),
        vec![
            NavigatorListEntry::HostHeader {
                alias: "snap".to_owned(),
            },
            NavigatorListEntry::Workstream {
                snapshot_index: 0,
                context: WorkstreamRowContext::Host,
                tree_branch: Some(TreeBranch { is_last: true }),
            },
            NavigatorListEntry::HostHeader {
                alias: "local".to_owned(),
            },
            NavigatorListEntry::Workstream {
                snapshot_index: 1,
                context: WorkstreamRowContext::Host,
                tree_branch: Some(TreeBranch { is_last: false }),
            },
            NavigatorListEntry::Workstream {
                snapshot_index: 2,
                context: WorkstreamRowContext::Host,
                tree_branch: Some(TreeBranch { is_last: true }),
            },
        ]
    );
}

#[test]
fn grouped_navigation_follows_rendered_row_order() {
    let shared_project = ProjectId::new();
    let other_project = ProjectId::new();
    let newest_snap = WorkstreamId::new();
    let middle_local = WorkstreamId::new();
    let oldest_snap = WorkstreamId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        // Snapshot order is activity order: snap, local, snap. Both
        // grouped views render snap's/shared project's two rows together.
        workstreams: vec![
            NavigatorWorkstream {
                host: NavigatorHost::Remote {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                project_id: shared_project,
                project_label: "shared".to_owned(),
                ..row(newest_snap, NavigatorRuntimeStatus::Working)
            },
            NavigatorWorkstream {
                project_id: other_project,
                project_label: "other".to_owned(),
                ..row(middle_local, NavigatorRuntimeStatus::Idle)
            },
            NavigatorWorkstream {
                host: NavigatorHost::Remote {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                project_id: shared_project,
                project_label: "shared".to_owned(),
                ..row(oldest_snap, NavigatorRuntimeStatus::Parked)
            },
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    for mode in [NavigatorViewMode::Host, NavigatorViewMode::Project] {
        view.view_mode = mode;
        view.select_row(0);
        view.select_next();
        assert_eq!(
            view.selected().map(|row| row.workstream_id),
            Some(oldest_snap)
        );
        view.select_next();
        assert_eq!(
            view.selected().map(|row| row.workstream_id),
            Some(middle_local)
        );
        view.select_next();
        assert_eq!(
            view.selected().map(|row| row.workstream_id),
            Some(newest_snap)
        );

        view.select_previous();
        assert_eq!(
            view.selected().map(|row| row.workstream_id),
            Some(middle_local)
        );
    }
}

#[test]
fn renderer_draws_a_navigator_only_shortcut_overlay() {
    let mut terminal = Terminal::new(TestBackend::new(80, 22)).unwrap();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![NavigatorWorkstream {
            project_label: "project".to_owned(),
            display_name: "native thread".to_owned(),
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
        }],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.toggle_help();

    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();

    assert!(rendered.contains("Keys · Workstreams"));
    assert!(rendered.contains("Navigation"));
    assert!(rendered.contains("Workstreams"));
    let full_help = help_lines(
        NavigatorPage::Workstreams,
        false,
        false,
        NavigatorViewMode::Recent,
    )
    .into_iter()
    .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
    .collect::<String>();
    assert!(full_help.contains("←/→"));
    assert!(full_help.contains("cycle views"));
    assert!(!full_help.contains("recover an unresolved"));
    assert!(!full_help.contains("Mouse"));
    assert!(!full_help.contains("click a row to select"));
    assert!(!full_help.contains("close keys"));
    assert!(!rendered.contains("provider pane"));
}

#[test]
fn management_pages_preserve_workstream_attachment_and_expose_bounded_summaries() {
    let attached_id = WorkstreamId::new();
    let project_id = ProjectId::new();
    let remote_id = WorkstreamId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            NavigatorWorkstream {
                project_id,
                project_label: "alpha".to_owned(),
                last_activity_at_millis: Some(2_000),
                ..row(attached_id, NavigatorRuntimeStatus::Idle)
            },
            NavigatorWorkstream {
                host: NavigatorHost::Remote {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                project_id,
                project_label: "alpha".to_owned(),
                last_activity_at_millis: Some(3_000),
                ..row(remote_id, NavigatorRuntimeStatus::Parked)
            },
        ],
        hosts: vec![
            NavigatorHostOverview {
                alias: "local".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
            NavigatorHostOverview {
                alias: "snap".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
            NavigatorHostOverview {
                alias: "spare".to_owned(),
                reachability: RemoteHostReachability::Unreachable(
                    RemoteHostIssue::SshOrRemoteExecutableUnavailable,
                ),
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
        ],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.observe_attachment(&AttachmentStatus {
        attempt_id: uuid::Uuid::new_v4(),
        host_alias: "local".to_owned(),
        workstream_id: attached_id,
        phase: AttachmentPhase::Running,
    });

    view.select_page(NavigatorPage::Projects);
    assert_eq!(view.page(), NavigatorPage::Projects);
    assert_eq!(view.projects().len(), 1);
    assert_eq!(view.projects()[0].label, "alpha");
    assert_eq!(view.projects()[0].workstream_count, 2);
    assert!(view.is_attached_to(&view.snapshot.workstreams[0]));
    assert_eq!(
        view.selected().map(|row| row.workstream_id),
        Some(attached_id)
    );

    assert_eq!(view.selected_project, Some(project_id));
    view.open_selected_detail();
    assert!(view.detail.is_none());

    view.select_page(NavigatorPage::Hosts);
    assert_eq!(
        view.hosts()
            .iter()
            .map(|host| (host.alias.as_str(), host.workstream_count))
            .collect::<Vec<_>>(),
        vec![("local", 1), ("snap", 1), ("spare", 0)]
    );
    view.select_next();
    assert_eq!(view.selected_host_alias(), Some("snap"));
    view.select_next();
    assert_eq!(view.selected_host_alias(), Some("spare"));
    view.open_selected_detail();
    assert!(view.detail.is_none());
    assert!(view.is_attached_to(&view.snapshot.workstreams[0]));
}

#[test]
fn management_parent_retains_the_current_workstreams_header() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());
    view.select_page(NavigatorPage::Projects);
    let mut terminal = Terminal::new(TestBackend::new(32, 12)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let buffer = terminal.backend().buffer();
    let rendered = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert_eq!(buffer[(0, 0)].symbol(), "┌");
    assert_eq!(buffer[(31, 0)].symbol(), "┐");
    assert!(rendered.contains("Workstreams · Recent"));
    assert!(rendered.contains("Projects"));
}

#[test]
fn host_summary_groups_active_projects_and_omits_archived_workstreams() {
    let alpha = ProjectId::new();
    let beta = ProjectId::new();
    let mut archived = row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked);
    archived.project_id = beta;
    archived.project_label = "beta".to_owned();
    archived.archived = true;
    let mut second_alpha = row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle);
    second_alpha.project_id = alpha;
    second_alpha.project_label = "alpha".to_owned();
    let mut first_alpha = row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle);
    first_alpha.project_id = alpha;
    first_alpha.project_label = "alpha".to_owned();
    let view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![archived, second_alpha, first_alpha],
        hosts: vec![NavigatorHostOverview {
            alias: "local".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: SnapshotResponse::default().provider_capabilities,
        }],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    let hosts = view.hosts();
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        hosts[0]
            .active_projects
            .iter()
            .map(|project| (project.label.as_str(), project.active_workstream_count))
            .collect::<Vec<_>>(),
        vec![("alpha", 2)]
    );
}

#[test]
fn host_page_shows_an_active_project_tree_without_private_identifiers() {
    let workstream_id = WorkstreamId::new();
    let location_id = LocationId::new();
    let mut remote = row(workstream_id, NavigatorRuntimeStatus::Idle);
    remote.host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    remote.location_id = location_id;
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![remote],
        hosts: vec![
            NavigatorHostOverview {
                alias: "local".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::Ready,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
            NavigatorHostOverview {
                alias: "snap".to_owned(),
                reachability: RemoteHostReachability::Unreachable(
                    RemoteHostIssue::ProtocolMismatch {
                        local: 14,
                        remote: 13,
                    },
                ),
                observer_status: ObserverStatus::TrustPending,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
        ],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.select_page(NavigatorPage::Hosts);
    view.select_next();
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();

    assert!(rendered.contains("project"));
    assert!(rendered.contains("1 active"));
    assert!(rendered.contains("└─"));
    assert!(!rendered.contains("observer ready"));
    assert!(!rendered.contains(&workstream_id.to_string()));
    assert!(!rendered.contains(&location_id.to_string()));
    assert_eq!(
        observer_status_indicator(ObserverStatus::Ready),
        ("✓", Color::Green)
    );
    let snap = view
        .hosts()
        .into_iter()
        .find(|host| host.alias == "snap")
        .unwrap();
    assert_eq!(
        String::from(host_connection_line(&snap, 100)),
        "✗ protocol 13 ≠ 14"
    );
}

#[test]
fn remote_build_mismatch_has_a_bounded_host_page_diagnosis() {
    let issue = remote_build_issue(&BuildInfoError::ProtocolVersionMismatch {
        local: 14,
        remote: 13,
    });

    assert_eq!(
        issue,
        RemoteHostIssue::ProtocolMismatch {
            local: 14,
            remote: 13,
        }
    );
    assert_eq!(issue.label(), "protocol 13 ≠ 14");
    assert_eq!(issue.color(), Color::Red);
}

#[test]
fn host_removal_lets_the_user_choose_disconnect_or_offboard() {
    let mut remote = row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked);
    remote.host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![remote],
        hosts: vec![
            NavigatorHostOverview {
                alias: "local".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::Ready,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
            NavigatorHostOverview {
                alias: "snap".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::Ready,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
        ],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.select_page(NavigatorPage::Hosts);
    view.select_next();

    forget_selected_host(&mut view);

    assert!(matches!(
        view.modal,
        Some(NavigatorModal::SelectHostRemoval {
            ref alias,
            offboard: false,
            ..
        }) if alias == "snap"
    ));
    view.toggle_host_removal_mode();
    assert!(matches!(
        view.modal,
        Some(NavigatorModal::SelectHostRemoval { offboard: true, .. })
    ));

    let (_, lines) = navigator_modal_content(view.modal.clone().unwrap(), 30);
    let rendered = lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert!(rendered.contains("disconnect: forget WSNav registration; keep observer"));
    assert!(rendered.contains("offboard: remove observer, then forget registration"));

    view.dismiss_modal();
    view.select_previous();
    forget_selected_host(&mut view);
    assert_eq!(view.modal, None);
    assert_eq!(
        view.message.as_deref(),
        Some("the local Host is protected and cannot be forgotten")
    );
}

#[test]
fn offboarding_refuses_to_remove_an_observer_while_a_runtime_is_live() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path()).unwrap();
    let mut remote_workstream = row(WorkstreamId::new(), NavigatorRuntimeStatus::Working);
    remote_workstream.host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![remote_workstream],
        hosts: vec![NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: SnapshotResponse::default().provider_capabilities,
        }],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    let mut monitor = RemoteMonitor::new();

    offboard_host(&root, &mut monitor, &mut view, "snap", 1, 1, 0);

    assert_eq!(
        view.message.as_deref(),
        Some("park live Workstreams before offboarding this host")
    );
}

#[test]
fn project_tree_exposes_host_owned_locations_without_start_actions() {
    let project_id = ProjectId::new();
    let local_location = LocationId::new();
    let remote_location = LocationId::new();
    let local_active = WorkstreamId::new();
    let local_archived = WorkstreamId::new();
    let remote_archived = WorkstreamId::new();
    let mut local_archived_row = NavigatorWorkstream {
        project_id,
        location_id: local_location,
        location_label: "main checkout".to_owned(),
        remote_identity_display: Some("github.com/owner/repo".to_owned()),
        last_activity_at_millis: Some(2_000),
        ..row(local_archived, NavigatorRuntimeStatus::Parked)
    };
    local_archived_row.archived = true;
    let mut remote_archived_row = NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
        },
        project_id,
        location_id: remote_location,
        location_label: "remote checkout".to_owned(),
        last_activity_at_millis: Some(1_000),
        ..row(remote_archived, NavigatorRuntimeStatus::Parked)
    };
    remote_archived_row.archived = true;
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            NavigatorWorkstream {
                project_id,
                location_id: local_location,
                location_label: "main checkout".to_owned(),
                last_activity_at_millis: Some(3_000),
                ..row(local_active, NavigatorRuntimeStatus::Idle)
            },
            local_archived_row,
            remote_archived_row,
        ],
        hosts: vec![NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: SnapshotResponse::default().provider_capabilities,
        }],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.select_page(NavigatorPage::Projects);

    let project = view.projects().pop().unwrap();
    assert_eq!(project.active_workstream_count, 1);
    assert_eq!(project.archived_workstream_count, 2);
    assert_eq!(project.locations.len(), 2);

    let mut terminal = Terminal::new(TestBackend::new(80, 14)).unwrap();
    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("1 active · 2 archived"));
    assert!(rendered.contains("↗ owner/repo"));
    assert!(!rendered.contains("github.com/owner/repo"));
    assert!(!rendered.contains("origin ·"));
    assert!(!rendered.contains("0 active"));
    assert!(rendered.contains("local · main checkout"));
    assert!(rendered.contains("snap · remote checkout"));
    assert!(rendered.contains("├─"));
    assert!(rendered.contains("└─"));
    assert!(!rendered.contains("n start"));
    assert!(!rendered.contains(&local_location.to_string()));
    assert!(!rendered.contains(&remote_location.to_string()));
}

#[test]
fn management_page_mouse_targets_cover_inline_rows_without_page_tabs() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)],
        hosts: vec![
            NavigatorHostOverview {
                alias: "local".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
            NavigatorHostOverview {
                alias: "snap".to_owned(),
                reachability: RemoteHostReachability::Reachable,
                observer_status: ObserverStatus::NotInstalled,
                provider_capabilities: SnapshotResponse::default().provider_capabilities,
            },
        ],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();
    view.select_page(NavigatorPage::Projects);
    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Workstreams"));
    assert!(rendered.contains("Projects"));
    assert_eq!(view.project_from_y(2), view.selected_project);
    assert_eq!(view.project_from_y(4), view.selected_project);
    view.begin_project_click(view.project_from_y(4));
    assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Management));

    view.select_page(NavigatorPage::Hosts);
    terminal.draw(|frame| view.render(frame)).unwrap();
    assert_eq!(view.host_from_y(2).as_deref(), Some("local"));
    assert_eq!(view.host_from_y(3).as_deref(), Some("local"));
    assert_eq!(view.host_from_y(4).as_deref(), Some("local"));
    view.begin_host_click(view.host_from_y(3));
    assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Management));
}

#[test]
fn compact_workstream_controls_preserve_terminal_key_memory() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());
    let compact = compact_keys(&view);
    assert!(compact.starts_with(' '));
    assert!(compact.contains("←/→ view"));
    assert!(compact.contains("n register"));
    assert!(compact.contains("? keys"));
    assert!(!compact.contains("s scope"));
    assert!(!compact.contains("Enter"));
    assert!(!compact.contains("Esc"));
    assert_bindings_in_order(
        &compact,
        &[
            "←/→ view",
            "n register",
            "f fork",
            "p park",
            "r rename",
            "a ack",
            "x archive",
        ],
    );
    assert!(!compact.contains("No Workstreams"));
    assert_eq!(
        view.footer_status(),
        "No Workstreams yet · n adds a Project"
    );
    view.cycle_view_mode_next();
    view.cycle_view_mode_next();
    view.cycle_view_mode_next();
    assert_eq!(view.view_mode(), NavigatorViewMode::Archived);
    assert!(compact_keys(&view).contains("u restore"));
    assert!(!compact_keys(&view).contains("n register"));
}

#[test]
fn compact_management_controls_keep_related_actions_adjacent() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());
    view.select_page(NavigatorPage::Projects);
    let project_compact = compact_keys(&view);
    assert!(project_compact.contains("a add"));
    assert!(project_compact.contains("x remove"));
    assert!(project_compact.contains(", workstreams"));
    assert!(project_compact.contains(". hosts"));
    assert!(!project_compact.contains("Enter"));
    assert!(!project_compact.contains("Esc"));
    assert_bindings_in_order(
        &project_compact,
        &["a add", "x remove", ", workstreams", ". hosts"],
    );
    assert!(!project_compact.contains("n start"));
    assert!(!project_compact.contains("n new"));

    view.toggle_management_page(NavigatorPage::Projects);
    assert_eq!(view.page(), NavigatorPage::Workstreams);
    view.toggle_management_page(NavigatorPage::Hosts);
    assert_eq!(view.page(), NavigatorPage::Hosts);
    view.toggle_management_page(NavigatorPage::Hosts);
    assert_eq!(view.page(), NavigatorPage::Workstreams);

    view.select_page(NavigatorPage::Hosts);
    let host_compact = compact_keys(&view);
    assert!(host_compact.contains("a add"));
    assert!(host_compact.contains("x remove"));
    assert!(host_compact.contains("r root"));
    assert!(!host_compact.contains("v verify"));
    assert!(!host_compact.contains("Enter"));
    assert!(!host_compact.contains("Esc"));
    assert_bindings_in_order(
        &host_compact,
        &["a add", "x remove", "r root", ", projects", ". workstreams"],
    );

    let host_help = help_lines(
        NavigatorPage::Hosts,
        false,
        false,
        NavigatorViewMode::Recent,
    )
    .into_iter()
    .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
    .collect::<String>();
    assert!(host_help.contains("add/setup SSH host"));
    assert!(host_help.contains("disconnect/offboard"));
}

#[test]
fn expanded_controls_preserve_terminal_key_memory() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());
    let workstream_help = help_lines(
        NavigatorPage::Workstreams,
        false,
        false,
        NavigatorViewMode::Recent,
    );
    let workstream_help = workstream_help
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert!(workstream_help.contains("x          archive"));
    assert!(!workstream_help.contains("switch active/archived scope"));
    assert!(workstream_help.contains(",          Projects page"));
    assert!(workstream_help.contains(".          Hosts page"));
    assert!(!workstream_help.contains("1          Workstreams page"));

    view.select_page(NavigatorPage::Workstreams);
    view.help_visible = true;
    let mut help_terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
    help_terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered_help = help_terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered_help.contains("archive (may park)"));

    let archived_help = help_lines(
        NavigatorPage::Workstreams,
        false,
        false,
        NavigatorViewMode::Archived,
    )
    .into_iter()
    .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
    .collect::<String>();
    assert!(archived_help.contains("restore (no start)"));
    assert!(!archived_help.contains("new Workstream"));
    assert!(!archived_help.contains("focus native agent"));

    let expanded = help_lines(
        NavigatorPage::Workstreams,
        false,
        false,
        NavigatorViewMode::Recent,
    );
    assert!(expanded.iter().all(|line| {
        line.spans
            .iter()
            .filter(|span| span.style.fg == Some(Color::Yellow))
            .count()
            <= 1
    }));
    let rendered_expanded = expanded
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
        .collect::<String>();
    assert!(rendered_expanded.contains("↑/↓"));
    assert!(rendered_expanded.contains("←/→"));
    assert!(!rendered_expanded.contains("j/k"));
    for line in &expanded {
        let Some(shortcut) = line
            .spans
            .first()
            .filter(|span| span.style.fg == Some(Color::Yellow))
        else {
            continue;
        };
        let padding = line.spans[1]
            .content
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        assert_eq!(
            Line::raw(shortcut.content.as_ref()).width() + padding,
            HELP_DESCRIPTION_COLUMN
        );
    }
}

#[test]
fn expanded_help_descriptions_fit_every_default_width_surface() {
    let surfaces = [
        help_lines(
            NavigatorPage::Workstreams,
            false,
            false,
            NavigatorViewMode::Recent,
        ),
        help_lines(
            NavigatorPage::Workstreams,
            false,
            false,
            NavigatorViewMode::Archived,
        ),
        help_lines(
            NavigatorPage::Projects,
            false,
            false,
            NavigatorViewMode::Recent,
        ),
        help_lines(
            NavigatorPage::Hosts,
            false,
            false,
            NavigatorViewMode::Recent,
        ),
        help_lines(
            NavigatorPage::Workstreams,
            true,
            false,
            NavigatorViewMode::Recent,
        ),
        help_lines(
            NavigatorPage::Workstreams,
            true,
            true,
            NavigatorViewMode::Recent,
        ),
    ];

    for lines in surfaces {
        for line in lines {
            let Some(shortcut) = line
                .spans
                .first()
                .filter(|span| span.style.fg == Some(Color::Yellow))
            else {
                continue;
            };
            assert!(
                line.width() <= HELP_CONTENT_WIDTH,
                "help row exceeds {HELP_CONTENT_WIDTH} cells: {line:?}"
            );
            assert!(
                Line::raw(line.spans[1].content.trim_start()).width() <= HELP_DESCRIPTION_WIDTH,
                "help description exceeds {HELP_DESCRIPTION_WIDTH} cells: {line:?}"
            );
            assert!(Line::raw(shortcut.content.as_ref()).width() < HELP_DESCRIPTION_COLUMN);
        }
    }
}

#[test]
fn status_box_wraps_bounded_navigator_feedback_above_key_hints() {
    let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());
    view.set_message(
        "an intentionally long navigator action result wraps without replacing persistent keys",
    );
    let mut terminal = Terminal::new(TestBackend::new(32, 16)).unwrap();

    terminal.draw(|frame| view.render(frame)).unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("Status"));
    assert!(rendered.contains("an intentionally long"));
    assert!(rendered.contains("navigator action result"));
    assert!(rendered.contains("←/→"));
}

#[test]
fn project_summary_uses_distinct_readable_active_and_archived_colors() {
    let project = NavigatorProjectOverview {
        project_id: ProjectId::new(),
        label: "project".to_owned(),
        remote_identity_display: None,
        workstream_count: 1,
        active_workstream_count: 1,
        archived_workstream_count: 0,
        locations: vec![NavigatorProjectLocation {
            host: NavigatorHost::Local,
            location_id: LocationId::new(),
            label: "main checkout".to_owned(),
            active_workstream_count: 1,
            archived_workstream_count: 0,
            latest_activity_at_millis: Some(1_000),
        }],
        latest_activity_at_millis: Some(1_000),
    };

    let line = project_activity_line(&project);

    assert_eq!(line.spans[0].content, "1 active");
    assert_eq!(line.spans[0].style.fg, Some(PROJECT_ACTIVE_COLOR));
    assert_eq!(line.spans[1].content, " · ");
    assert_eq!(line.spans[1].style.fg, Some(PROJECT_TREE_COLOR));
    assert_eq!(line.spans[2].content, "0 archived");
    assert_eq!(line.spans[2].style.fg, Some(PROJECT_ARCHIVED_COLOR));
}

#[test]
fn project_origin_elides_the_normalized_remote_host_for_narrow_rendering() {
    assert_eq!(
        compact_remote_display("github.com/owner/repository"),
        "owner/repository"
    );
    assert_eq!(
        compact_remote_display("git.example.test:8443/group/repository"),
        "group/repository"
    );
    assert_eq!(compact_remote_display("unknown"), "unknown");
}

#[test]
fn grouped_rows_render_explicit_two_line_tree_and_bound_title_width() {
    let first = WorkstreamId::new();
    let second = WorkstreamId::new();
    let project_id = ProjectId::new();
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            NavigatorWorkstream {
                project_id,
                display_name: "a deliberately long native thread title".to_owned(),
                ..row(first, NavigatorRuntimeStatus::Idle)
            },
            NavigatorWorkstream {
                project_id,
                ..row(second, NavigatorRuntimeStatus::Parked)
            },
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.view_mode = NavigatorViewMode::Project;
    let mut terminal = Terminal::new(TestBackend::new(80, 14)).unwrap();
    terminal.draw(|frame| view.render(frame)).unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("├─"));
    assert!(rendered.contains("└─"));
    assert!(rendered.contains("│"));
    assert!(!rendered.contains('•'));

    let spans = thread_line(
        &view.snapshot.workstreams[0],
        "✓",
        Style::default(),
        Style::default(),
        " │  ",
        40,
    );
    let line = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(line.contains('…'));
    assert!(line.ends_with("activity unknown"));
    assert!(line.chars().count() <= 40);
}

#[test]
fn working_state_wins_over_a_prior_unacknowledged_result() {
    let row = NavigatorWorkstream {
        result_ready: true,
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Working)
    };

    assert_eq!(status_indicator(&row).0, "●");
}

#[test]
fn acknowledged_attention_returns_to_an_empty_idle_indicator() {
    let row = row(WorkstreamId::new(), NavigatorRuntimeStatus::Attention);

    assert_eq!(status_indicator(&row).0, " ");
}

#[test]
fn working_indicator_is_static_and_wins_over_a_stale_result() {
    let row = NavigatorWorkstream {
        result_ready: true,
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Working)
    };

    assert_eq!(status_indicator(&row).0, "●");
}

#[test]
fn unreachable_remote_keeps_cached_status_and_attention_without_claiming_a_stop() {
    let mut monitor = RemoteMonitor::new();
    let workstream_id = WorkstreamId::new();
    let mut cached_capabilities = SnapshotResponse::default().provider_capabilities;
    cached_capabilities[0].status = crate::protocol::ProviderCapabilityStatus::Available;
    cached_capabilities[0].reason = crate::protocol::ProviderCapabilityReason::None;
    cached_capabilities[0].fresh_launch = true;
    cached_capabilities[0].exact_resume = true;
    cached_capabilities[0].observe = true;
    monitor.hosts.insert(
        "snap".to_owned(),
        CachedRemoteHost {
            workstreams: vec![NavigatorWorkstream {
                host: NavigatorHost::Remote {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                result_ready: true,
                ..row(workstream_id, NavigatorRuntimeStatus::Working)
            }],
            unresolved_operation_count: 0,
            unresolved_operations: Vec::new(),
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: cached_capabilities,
            reachability: RemoteHostReachability::Unreachable(
                RemoteHostIssue::SshOrRemoteExecutableUnavailable,
            ),
            pending: false,
            next_poll: Instant::now(),
            backoff: REMOTE_INITIAL_BACKOFF,
        },
    );

    let snapshot = monitor.combine(LocalNavigatorSnapshot::default());
    let cached = snapshot.workstreams.first().unwrap();

    assert_eq!(cached.runtime_status, NavigatorRuntimeStatus::Working);
    assert!(cached.result_ready);
    assert!(!cached.host.is_reachable());
    assert_eq!(snapshot.unreachable_hosts, vec!["snap"]);
    assert_eq!(
        NavigatorView::new(snapshot.clone()).footer_status(),
        "snap SSH/wsnav unavailable; showing cached state"
    );
    let host = snapshot
        .hosts
        .iter()
        .find(|host| host.alias == "snap")
        .unwrap();
    assert!(host.provider_capabilities[0].is_new_eligible());
    assert!(!host.provider_is_new_eligible(ProviderKind::Codex));
}

#[test]
fn local_snapshot_caches_fresh_provider_capabilities() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();

    let snapshot = local_snapshot(&root).unwrap();
    let capabilities = &snapshot.hosts[0].provider_capabilities;

    assert_eq!(
        capabilities
            .iter()
            .map(|capability| capability.kind)
            .collect::<Vec<_>>(),
        crate::protocol::KNOWN_PROVIDER_KINDS,
    );
}

#[test]
fn combined_snapshot_reuses_installation_evidence_and_refreshes_registry_readiness() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let codex_calls = Cell::new(0);
    let tmux_calls = Cell::new(0);
    let installation_cache = crate::provider::InstallationProbeCache::probe_with(
        |program| match program {
            "codex" => {
                codex_calls.set(codex_calls.get() + 1);
                true
            }
            "tmux" => {
                tmux_calls.set(tmux_calls.get() + 1);
                true
            }
            _ => false,
        },
        crate::provider::opencode::InstallationProbe::Available,
    );
    let mut remote = RemoteMonitor::new();
    remote.set_installation_cache(installation_cache);

    let first = combined_snapshot(&root, &mut remote, None).unwrap();
    assert_eq!(codex_calls.get(), 1);
    assert_eq!(tmux_calls.get(), 1);
    assert_eq!(
        first.hosts[0].provider_capabilities[0].reason,
        crate::protocol::ProviderCapabilityReason::ObserverNotReady
    );

    let mut registry = HostRegistry::open(&root).unwrap();
    registry
        .record_codex_integration(
            crate::provider::codex::profile::ProfileOwnership {
                canonical_path: PathBuf::from("/tmp/wsnav-observer.json"),
                owner_id: "owner".to_owned(),
                profile_schema_version: 2,
                hook_executable: PathBuf::from("/tmp/wsnav"),
                content_hash: "hash".to_owned(),
            },
            crate::state::IntegrationLifecycle::Ready,
        )
        .unwrap();
    drop(registry);

    let refreshed = combined_snapshot(&root, &mut remote, None).unwrap();
    assert_eq!(codex_calls.get(), 1);
    assert_eq!(tmux_calls.get(), 1);
    assert_eq!(
        refreshed.hosts[0].provider_capabilities[0].status,
        crate::protocol::ProviderCapabilityStatus::Available
    );
}

#[test]
fn reachable_remote_snapshot_replaces_cached_provider_capabilities() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let mut catalog = ClientCatalog::open(&root).unwrap();
    let mut monitor = RemoteMonitor::new();
    monitor.hosts.insert(
        "snap".to_owned(),
        CachedRemoteHost {
            workstreams: Vec::new(),
            unresolved_operation_count: 0,
            unresolved_operations: Vec::new(),
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: SnapshotResponse::default().provider_capabilities,
            reachability: RemoteHostReachability::Unreachable(RemoteHostIssue::Checking),
            pending: false,
            next_poll: Instant::now(),
            backoff: REMOTE_INITIAL_BACKOFF,
        },
    );
    let mut snapshot = SnapshotResponse::default();
    snapshot.provider_capabilities[0].status = crate::protocol::ProviderCapabilityStatus::Available;
    snapshot.provider_capabilities[0].reason = crate::protocol::ProviderCapabilityReason::None;
    snapshot.provider_capabilities[0].fresh_launch = true;
    snapshot.provider_capabilities[0].exact_resume = true;
    snapshot.provider_capabilities[0].observe = true;
    let expected = snapshot.provider_capabilities.clone();
    monitor
        .sender
        .send(RemotePollResult {
            alias: "snap".to_owned(),
            host_id: HostId::new(),
            outcome: Ok((snapshot, crate::protocol::OperationsResponse::default())),
        })
        .unwrap();

    monitor.collect(Instant::now(), &mut catalog).unwrap();

    assert_eq!(monitor.hosts["snap"].provider_capabilities, expected);
    assert!(monitor.hosts["snap"].reachability.is_reachable());
}

#[test]
fn combined_snapshot_interleaves_hosts_by_visible_activity_age() {
    let local_older = WorkstreamId::new();
    let local_unknown = WorkstreamId::new();
    let remote_newest = WorkstreamId::new();
    let remote_middle = WorkstreamId::new();
    let mut monitor = RemoteMonitor::new();
    monitor.hosts.insert(
        "snap".to_owned(),
        CachedRemoteHost {
            workstreams: vec![
                NavigatorWorkstream {
                    last_activity_at_millis: Some(3_000),
                    ..row(remote_middle, NavigatorRuntimeStatus::Idle)
                },
                NavigatorWorkstream {
                    last_activity_at_millis: Some(5_000),
                    ..row(remote_newest, NavigatorRuntimeStatus::Working)
                },
            ],
            unresolved_operation_count: 0,
            unresolved_operations: Vec::new(),
            observer_status: ObserverStatus::NotInstalled,
            provider_capabilities: SnapshotResponse::default().provider_capabilities,
            reachability: RemoteHostReachability::Reachable,
            pending: false,
            next_poll: Instant::now(),
            backoff: REMOTE_INITIAL_BACKOFF,
        },
    );

    let snapshot = monitor.combine(LocalNavigatorSnapshot {
        workstreams: vec![
            NavigatorWorkstream {
                last_activity_at_millis: Some(1_000),
                ..row(local_older, NavigatorRuntimeStatus::Idle)
            },
            row(local_unknown, NavigatorRuntimeStatus::Parked),
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    assert_eq!(
        snapshot
            .workstreams
            .iter()
            .map(|workstream| workstream.workstream_id)
            .collect::<Vec<_>>(),
        vec![remote_newest, remote_middle, local_older, local_unknown]
    );
}

#[test]
fn equal_or_unknown_activity_uses_stable_identity_fallbacks() {
    let local = NavigatorWorkstream {
        last_activity_at_millis: Some(1_000),
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
    };
    let remote = NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
        },
        last_activity_at_millis: Some(1_000),
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
    };
    let unknown = NavigatorWorkstream {
        last_activity_at_millis: None,
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
    };
    let remote_unknown = NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
        },
        last_activity_at_millis: None,
        ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
    };

    assert_eq!(compare_workstream_activity(&local, &remote), Ordering::Less);
    assert_eq!(
        compare_workstream_activity(&local, &unknown),
        Ordering::Less
    );
    assert_eq!(
        compare_workstream_activity(&unknown, &remote_unknown),
        Ordering::Less
    );
}

#[test]
fn selection_uses_host_and_workstream_identity_together() {
    let shared_workstream_id = WorkstreamId::new();
    let local = row(shared_workstream_id, NavigatorRuntimeStatus::Idle);
    let remote = NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
        },
        ..row(shared_workstream_id, NavigatorRuntimeStatus::Working)
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![local.clone(), remote.clone()],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });
    view.select_next();

    view.replace_snapshot(LocalNavigatorSnapshot {
        workstreams: vec![remote, local],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    assert_eq!(view.selected().unwrap().host.alias(), "snap");
}

#[test]
fn creation_selects_the_exact_destination_on_the_same_host() {
    let source = WorkstreamId::new();
    let destination = WorkstreamId::new();
    let remote_destination = NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
        },
        ..row(destination, NavigatorRuntimeStatus::Starting)
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![
            row(source, NavigatorRuntimeStatus::Idle),
            remote_destination,
        ],
        hosts: Vec::new(),
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: 0,
        unresolved_operations: Vec::new(),
    });

    assert!(view.select_workstream("snap", destination));
    assert_eq!(view.selected().unwrap().workstream_id, destination);
    assert_eq!(view.selected().unwrap().host.alias(), "snap");
    assert!(!view.select_workstream("local", destination));
}

#[test]
fn creation_output_accepts_one_typed_destination_identifier() {
    let destination = WorkstreamId::new();

    assert_eq!(
        parse_created_workstream(format!("forked workstream {destination}\n").as_bytes()).unwrap(),
        destination
    );
    assert!(matches!(
        parse_created_workstream(b"forked workstream not-an-id\n"),
        Err(NavigatorError::InvalidActionResult)
    ));
}

#[test]
fn acknowledgement_uses_the_current_attention_revision_not_first_result() {
    let mut attention = AttentionState::new(WorkstreamId::new());
    attention
        .mark_result(
            crate::domain::ProviderSessionId::codex("session-a").unwrap(),
            "turn-a".to_owned(),
        )
        .unwrap();
    let first_result_revision = attention.result_unseen_since_revision.unwrap();
    attention
        .mark_result(
            crate::domain::ProviderSessionId::codex("session-a").unwrap(),
            "turn-b".to_owned(),
        )
        .unwrap();

    assert_ne!(attention.revision, first_result_revision);
    assert_eq!(
        acknowledgement_revision(Some(&attention)),
        Some(attention.revision)
    );
}

fn assert_bindings_in_order(bindings: &str, expected: &[&str]) {
    let positions = expected
        .iter()
        .map(|binding| bindings.find(binding).expect("expected compact binding"))
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

fn compact_keys(view: &NavigatorView) -> String {
    view.compact_key_lines(80)
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect()
}

#[test]
fn provider_choice_refuses_zero_and_auto_selects_sole_codex() {
    let source = row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle);
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![source.clone()],
        hosts: vec![NavigatorHostOverview {
            alias: "local".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: capabilities(true, false),
        }],
        ..LocalNavigatorSnapshot::default()
    });
    assert_eq!(
        view.provider_choice_for_new(&source),
        ProviderChoice::Immediate(ProviderKind::Codex)
    );

    view.snapshot.hosts[0].provider_capabilities = capabilities(false, false);
    assert_eq!(view.provider_choice_for_new(&source), ProviderChoice::None);
}

#[test]
fn provider_choice_uses_source_provider_and_preserves_source_identity() {
    let location_id = LocationId::new();
    let mut source = row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle);
    source.host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    source.provider = ProviderKind::OpenCode;
    source.location_id = location_id;
    let view = NavigatorView::new(LocalNavigatorSnapshot {
        workstreams: vec![source.clone()],
        hosts: vec![NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: capabilities(true, true),
        }],
        ..LocalNavigatorSnapshot::default()
    });

    let ProviderChoice::Modal {
        providers,
        selected,
    } = view.provider_choice_for_new(&source)
    else {
        panic!("multiple eligible providers should open a chooser");
    };
    assert_eq!(providers, vec![ProviderKind::Codex, ProviderKind::OpenCode]);
    assert_eq!(selected, 1);

    let intent = ProviderChoiceIntent::New { source };
    let ProviderChoiceIntent::New { source } = intent else {
        unreachable!();
    };
    assert_eq!(source.host.alias(), "snap");
    assert_eq!(source.location_id, location_id);
    assert_eq!(source.provider, ProviderKind::OpenCode);
}

#[test]
fn creation_children_propagate_exact_provider_without_changing_fork_arity() {
    let source = row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle);
    assert_eq!(
        creation_command_arguments(
            CreationAction::Independent,
            &source,
            Some(ProviderKind::OpenCode)
        )
        .unwrap(),
        vec![
            "new-workstream".to_owned(),
            source.workstream_id.to_string(),
            "--provider".to_owned(),
            "opencode".to_owned(),
        ]
    );
    assert_eq!(
        creation_command_arguments(CreationAction::Fork, &source, None).unwrap(),
        vec![
            "fork-workstream".to_owned(),
            source.workstream_id.to_string()
        ]
    );

    let mut remote = source.clone();
    remote.host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    assert_eq!(
        creation_command_arguments(
            CreationAction::Independent,
            &remote,
            Some(ProviderKind::Codex)
        )
        .unwrap(),
        vec![
            "host".to_owned(),
            "new".to_owned(),
            "snap".to_owned(),
            remote.workstream_id.to_string(),
            remote.workstream_revision.value().to_string(),
            "--provider".to_owned(),
            "codex".to_owned(),
        ]
    );
    assert_eq!(
        creation_command_arguments(CreationAction::Fork, &remote, None).unwrap(),
        vec![
            "host".to_owned(),
            "fork".to_owned(),
            "snap".to_owned(),
            remote.workstream_id.to_string(),
            remote.workstream_revision.value().to_string(),
        ]
    );
}

#[test]
fn registration_provider_chooser_keeps_pending_host_and_directory_intent() {
    let temporary = tempfile::tempdir().unwrap();
    let root = StateRoot::create(temporary.path().join("state")).unwrap();
    let host = NavigatorHost::Local;
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        hosts: vec![NavigatorHostOverview {
            alias: "local".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: capabilities(true, true),
        }],
        ..LocalNavigatorSnapshot::default()
    });
    let mut remote = RemoteMonitor::new();
    register_project_browser_directory(&root, &mut remote, &mut view, &host, "projects/demo");

    let Some(NavigatorModal::SelectProvider {
        providers,
        selected,
        intent:
            ProviderChoiceIntent::Register {
                host: pending_host,
                relative_path,
            },
    }) = view.modal
    else {
        panic!("multiple registration providers should open a chooser");
    };
    assert_eq!(providers, vec![ProviderKind::Codex, ProviderKind::OpenCode]);
    assert_eq!(selected, 0);
    assert_eq!(pending_host, NavigatorHost::Local);
    assert_eq!(relative_path, "projects/demo");
}

#[test]
fn offered_provider_becomes_non_confirmable_when_host_turns_unreachable() {
    let host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        hosts: vec![NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: capabilities(true, true),
        }],
        ..LocalNavigatorSnapshot::default()
    });
    assert!(view.provider_choice_is_current(&host, ProviderKind::OpenCode));

    view.snapshot.hosts[0].reachability =
        RemoteHostReachability::Unreachable(RemoteHostIssue::ControlCommunicationFailed);
    assert!(!view.provider_choice_is_current(&host, ProviderKind::OpenCode));
}

#[test]
fn offered_provider_becomes_non_confirmable_when_current_capability_changes() {
    let host = NavigatorHost::Remote {
        alias: "snap".to_owned(),
        reachability: RemoteHostReachability::Reachable,
    };
    let mut view = NavigatorView::new(LocalNavigatorSnapshot {
        hosts: vec![NavigatorHostOverview {
            alias: "snap".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status: ObserverStatus::Ready,
            provider_capabilities: capabilities(true, true),
        }],
        ..LocalNavigatorSnapshot::default()
    });
    assert!(view.provider_choice_is_current(&host, ProviderKind::OpenCode));

    view.snapshot.hosts[0].provider_capabilities = capabilities(true, false);
    assert!(!view.provider_choice_is_current(&host, ProviderKind::OpenCode));
}

fn row(workstream_id: WorkstreamId, runtime_status: NavigatorRuntimeStatus) -> NavigatorWorkstream {
    NavigatorWorkstream {
        host: NavigatorHost::Local,
        project_id: ProjectId::new(),
        location_id: LocationId::new(),
        workstream_id,
        provider: ProviderKind::Codex,
        project_label: "project".to_owned(),
        remote_identity_display: None,
        location_label: "project".to_owned(),
        display_name: "thread".to_owned(),
        runtime_status,
        archived: false,
        result_ready: false,
        recovery_required: false,
        attention_revision: None,
        last_activity_at_millis: None,
        workstream_revision: Revision::INITIAL,
    }
}

fn capabilities(codex: bool, opencode: bool) -> Vec<ProviderCapability> {
    [
        (ProviderKind::Codex, codex),
        (ProviderKind::OpenCode, opencode),
    ]
    .into_iter()
    .map(|(kind, eligible)| {
        if eligible {
            ProviderCapability {
                kind,
                status: crate::protocol::ProviderCapabilityStatus::Available,
                reason: crate::protocol::ProviderCapabilityReason::None,
                fresh_launch: true,
                exact_resume: true,
                observe: true,
                metadata_read: true,
                rename: true,
                fork: true,
            }
        } else {
            ProviderCapability {
                kind,
                status: crate::protocol::ProviderCapabilityStatus::Unavailable,
                reason: if kind == ProviderKind::Codex {
                    crate::protocol::ProviderCapabilityReason::ObserverNotReady
                } else {
                    crate::protocol::ProviderCapabilityReason::AdapterUnavailable
                },
                fresh_launch: false,
                exact_resume: false,
                observe: false,
                metadata_read: false,
                rename: false,
                fork: false,
            }
        }
    })
    .collect()
}
