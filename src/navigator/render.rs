use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

use crate::{
    domain::{Clock, ProjectId, ProviderKind, SystemClock},
    protocol::{ObserverStatus, ProjectDirectoriesResponse},
};

use super::model::{
    LocalNavigatorSnapshot, NavigatorHost, NavigatorRuntimeStatus, NavigatorWorkstream,
    RemoteHostReachability, provider_label,
};
use super::snapshot::PROJECT_BROWSER_VIEWPORT_ROWS;
use super::view::{
    NavigatorHostSummary, NavigatorListEntry, NavigatorModal, NavigatorPage,
    NavigatorProjectOverview, NavigatorViewMode, TreeBranch, WorkstreamRowContext,
};

pub(in crate::navigator) fn project_browser_scroll_to_selected(
    scroll: &mut usize,
    visible: &[usize],
    selected: usize,
) {
    let Some(position) = visible.iter().position(|index| *index == selected) else {
        *scroll = 0;
        return;
    };
    *scroll = (*scroll).min(visible.len().saturating_sub(1));
    if position < *scroll {
        *scroll = position;
    } else if position >= scroll.saturating_add(PROJECT_BROWSER_VIEWPORT_ROWS) {
        *scroll = position
            .saturating_add(1)
            .saturating_sub(PROJECT_BROWSER_VIEWPORT_ROWS);
    }
}

pub(in crate::navigator) fn navigator_modal_area(outer: Rect, modal: &NavigatorModal) -> Rect {
    let width = outer.width.min(match modal {
        NavigatorModal::ProjectBrowser { .. } => 64,
        _ => 52,
    });
    let desired_height = match modal {
        NavigatorModal::SelectRegistrationHost { hosts, .. } => hosts.len().saturating_add(4),
        NavigatorModal::SelectProvider { providers, .. } => providers.len().saturating_add(4),
        NavigatorModal::ProjectBrowser { directories, .. } => directories
            .entries
            .len()
            .min(PROJECT_BROWSER_VIEWPORT_ROWS)
            .saturating_add(5),
        NavigatorModal::ConfirmArchive(_)
        | NavigatorModal::ConfirmForkRecovery { .. }
        | NavigatorModal::SelectHostRemoval { .. }
        | NavigatorModal::ConfirmForgetProject { .. }
        | NavigatorModal::Rename { .. }
        | NavigatorModal::ConfigureProjectBrowserRoot { .. }
        | NavigatorModal::RegisterHost { .. } => 7,
    };
    let height = outer
        .height
        .min(u16::try_from(desired_height).unwrap_or(u16::MAX));
    Rect::new(
        outer
            .x
            .saturating_add(outer.width.saturating_sub(width) / 2),
        outer
            .y
            .saturating_add(outer.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

#[allow(clippy::too_many_lines)]
pub(in crate::navigator) fn navigator_modal_content(
    modal: NavigatorModal,
    content_width: usize,
) -> (String, Vec<Line<'static>>) {
    let key = Style::default().fg(Color::Yellow);
    match modal {
        NavigatorModal::ConfirmArchive(workstream) => (
            " Archive working Workstream ".to_owned(),
            vec![
                Line::from(Span::styled(
                    truncate_display(&workstream.display_name, 42),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::raw(format!(
                    "This parks the working {} Runtime before archiving.",
                    provider_label(workstream.provider)
                )),
                Line::from(vec![
                    Span::styled("Enter/y", key),
                    Span::raw(" confirm   "),
                    Span::styled("Esc/n", key),
                    Span::raw(" cancel"),
                ]),
            ],
        ),
        NavigatorModal::ConfirmForkRecovery { source, .. } => (
            " Finish earlier Fork ".to_owned(),
            vec![
                Line::from(Span::styled(
                    truncate_display(&source.display_name, 42),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::raw("An earlier Fork did not finish confirming its destination."),
                Line::from(vec![
                    Span::styled("Enter/y", key),
                    Span::raw(" reconcile it   "),
                    Span::styled("n", key),
                    Span::raw(" start another Fork"),
                ]),
                Line::from(vec![Span::styled("Esc", key), Span::raw(" cancel")]),
            ],
        ),
        NavigatorModal::SelectHostRemoval {
            alias,
            workstream_count,
            location_count,
            unresolved_operation_count,
            offboard,
        } => host_removal_modal(
            alias,
            workstream_count,
            location_count,
            unresolved_operation_count,
            offboard,
            key,
        ),
        NavigatorModal::ConfirmForgetProject {
            label,
            archived_workstream_count,
            location_count,
            ..
        } => forget_project_modal(label, archived_workstream_count, location_count, key),
        NavigatorModal::Rename { workstream, value } => (
            " Rename Workstream ".to_owned(),
            vec![
                Line::raw(format!(
                    "Set the canonical {} thread title:",
                    provider_label(workstream.provider)
                )),
                Line::from(Span::styled(
                    truncate_display(&value, 44),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(vec![
                    Span::styled("Enter", key),
                    Span::raw(" save   "),
                    Span::styled("Esc", key),
                    Span::raw(" cancel"),
                ]),
            ],
        ),
        NavigatorModal::SelectRegistrationHost { hosts, selected } => {
            registration_host_picker_modal(hosts, selected, key)
        }
        NavigatorModal::SelectProvider {
            providers,
            selected,
            ..
        } => provider_picker_modal(providers, selected, key),
        NavigatorModal::ProjectBrowser {
            ref host,
            ref directories,
            selected,
            scroll,
            ref filter,
        } => project_browser_modal(
            host,
            directories,
            selected,
            scroll,
            filter,
            content_width,
            key,
        ),
        NavigatorModal::ConfigureProjectBrowserRoot { host, value } => {
            project_browser_root_modal(&host, &value, key)
        }
        NavigatorModal::RegisterHost { value } => register_host_modal(&value, key),
    }
}

fn project_browser_root_modal(
    host: &NavigatorHost,
    value: &str,
    key: Style,
) -> (String, Vec<Line<'static>>) {
    (
        format!(" Project browser root · {} ", host.alias()),
        vec![
            Line::raw("Set the host-local root (for example ~/code):"),
            Line::from(Span::styled(
                truncate_display(value, 44),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::raw("Absolute paths remain on the selected host."),
            Line::from(vec![
                Span::styled("Enter", key),
                Span::raw(" save   "),
                Span::styled("Esc", key),
                Span::raw(" cancel"),
            ]),
        ],
    )
}

fn project_browser_modal(
    host: &NavigatorHost,
    directories: &ProjectDirectoriesResponse,
    selected: usize,
    scroll: usize,
    filter: &str,
    content_width: usize,
    key: Style,
) -> (String, Vec<Line<'static>>) {
    let visible = project_browser_entry_indexes(directories, filter);
    let cursor = if directories.relative_path.is_empty() {
        directories.root_label.clone()
    } else {
        format!("{}/{}", directories.root_label, directories.relative_path)
    };
    let entry_width = content_width.saturating_sub(4).max(1);
    let mut lines = vec![
        Line::from(Span::styled(
            truncate_display(&cursor, content_width),
            Style::default().fg(Color::Cyan),
        )),
        Line::raw(if filter.is_empty() {
            format!("{} folders", visible.len())
        } else {
            truncate_display(&format!("Filter: {filter}"), content_width)
        }),
    ];
    if visible.is_empty() {
        lines.push(Line::raw("  no matching folders"));
    } else {
        for index in visible
            .into_iter()
            .skip(scroll)
            .take(PROJECT_BROWSER_VIEWPORT_ROWS)
        {
            let entry = &directories.entries[index];
            let marker = if index == selected { "> " } else { "  " };
            let git = if entry.is_git_repository { " ✓" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(
                    marker,
                    if index == selected {
                        key
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(
                    truncate_display(&entry.name, entry_width),
                    if index == selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
                Span::styled(git, Style::default().fg(Color::Green)),
            ]));
        }
    }
    lines.push(Line::from(Span::styled(
        truncate_display("Enter open/add · r add · h up", content_width),
        key,
    )));
    (format!(" Add Project · {} ", host.alias()), lines)
}

pub(in crate::navigator) fn project_browser_entry_indexes(
    directories: &ProjectDirectoriesResponse,
    filter: &str,
) -> Vec<usize> {
    let filter = filter.to_lowercase();
    directories
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.name.to_lowercase().contains(&filter).then_some(index))
        .collect()
}

fn host_removal_modal(
    alias: String,
    workstream_count: usize,
    location_count: usize,
    unresolved_operation_count: usize,
    offboard: bool,
    key: Style,
) -> (String, Vec<Line<'static>>) {
    let keep_marker = if offboard { "  " } else { "> " };
    let offboard_marker = if offboard { "> " } else { "  " };
    (
        " Remove remote Host ".to_owned(),
        vec![
            modal_emphasis(alias),
            Line::from(vec![
                Span::styled(keep_marker, if offboard { Style::default() } else { key }),
                Span::raw("disconnect: forget WSNav registration; keep observer"),
            ]),
            Line::from(vec![
                Span::styled(
                    offboard_marker,
                    if offboard { key } else { Style::default() },
                ),
                Span::raw("offboard: remove observer, then forget registration"),
            ]),
            Line::raw(format!(
                "Retains {workstream_count} Workstreams and {unresolved_operation_count} operation{}; removes {location_count} local Project locations.",
                if unresolved_operation_count == 1 {
                    ""
                } else {
                    "s"
                }
            )),
            Line::from(vec![
                Span::styled("↑/↓", key),
                Span::raw(" choose   "),
                Span::styled("Enter", key),
                Span::raw(" continue   "),
                Span::styled("Esc", key),
                Span::raw(" cancel"),
            ]),
        ],
    )
}

fn forget_project_modal(
    label: String,
    archived_workstream_count: usize,
    location_count: usize,
    key: Style,
) -> (String, Vec<Line<'static>>) {
    (
        " Remove Project from WSNav ".to_owned(),
        vec![
            modal_emphasis(label),
            Line::raw(format!(
                "Hides {archived_workstream_count} archived Workstream{} at {location_count} location{}.",
                if archived_workstream_count == 1 {
                    ""
                } else {
                    "s"
                },
                if location_count == 1 { "" } else { "s" },
            )),
            Line::raw("Retains host state, Git project files, and native provider history."),
            confirmation_line("remove", key),
        ],
    )
}

fn registration_host_picker_modal(
    hosts: Vec<NavigatorHost>,
    selected: usize,
    key: Style,
) -> (String, Vec<Line<'static>>) {
    let mut lines = vec![Line::raw("Choose the host that owns this Project:")];
    lines.extend(hosts.into_iter().enumerate().map(|(index, host)| {
        let marker = if index == selected { "> " } else { "  " };
        let availability = if host.is_reachable() {
            ""
        } else {
            " · unavailable"
        };
        Line::from(Span::styled(
            format!("{marker}{}{}", host.alias(), availability),
            if index == selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ))
    }));
    lines.push(Line::from(vec![
        Span::styled("Enter", key),
        Span::raw(" choose   "),
        Span::styled("Esc", key),
        Span::raw(" cancel"),
    ]));
    (" Add Project · choose host ".to_owned(), lines)
}

fn provider_picker_modal(
    providers: Vec<ProviderKind>,
    selected: usize,
    key: Style,
) -> (String, Vec<Line<'static>>) {
    let mut lines = vec![Line::raw("Choose the provider for this Workstream:")];
    lines.extend(providers.into_iter().enumerate().map(|(index, provider)| {
        let marker = if index == selected { "> " } else { "  " };
        Line::from(Span::styled(
            format!("{marker}{}", provider_label(provider)),
            if index == selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ))
    }));
    lines.push(Line::from(vec![
        Span::styled("↑/↓", key),
        Span::raw(" choose   "),
        Span::styled("Enter", key),
        Span::raw(" confirm   "),
        Span::styled("Esc", key),
        Span::raw(" cancel"),
    ]));
    (" Select provider ".to_owned(), lines)
}

fn register_host_modal(value: &str, key: Style) -> (String, Vec<Line<'static>>) {
    (
        " Register remote host ".to_owned(),
        vec![
            Line::raw("Enter a configured SSH host alias (for example, snap):"),
            modal_emphasis(truncate_display(value, 44)),
            Line::raw("Uses the standard remote wsnav installation."),
            Line::from(vec![
                Span::styled("Enter", key),
                Span::raw(" verify and register   "),
                Span::styled("Esc", key),
                Span::raw(" cancel"),
            ]),
        ],
    )
}

fn modal_emphasis(value: String) -> Line<'static> {
    Line::from(Span::styled(
        value,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))
}

fn confirmation_line(action: &'static str, key: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled("Enter/y", key),
        Span::raw(format!(" {action}   ")),
        Span::styled("Esc/n", key),
        Span::raw(" cancel"),
    ])
}

pub(in crate::navigator) fn binding_line(bindings: &[(&str, &str)]) -> Line<'static> {
    let key = Style::default().fg(Color::Yellow);
    let label = Style::default().fg(Color::Gray);
    let mut spans = vec![Span::raw(" ")];
    for (index, (shortcut, description)) in bindings.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled((*shortcut).to_owned(), key));
        spans.push(Span::raw(" "));
        spans.push(Span::styled((*description).to_owned(), label));
    }
    Line::from(spans)
}

pub(in crate::navigator) fn help_lines(
    page: NavigatorPage,
    showing_detail: bool,
    showing_recovery: bool,
    workstream_view: NavigatorViewMode,
) -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::Yellow);
    let mut lines = vec![
        Line::from(Span::styled("Navigation", heading)),
        Line::from(vec![Span::styled("↑/↓ or j/k", key), Span::raw("  select")]),
        Line::from(vec![
            Span::styled(",", key),
            Span::raw("          Projects page"),
        ]),
        Line::from(vec![
            Span::styled(".", key),
            Span::raw("          Hosts page"),
        ]),
    ];
    if showing_detail {
        lines.extend([
            Line::raw(""),
            Line::from(Span::styled("Details", heading)),
            Line::from(vec![
                Span::styled("Enter", key),
                Span::raw(if showing_recovery {
                    "      reconcile selected operation"
                } else {
                    "      return to the list"
                }),
            ]),
            Line::from(vec![
                Span::styled("Esc", key),
                Span::raw("        return to the list"),
            ]),
        ]);
    } else if page == NavigatorPage::Workstreams {
        lines.extend(workstream_help_lines(workstream_view, heading, key));
    } else {
        let mut page_lines = vec![
            Line::raw(""),
            Line::from(Span::styled(page.label(), heading)),
        ];
        if page == NavigatorPage::Projects {
            page_lines.extend([
                Line::from(vec![
                    Span::styled("a", key),
                    Span::raw("          browse and add a Project"),
                ]),
                Line::from(vec![
                    Span::styled("x", key),
                    Span::raw("          remove an archived Project from WSNav"),
                ]),
            ]);
        } else if page == NavigatorPage::Hosts {
            page_lines.extend([
                Line::from(vec![
                    Span::styled("a", key),
                    Span::raw("          add, verify, and set up a remote SSH host"),
                ]),
                Line::from(vec![
                    Span::styled("s", key),
                    Span::raw("          review the selected Host's Codex observer"),
                ]),
                Line::from(vec![
                    Span::styled("r", key),
                    Span::raw("          set the selected Host's Project browser root"),
                ]),
                Line::from(vec![
                    Span::styled("x", key),
                    Span::raw("          disconnect or offboard the selected remote Host"),
                ]),
            ]);
        }
        lines.extend(page_lines);
    }
    lines
}

fn workstream_help_lines(
    view_mode: NavigatorViewMode,
    heading: Style,
    key: Style,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled("Workstreams", heading)),
        Line::from(vec![
            Span::styled("←/→", key),
            Span::raw("          cycle recent/project/host/archived"),
        ]),
        Line::from(vec![
            Span::styled("Enter", key),
            Span::raw("      open, start, or recover"),
        ]),
        Line::from(vec![
            Span::styled("i", key),
            Span::raw("          show bounded status"),
        ]),
    ];
    if view_mode.is_archived() {
        lines.push(Line::from(vec![
            Span::styled("u", key),
            Span::raw("          restore without starting the native provider"),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Tab", key),
            Span::raw("        focus native agent"),
        ]));
        lines.extend([
            Line::from(vec![
                Span::styled("n", key),
                Span::raw("          new Workstream"),
            ]),
            Line::from(vec![
                Span::styled("f", key),
                Span::raw("          fork at last settled turn"),
            ]),
            Line::from(vec![Span::styled("p", key), Span::raw("          park")]),
            Line::from(vec![
                Span::styled("r", key),
                Span::raw("          rename canonical provider thread"),
            ]),
            Line::from(vec![
                Span::styled("x", key),
                Span::raw("          archive (confirms a working Runtime)"),
            ]),
            Line::from(vec![
                Span::styled("a", key),
                Span::raw("          acknowledge attention"),
            ]),
        ]);
    }
    lines
}

pub(in crate::navigator) const ATTACHMENT_READY_MESSAGE_DURATION: Duration = Duration::from_secs(3);
pub(in crate::navigator) const COMPACT_HINT_LEFT_INSET: usize = 1;
/// A bordered status frame with at most three wrapped content lines.
pub(in crate::navigator) const STATUS_BOX_HEIGHT: u16 = 5;

/// Keep selection distinct from every semantic row foreground. In particular,
/// `DarkGray` is reserved for secondary activity text and the parked marker,
/// so it must never become the selected-row background.
pub(in crate::navigator) const SELECTED_ROW_BACKGROUND: Color = Color::Indexed(236);
pub(in crate::navigator) const PARKED_INDICATOR_COLOR: Color = Color::Indexed(110);

/// Activity age is a neutral brightness ramp rather than another identity or
/// lifecycle color. Recent work should be easiest to spot; stale work remains
/// readable but deliberately recedes.
pub(in crate::navigator) const AGE_UNKNOWN_COLOR: Color = Color::Indexed(244);
pub(in crate::navigator) const AGE_RECENT_COLOR: Color = Color::Indexed(255);
pub(in crate::navigator) const AGE_HOURLY_COLOR: Color = Color::Indexed(251);
pub(in crate::navigator) const AGE_DAILY_COLOR: Color = Color::Indexed(247);
pub(in crate::navigator) const AGE_WEEKLY_COLOR: Color = Color::Indexed(244);
pub(in crate::navigator) const AGE_STALE_COLOR: Color = Color::Indexed(241);
/// Projects pages deliberately avoid near-black supporting text: this pane
/// commonly sits against an unthemed black terminal background.
pub(in crate::navigator) const PROJECT_TREE_COLOR: Color = Color::Indexed(245);
pub(in crate::navigator) const PROJECT_ORIGIN_ICON_COLOR: Color = Color::Indexed(109);
pub(in crate::navigator) const PROJECT_ORIGIN_LABEL_COLOR: Color = Color::Indexed(250);
pub(in crate::navigator) const PROJECT_ACTIVE_COLOR: Color = Color::Green;
pub(in crate::navigator) const PROJECT_ARCHIVED_COLOR: Color = Color::Indexed(110);

pub(in crate::navigator) fn selected_row_style() -> Style {
    Style::default()
        .bg(SELECTED_ROW_BACKGROUND)
        .add_modifier(Modifier::BOLD)
}

pub(in crate::navigator) fn navigator_list_item(
    entry: &NavigatorListEntry,
    snapshot: &LocalNavigatorSnapshot,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
) -> ListItem<'static> {
    match entry {
        NavigatorListEntry::HostHeader { alias } => host_header_item(alias),
        NavigatorListEntry::ProjectHeader { project_id, label } => {
            project_header_item(*project_id, label, project_colors)
        }
        NavigatorListEntry::Workstream {
            snapshot_index,
            context,
            tree_branch,
        } => workstream_item(
            &snapshot.workstreams[*snapshot_index],
            *context,
            *tree_branch,
            project_colors,
            available_width,
        ),
    }
}

fn host_header_item(alias: &str) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            alias.to_owned(),
            Style::default()
                .fg(host_color(alias))
                .add_modifier(Modifier::BOLD),
        ),
    ]))
}

pub(in crate::navigator) fn project_overview_item(
    project: &NavigatorProjectOverview,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
) -> ListItem<'static> {
    let mut lines = vec![Line::from(Span::styled(
        truncate_display(&project.label, usize::from(available_width)),
        Style::default()
            .fg(project_accent(project.project_id, project_colors))
            .add_modifier(Modifier::BOLD),
    ))];
    if let Some(remote_identity_display) = project.remote_identity_display.as_deref() {
        let compact_remote = compact_remote_display(remote_identity_display);
        lines.push(Line::from(vec![
            Span::styled("↗ ", Style::default().fg(PROJECT_ORIGIN_ICON_COLOR)),
            Span::styled(
                truncate_display(
                    compact_remote,
                    usize::from(available_width.saturating_sub(2)),
                ),
                Style::default().fg(PROJECT_ORIGIN_LABEL_COLOR),
            ),
        ]));
    }
    lines.push(project_activity_line(project));
    lines.extend(
        project
            .locations
            .iter()
            .enumerate()
            .map(|(index, location)| {
                let branch = if index + 1 == project.locations.len() {
                    "└─"
                } else {
                    "├─"
                };
                let location_label_width = usize::from(
                    available_width.saturating_sub(
                        u16::try_from(location.host.alias().chars().count().saturating_add(5))
                            .unwrap_or(u16::MAX),
                    ),
                );
                Line::from(vec![
                    Span::styled(branch, Style::default().fg(PROJECT_TREE_COLOR)),
                    Span::styled(
                        location.host.alias().to_owned(),
                        Style::default().fg(host_color(location.host.alias())),
                    ),
                    Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
                    Span::styled(
                        truncate_display(&location.label, location_label_width),
                        Style::default().fg(Color::Gray),
                    ),
                ])
            }),
    );
    ListItem::new(lines)
}

pub(in crate::navigator) fn project_overview_height(project: &NavigatorProjectOverview) -> u16 {
    u16::try_from(
        2_usize
            .saturating_add(usize::from(project.remote_identity_display.is_some()))
            .saturating_add(project.locations.len()),
    )
    .unwrap_or(u16::MAX)
}

pub(in crate::navigator) fn project_activity_line(
    project: &NavigatorProjectOverview,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} active", project.active_workstream_count),
            Style::default().fg(PROJECT_ACTIVE_COLOR),
        ),
        Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
        Span::styled(
            format!("{} archived", project.archived_workstream_count),
            Style::default().fg(PROJECT_ARCHIVED_COLOR),
        ),
    ])
}

/// The canonical remote label retains its host for safe normalization and
/// grouping evidence. The narrow Projects page needs only its repository path.
pub(in crate::navigator) fn compact_remote_display(remote_identity_display: &str) -> &str {
    remote_identity_display
        .split_once('/')
        .map_or(remote_identity_display, |(_, path)| path)
}

pub(in crate::navigator) fn host_overview_height(host: &NavigatorHostSummary) -> u16 {
    u16::try_from(2_usize.saturating_add(host.active_projects.len().max(1))).unwrap_or(u16::MAX)
}

pub(in crate::navigator) fn host_overview_item(
    host: &NavigatorHostSummary,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
) -> ListItem<'static> {
    let mut lines = vec![Line::from(Span::styled(
        truncate_display(&host.alias, usize::from(available_width)),
        Style::default()
            .fg(host_color(&host.alias))
            .add_modifier(Modifier::BOLD),
    ))];
    lines.push(host_connection_line(host, available_width));
    if host.active_projects.is_empty() {
        lines.push(Line::from(Span::styled(
            "└─ no active Projects",
            Style::default().fg(PROJECT_TREE_COLOR),
        )));
    } else {
        lines.extend(
            host.active_projects
                .iter()
                .enumerate()
                .map(|(index, project)| {
                    let branch = if index + 1 == host.active_projects.len() {
                        "└─"
                    } else {
                        "├─"
                    };
                    let count = if project.active_workstream_count == 1 {
                        "1 active".to_owned()
                    } else {
                        format!("{} active", project.active_workstream_count)
                    };
                    Line::from(vec![
                        Span::styled(branch, Style::default().fg(PROJECT_TREE_COLOR)),
                        Span::styled(
                            truncate_display(
                                &project.label,
                                usize::from(available_width.saturating_sub(2)),
                            ),
                            Style::default().fg(project_accent(project.project_id, project_colors)),
                        ),
                        Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
                        Span::styled(count, Style::default().fg(Color::Gray)),
                    ])
                }),
        );
    }
    ListItem::new(lines)
}

pub(in crate::navigator) fn host_connection_line(
    host: &NavigatorHostSummary,
    available_width: u16,
) -> Line<'static> {
    match host.reachability {
        RemoteHostReachability::Reachable => {
            let (observer, observer_color) = observer_status_indicator(host.observer_status);
            Line::from(vec![
                Span::styled("available", Style::default().fg(Color::Indexed(250))),
                Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
                Span::styled(
                    truncate_display(observer, usize::from(available_width.saturating_sub(12))),
                    Style::default().fg(observer_color),
                ),
            ])
        }
        RemoteHostReachability::Unreachable(issue) => Line::from(vec![
            Span::styled("✗ ", Style::default().fg(issue.color())),
            Span::styled(
                truncate_display(
                    &issue.label(),
                    usize::from(available_width.saturating_sub(2)),
                ),
                Style::default().fg(issue.color()),
            ),
        ]),
    }
}

pub(in crate::navigator) const fn observer_status_indicator(
    status: ObserverStatus,
) -> (&'static str, Color) {
    match status {
        ObserverStatus::Ready => ("✓", Color::Green),
        ObserverStatus::TrustPending => ("review needed", Color::Yellow),
        ObserverStatus::Modified => ("observer changed", Color::Yellow),
        ObserverStatus::NotInstalled => ("not set up", Color::Indexed(250)),
        ObserverStatus::Disabled => ("disabled", Color::Indexed(250)),
    }
}

fn project_header_item(
    project_id: ProjectId,
    label: &str,
    project_colors: &BTreeMap<ProjectId, Color>,
) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::raw("  "),
        project_marker(project_id, project_colors),
        Span::raw(" "),
        Span::styled(
            label.to_owned(),
            Style::default()
                .fg(project_accent(project_id, project_colors))
                .add_modifier(Modifier::BOLD),
        ),
    ]))
}

fn workstream_item(
    row: &NavigatorWorkstream,
    context: WorkstreamRowContext,
    tree_branch: Option<TreeBranch>,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
) -> ListItem<'static> {
    let (indicator, indicator_style) = status_indicator(row);
    let thread_style = Style::default().fg(Color::White);
    let (context_prefix, thread_prefix) = tree_prefix(tree_branch);
    ListItem::new(vec![
        workstream_context_line(
            row,
            context,
            context_prefix,
            project_colors,
            available_width,
        ),
        Line::from(thread_line(
            row,
            indicator,
            indicator_style,
            thread_style,
            thread_prefix,
            available_width,
        )),
    ])
}

fn tree_prefix(tree_branch: Option<TreeBranch>) -> (&'static str, &'static str) {
    match tree_branch {
        None => ("   ", " "),
        Some(TreeBranch { is_last: true }) => (" └─ ", "    "),
        Some(TreeBranch { is_last: false }) => (" ├─ ", " │  "),
    }
}

pub(in crate::navigator) fn workstream_context_line(
    row: &NavigatorWorkstream,
    context: WorkstreamRowContext,
    prefix: &str,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
) -> Line<'static> {
    match context {
        WorkstreamRowContext::Recent => recent_context_line(
            prefix,
            &row.project_label,
            row.project_id,
            row.host.alias(),
            row.provider,
            project_colors,
            available_width,
        ),
        WorkstreamRowContext::Host => {
            let prefix_width = Line::raw(prefix).width();
            let fixed_width = prefix_width
                .saturating_add(2)
                .saturating_add(provider_context_width(row.provider));
            let project_budget = usize::from(available_width).saturating_sub(fixed_width);
            let project = truncate_display_width(&row.project_label, project_budget);
            let mut line = vec![Span::raw(prefix.to_owned())];
            line.extend(provider_context_spans(row.provider));
            line.extend([
                project_marker(row.project_id, project_colors),
                Span::raw(" "),
                Span::styled(
                    project,
                    Style::default()
                        .fg(project_accent(row.project_id, project_colors))
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            Line::from(line)
        }
        WorkstreamRowContext::Project => {
            let host_budget = usize::from(available_width)
                .saturating_sub(Line::raw(prefix).width())
                .saturating_sub(provider_context_width(row.provider));
            let host = truncate_display_width(row.host.alias(), host_budget);
            let mut line = vec![Span::raw(prefix.to_owned())];
            line.extend(provider_context_spans(row.provider));
            line.push(Span::styled(
                host,
                Style::default()
                    .fg(host_color(row.host.alias()))
                    .add_modifier(Modifier::BOLD),
            ));
            Line::from(line)
        }
    }
}

/// A Recent row reserves its fixed provider segment first. Within the remaining
/// width it remains intentionally project-first: the host is useful location
/// evidence, but secondary to finding the right workstream, so it occupies a
/// bounded right-aligned column rather than competing with the Project name.
fn recent_context_line(
    prefix: &str,
    project_label: &str,
    project_id: ProjectId,
    host_alias: &str,
    provider: ProviderKind,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
) -> Line<'static> {
    const MIN_PROJECT_WIDTH: usize = 4;
    const MAX_HOST_WIDTH: usize = 12;

    let prefix_width = Line::raw(prefix).width();
    let content_width = usize::from(available_width)
        .saturating_sub(prefix_width)
        .saturating_sub(provider_context_width(provider));
    let host_budget = content_width
        .saturating_sub(MIN_PROJECT_WIDTH.saturating_add(1))
        .min(MAX_HOST_WIDTH);
    let host = truncate_display_width(host_alias, host_budget);
    let host_width = Line::raw(&host).width();
    let project_budget = content_width
        .saturating_sub(host_width.saturating_add(1))
        .max(MIN_PROJECT_WIDTH);
    let project = truncate_display_width(project_label, project_budget);
    let used_width = prefix_width + Line::raw(&project).width() + host_width;
    let padding = usize::from(available_width)
        .saturating_sub(used_width + provider_context_width(provider))
        .max(1);
    let mut line = vec![Span::raw(prefix.to_owned())];
    line.extend(provider_context_spans(provider));
    line.extend([
        Span::styled(
            project,
            Style::default()
                .fg(project_accent(project_id, project_colors))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(padding)),
        Span::styled(
            host,
            Style::default()
                .fg(host_color(host_alias))
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    Line::from(line)
}

fn provider_context_width(provider: ProviderKind) -> usize {
    3 + provider_label(provider).chars().count()
}

fn provider_context_spans(provider: ProviderKind) -> Vec<Span<'static>> {
    vec![
        Span::styled(provider_label(provider), Style::default().fg(Color::Gray)),
        Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
    ]
}

fn project_marker(
    project_id: ProjectId,
    project_colors: &BTreeMap<ProjectId, Color>,
) -> Span<'static> {
    Span::styled(
        "•",
        Style::default().fg(project_accent(project_id, project_colors)),
    )
}

fn project_accent(project_id: ProjectId, project_colors: &BTreeMap<ProjectId, Color>) -> Color {
    *project_colors
        .get(&project_id)
        .expect("every visible Project receives one accent color")
}

pub(in crate::navigator) fn host_color(alias: &str) -> Color {
    if alias == "local" {
        HOST_LABEL_PALETTE[0]
    } else {
        let remote_palette_len = HOST_LABEL_PALETTE.len() - 1;
        let index = stable_color_index(alias.as_bytes(), remote_palette_len);
        HOST_LABEL_PALETTE[index + 1]
    }
}

/// Host labels use a single cool blue family. Project labels deliberately use
/// a separate muted violet family below, so the compact context line reads as
/// `host · project`, not as a string of unrelated colors.
pub(in crate::navigator) const HOST_LABEL_PALETTE: [Color; 4] = [
    Color::LightBlue,
    Color::Indexed(75),
    Color::Indexed(111),
    Color::Indexed(117),
];

/// Projects use a muted violet family that stays distinct from the cool host
/// axis and the green/yellow/red lifecycle-state colors.
pub(in crate::navigator) const PROJECT_MARKER_PALETTE: [Color; 12] = [
    Color::Indexed(96),
    Color::Indexed(97),
    Color::Indexed(98),
    Color::Indexed(133),
    Color::Indexed(134),
    Color::Indexed(139),
    Color::Indexed(140),
    Color::Indexed(141),
    Color::Indexed(146),
    Color::Indexed(147),
    Color::Indexed(176),
    Color::Indexed(177),
];

/// Allocates distinct muted accents to concurrently visible Projects. The
/// Project IDs select a deterministic initial slot; collision probing prevents
/// a same-color wall for the normal dozen-project navigator scope.
pub(in crate::navigator) fn visible_project_colors(
    snapshot: &LocalNavigatorSnapshot,
) -> BTreeMap<ProjectId, Color> {
    let project_ids = snapshot
        .workstreams
        .iter()
        .map(|workstream| workstream.project_id)
        .collect::<BTreeSet<_>>();
    let mut used = [false; PROJECT_MARKER_PALETTE.len()];
    let mut colors = BTreeMap::new();
    for project_id in project_ids {
        let start = stable_color_index(
            project_id.as_uuid().as_bytes(),
            PROJECT_MARKER_PALETTE.len(),
        );
        let index = (0..PROJECT_MARKER_PALETTE.len())
            .map(|offset| (start + offset) % PROJECT_MARKER_PALETTE.len())
            .find(|index| !used[*index])
            .unwrap_or(start);
        used[index] = true;
        colors.insert(project_id, PROJECT_MARKER_PALETTE[index]);
    }
    colors
}

fn stable_color_index(seed: &[u8], palette_len: usize) -> usize {
    debug_assert!(palette_len > 0);
    let hash = seed.iter().fold(0_u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u64::from(*byte))
    });
    usize::try_from(hash % u64::try_from(palette_len).unwrap()).unwrap()
}

pub(in crate::navigator) fn thread_line(
    row: &NavigatorWorkstream,
    indicator: &'static str,
    indicator_style: Style,
    thread_style: Style,
    prefix: &str,
    available_width: u16,
) -> Vec<Span<'static>> {
    let now_millis = SystemClock.now_millis().ok();
    let age = activity_label(row.last_activity_at_millis, now_millis);
    let minimum_title_width = 4;
    let fixed_width = prefix.chars().count() + indicator.chars().count() + 2 + age.chars().count();
    let title_budget = usize::from(available_width)
        .saturating_sub(fixed_width.saturating_add(1))
        .max(minimum_title_width);
    let title = truncate_display(&row.display_name, title_budget);
    let used_width = prefix.chars().count()
        + indicator.chars().count()
        + 1
        + title.chars().count()
        + 1
        + age.chars().count();
    let padding = usize::from(available_width)
        .saturating_sub(used_width)
        .max(1);
    let mut line = vec![
        Span::raw(prefix.to_owned()),
        Span::styled(indicator, indicator_style),
        Span::raw(" "),
        Span::styled(title, thread_style),
    ];
    line.push(Span::raw(" ".repeat(padding)));
    line.push(Span::styled(
        age,
        Style::default().fg(activity_age_color(row.last_activity_at_millis, now_millis)),
    ));
    line
}

fn truncate_display(value: &str, maximum: usize) -> String {
    if maximum == 0 {
        return String::new();
    }
    let mut result = value.chars().take(maximum).collect::<String>();
    if value.chars().nth(maximum).is_some() && maximum > 1 {
        result.pop();
        result.push('…');
    }
    result
}

/// Truncates to terminal-cell width while retaining a visible ellipsis. It is
/// used only where a label is aligned against the far edge of the navigator.
fn truncate_display_width(value: &str, maximum: usize) -> String {
    if maximum == 0 {
        return String::new();
    }
    if Line::raw(value).width() <= maximum {
        return value.to_owned();
    }
    if maximum == 1 {
        return "…".to_owned();
    }

    let mut result = String::new();
    for character in value.chars() {
        let character_width = Line::raw(character.to_string()).width();
        if Line::raw(&result).width() + character_width > maximum.saturating_sub(1) {
            break;
        }
        result.push(character);
    }
    result.push('…');
    result
}

pub(in crate::navigator) fn activity_label(
    last_activity_at_millis: Option<i64>,
    now_millis: Option<i64>,
) -> String {
    relative_activity_age(last_activity_at_millis, now_millis)
        .unwrap_or_else(|| "activity unknown".to_owned())
}

pub(in crate::navigator) fn relative_activity_age(
    last_activity_at_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Option<String> {
    let elapsed_seconds = activity_elapsed_seconds(last_activity_at_millis, now_millis)?;
    Some(match elapsed_seconds {
        0..=59 => "now".to_owned(),
        60..=3_599 => format!("{}m ago", elapsed_seconds / 60),
        3_600..=86_399 => format!("{}h ago", elapsed_seconds / 3_600),
        _ => format!("{}d ago", elapsed_seconds / 86_400),
    })
}

fn activity_elapsed_seconds(
    last_activity_at_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Option<i64> {
    Some(
        now_millis?
            .saturating_sub(last_activity_at_millis?)
            .max(0)
            .saturating_div(1_000),
    )
}

/// Uses a readable neutral ramp, brightest for the newest activity and
/// progressively muted for older work. This deliberately avoids the
/// green/yellow/red lifecycle colors and the host/project identity palettes.
pub(in crate::navigator) fn activity_age_color(
    last_activity_at_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Color {
    match activity_elapsed_seconds(last_activity_at_millis, now_millis) {
        None => AGE_UNKNOWN_COLOR,
        Some(0..=59) => AGE_RECENT_COLOR,
        Some(60..=3_599) => AGE_HOURLY_COLOR,
        Some(3_600..=86_399) => AGE_DAILY_COLOR,
        Some(86_400..=604_799) => AGE_WEEKLY_COLOR,
        Some(_) => AGE_STALE_COLOR,
    }
}

/// Returns a compact user-facing state from bounded lifecycle and attention
/// metadata. Ordinary idle is deliberately unmarked; active and completed
/// work stand out without consuming thread-title space.
pub(in crate::navigator) fn status_indicator(row: &NavigatorWorkstream) -> (&'static str, Style) {
    if !row.host.is_reachable() {
        return ("?", Style::default().fg(Color::Red));
    }
    match row.runtime_status {
        NavigatorRuntimeStatus::RecoveryRequired => ("!", Style::default().fg(Color::Red)),
        NavigatorRuntimeStatus::Working => ("●", Style::default().fg(Color::Yellow)),
        NavigatorRuntimeStatus::Unknown => ("?", Style::default().fg(Color::Red)),
        NavigatorRuntimeStatus::Parked => ("p", Style::default().fg(PARKED_INDICATOR_COLOR)),
        NavigatorRuntimeStatus::Starting => ("…", Style::default().fg(Color::Cyan)),
        NavigatorRuntimeStatus::Idle | NavigatorRuntimeStatus::Attention => {
            if row.result_ready {
                ("✓", Style::default().fg(Color::Green))
            } else {
                (" ", Style::default())
            }
        }
    }
}
