//! Pure D17 Workstreams interaction model.
//!
//! This module turns the bounded schema-14 snapshot into display/action
//! intent. It does not open state, materialize a provisional shell, attach a
//! provider, or render a provider pane.

#![allow(
    dead_code,
    reason = "the D17 Workstreams navigator remains unreachable until the atomic cutover"
)]

use std::collections::BTreeMap;

use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::{
    d17_snapshot::{D17OnboardingStatus, D17Snapshot, D17WorkstreamSnapshot},
    domain::{ProviderKind, Revision, RuntimeId, WorkstreamId},
};

/// The only ordinary D17 Navigator pages.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum D17Page {
    #[default]
    Workstreams,
    Archived,
}

impl D17Page {
    #[must_use]
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Workstreams => "Workstreams",
            Self::Archived => "Archived",
        }
    }
}

/// Process-local selection identity. The shell card has no durable row and is
/// deliberately a singleton distinct from every Workstream identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum D17RowId {
    ProvisionalShell,
    Workstream(WorkstreamId),
}

/// One rendered D17 Workstreams row. Project headings are context only and can
/// never become an action target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum D17Row {
    ProvisionalShell,
    ProjectHeader { display_name: String },
    Workstream(D17WorkstreamSnapshot),
}

impl D17Row {
    #[must_use]
    pub(crate) const fn id(&self) -> Option<D17RowId> {
        match self {
            Self::ProvisionalShell => Some(D17RowId::ProvisionalShell),
            Self::ProjectHeader { .. } => None,
            Self::Workstream(workstream) => Some(D17RowId::Workstream(workstream.workstream_id)),
        }
    }
}

/// The only effects a D17 terminal controller may request. Provider kind and
/// location are carried only for contextual same-session actions; a new shell
/// deliberately has neither field because the native command owns both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum D17Command {
    None,
    Quit,
    MaterializeProvisionalShell,
    Attach {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        runtime_id: RuntimeId,
        expected_runtime_revision: Revision,
    },
    NewAtSameLocation {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
}

/// The process-local D17 cursor and page state. It intentionally contains no
/// provider chooser, browser cursor, directory selection, or Project action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17Model {
    snapshot: D17Snapshot,
    page: D17Page,
    selected: Option<D17RowId>,
}

/// Thin D17 Navigator wrapper. It owns only presentation-local selection and
/// rendering state; state, shell materialization, provider launch, and tmux
/// attachment stay outside this dormant seam until the atomic cutover.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17Navigator {
    model: D17Model,
}

impl D17Navigator {
    #[must_use]
    pub(crate) fn new(snapshot: D17Snapshot) -> Self {
        Self {
            model: D17Model::new(snapshot),
        }
    }

    #[must_use]
    pub(crate) const fn model(&self) -> &D17Model {
        &self.model
    }

    pub(crate) const fn model_mut(&mut self) -> &mut D17Model {
        &mut self.model
    }

    pub(crate) fn replace_snapshot(&mut self, snapshot: D17Snapshot) {
        self.model.replace_snapshot(snapshot);
    }

    #[must_use]
    pub(crate) fn handle_key(&mut self, key: KeyCode) -> D17Command {
        self.model.handle_key(key)
    }

    /// Renders the D17-only Workstreams/Archived surface. The renderer has no
    /// provider-pane, shell, state, or filesystem effect.
    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        render_model(frame, area, &self.model);
    }
}

impl D17Model {
    #[must_use]
    pub(crate) fn new(snapshot: D17Snapshot) -> Self {
        Self {
            snapshot,
            page: D17Page::Workstreams,
            selected: Some(D17RowId::ProvisionalShell),
        }
    }

    #[must_use]
    pub(crate) const fn page(&self) -> D17Page {
        self.page
    }

    #[must_use]
    pub(crate) const fn selected(&self) -> Option<D17RowId> {
        self.selected
    }

    #[must_use]
    pub(crate) fn rows(&self) -> Vec<D17Row> {
        rows_for(&self.snapshot, self.page)
    }

    /// Replaces only passive snapshot data while retaining a still-visible
    /// cursor. The derived shell remains the default only on Workstreams.
    pub(crate) fn replace_snapshot(&mut self, snapshot: D17Snapshot) {
        self.snapshot = snapshot;
        self.selected = self
            .selected
            .filter(|selected| self.rows().iter().any(|row| row.id() == Some(*selected)))
            .or_else(|| (self.page == D17Page::Workstreams).then_some(D17RowId::ProvisionalShell))
            .or_else(|| self.rows().iter().find_map(D17Row::id));
    }

    pub(crate) fn select_next(&mut self) {
        self.select_offset(1);
    }

    pub(crate) fn select_previous(&mut self) {
        self.select_offset(-1);
    }

    /// Handles only D17's direct page/navigation/session commands. Native
    /// provider key input never flows through this model.
    #[must_use]
    pub(crate) fn handle_key(&mut self, key: KeyCode) -> D17Command {
        match key {
            KeyCode::Char('q') => D17Command::Quit,
            KeyCode::Char('.') => {
                self.set_page(D17Page::Archived);
                D17Command::None
            }
            KeyCode::Char('w') => {
                self.set_page(D17Page::Workstreams);
                D17Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_previous();
                D17Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                D17Command::None
            }
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Char('n') => self.new_from_selected(),
            _ => D17Command::None,
        }
    }

    fn set_page(&mut self, page: D17Page) {
        self.page = page;
        self.selected = if page == D17Page::Workstreams {
            Some(D17RowId::ProvisionalShell)
        } else {
            self.rows().iter().find_map(D17Row::id)
        };
    }

    fn select_offset(&mut self, offset: isize) {
        let actionable = self
            .rows()
            .into_iter()
            .filter_map(|row| row.id())
            .collect::<Vec<_>>();
        let Some(current) = self.selected else {
            self.selected = actionable.first().copied();
            return;
        };
        let Some(index) = actionable.iter().position(|id| *id == current) else {
            self.selected = actionable.first().copied();
            return;
        };
        let length = isize::try_from(actionable.len()).unwrap_or(0);
        if length == 0 {
            self.selected = None;
            return;
        }
        let next = (isize::try_from(index).unwrap_or(0) + offset).rem_euclid(length);
        self.selected = actionable.get(usize::try_from(next).unwrap_or(0)).copied();
    }

    fn activate_selected(&self) -> D17Command {
        if self.page != D17Page::Workstreams {
            return D17Command::None;
        }
        match self.selected {
            Some(D17RowId::ProvisionalShell) => D17Command::MaterializeProvisionalShell,
            Some(D17RowId::Workstream(workstream_id)) => self
                .selected_workstream(workstream_id)
                .filter(|workstream| workstream.onboarding.is_none())
                .and_then(|workstream| {
                    workstream.runtime.map(|runtime| D17Command::Attach {
                        workstream_id,
                        expected_workstream_revision: workstream.revision,
                        runtime_id: runtime.runtime_id,
                        expected_runtime_revision: runtime.revision,
                    })
                })
                .unwrap_or(D17Command::None),
            _ => D17Command::None,
        }
    }

    fn new_from_selected(&self) -> D17Command {
        let Some(D17RowId::Workstream(workstream_id)) = self.selected else {
            return D17Command::None;
        };
        self.selected_workstream(workstream_id)
            .filter(|workstream| !workstream.archived && workstream.onboarding.is_none())
            .map_or(D17Command::None, |workstream| {
                D17Command::NewAtSameLocation {
                    source_workstream_id: workstream.workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                }
            })
    }

    fn selected_workstream(&self, id: WorkstreamId) -> Option<&D17WorkstreamSnapshot> {
        self.snapshot
            .workstreams
            .iter()
            .find(|workstream| workstream.workstream_id == id)
    }
}

fn rows_for(snapshot: &D17Snapshot, page: D17Page) -> Vec<D17Row> {
    let archived = page == D17Page::Archived;
    let mut workstreams_by_project = BTreeMap::<_, Vec<_>>::new();
    for workstream in snapshot
        .workstreams
        .iter()
        .filter(|workstream| workstream.archived == archived)
    {
        workstreams_by_project
            .entry(workstream.project_id)
            .or_default()
            .push(workstream.clone());
    }

    let mut rows = Vec::new();
    if page == D17Page::Workstreams {
        rows.push(D17Row::ProvisionalShell);
    }
    for project in &snapshot.projects {
        let Some(workstreams) = workstreams_by_project.remove(&project.project_id) else {
            continue;
        };
        rows.push(D17Row::ProjectHeader {
            display_name: project.display_name.clone(),
        });
        rows.extend(workstreams.into_iter().map(D17Row::Workstream));
    }
    rows
}

fn render_model(frame: &mut Frame<'_>, area: Rect, model: &D17Model) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);
    let selected = model.selected();
    let rows = model.rows();
    let items = rows
        .iter()
        .map(|row| {
            let item = ListItem::new(row_lines(row));
            if row.id() == selected {
                item.style(Style::default().bg(Color::DarkGray))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let title = format!(" {} ", model.page().title());
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        layout[0],
    );
    frame.render_widget(
        Paragraph::new(controls(model)).style(Style::default().fg(Color::Gray)),
        layout[1],
    );
}

fn row_lines(row: &D17Row) -> Vec<Line<'static>> {
    match row {
        D17Row::ProvisionalShell => vec![
            Line::from(Span::styled(
                " New session · shell ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "   Choose a directory, then run codex or opencode",
                Style::default().fg(Color::Gray),
            )),
        ],
        D17Row::ProjectHeader { display_name } => vec![Line::from(Span::styled(
            display_name.clone(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))],
        D17Row::Workstream(workstream) => {
            let runtime = match workstream.onboarding {
                Some(D17OnboardingStatus::ActionFenced) => "onboarding",
                Some(D17OnboardingStatus::RecoveryRequired) => "recovery",
                None => workstream
                    .runtime
                    .map_or("stopped", |runtime| runtime_status_label(runtime.status)),
            };
            let title = workstream
                .native_name
                .clone()
                .unwrap_or_else(|| workstream.workstream_id.short());
            let indicator = if workstream.onboarding == Some(D17OnboardingStatus::RecoveryRequired)
                || workstream.recovery_unseen
            {
                ("!", Color::Red)
            } else if workstream.result_unseen {
                ("✓", Color::Green)
            } else {
                ("·", Color::Gray)
            };
            vec![
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        provider_label(workstream.provider),
                        provider_color(workstream.provider),
                    ),
                    Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                    Span::styled(runtime, Style::default().fg(Color::Gray)),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(indicator.0, Style::default().fg(indicator.1)),
                    Span::raw(" "),
                    Span::styled(title, Style::default().fg(Color::White)),
                ]),
            ]
        }
    }
}

const fn provider_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Codex => "Codex",
        ProviderKind::OpenCode => "OpenCode",
    }
}

const fn provider_color(provider: ProviderKind) -> Style {
    match provider {
        ProviderKind::Codex => Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        ProviderKind::OpenCode => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    }
}

const fn runtime_status_label(status: crate::domain::RuntimeStatus) -> &'static str {
    match status {
        crate::domain::RuntimeStatus::Starting => "starting",
        crate::domain::RuntimeStatus::Working => "working",
        crate::domain::RuntimeStatus::Idle => "idle",
        crate::domain::RuntimeStatus::Attention => "attention",
        crate::domain::RuntimeStatus::Stopped => "stopped",
        crate::domain::RuntimeStatus::Unknown => "recovery",
    }
}

fn controls(model: &D17Model) -> &'static str {
    match model.page() {
        D17Page::Workstreams => match model.selected() {
            Some(D17RowId::Workstream(workstream_id))
                if model
                    .selected_workstream(workstream_id)
                    .is_some_and(|workstream| {
                        !workstream.archived && workstream.onboarding.is_none()
                    }) =>
            {
                " ↑↓ select  ·  Enter open  ·  n new here  ·  . archived  ·  q quit"
            }
            Some(D17RowId::ProvisionalShell) => {
                " ↑↓ select  ·  Enter shell  ·  . archived  ·  q quit"
            }
            _ => " ↑↓ select  ·  . archived  ·  q quit",
        },
        D17Page::Archived => " ↑↓ select  ·  w workstreams  ·  q quit",
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{D17Command, D17Model, D17Page, D17Row, D17RowId};
    use crate::{
        d17_snapshot::{
            D17LocationSnapshot, D17OnboardingStatus, D17ProjectSnapshot, D17RuntimeSnapshot,
            D17Snapshot, D17WorkstreamSnapshot,
        },
        domain::{
            LocationId, ProjectId, ProviderKind, Revision, RuntimeId, RuntimeStatus, WorkstreamId,
        },
    };

    fn snapshot() -> (D17Snapshot, WorkstreamId, WorkstreamId) {
        let project_id = ProjectId::from(Uuid::from_u128(1));
        let location_id = LocationId::from(Uuid::from_u128(2));
        let active = WorkstreamId::from(Uuid::from_u128(3));
        let archived = WorkstreamId::from(Uuid::from_u128(4));
        (
            D17Snapshot {
                projects: vec![D17ProjectSnapshot {
                    project_id,
                    display_name: "checkout".to_owned(),
                    locations: vec![D17LocationSnapshot {
                        location_id,
                        display_name: "checkout".to_owned(),
                        revision: Revision::INITIAL,
                        is_label_source: true,
                    }],
                }],
                workstreams: vec![
                    D17WorkstreamSnapshot {
                        project_id,
                        location_id,
                        workstream_id: active,
                        provider: ProviderKind::Codex,
                        archived: false,
                        revision: Revision::INITIAL,
                        runtime: None,
                        onboarding: None,
                        native_name: None,
                        result_unseen: false,
                        recovery_unseen: false,
                    },
                    D17WorkstreamSnapshot {
                        project_id,
                        location_id,
                        workstream_id: archived,
                        provider: ProviderKind::OpenCode,
                        archived: true,
                        revision: Revision::INITIAL,
                        runtime: None,
                        onboarding: None,
                        native_name: None,
                        result_unseen: false,
                        recovery_unseen: false,
                    },
                ],
            },
            active,
            archived,
        )
    }

    #[test]
    fn workstreams_always_start_with_one_provisional_shell_card() {
        let (snapshot, _, _) = snapshot();
        let mut model = D17Model::new(snapshot);
        assert_eq!(model.page(), D17Page::Workstreams);
        assert_eq!(model.selected(), Some(D17RowId::ProvisionalShell));
        assert!(matches!(
            model.rows().first(),
            Some(D17Row::ProvisionalShell)
        ));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            D17Command::MaterializeProvisionalShell
        );
    }

    #[test]
    fn shell_card_renders_native_provider_choice_without_a_picker() {
        let (snapshot, _, _) = snapshot();
        let model = D17Model::new(snapshot);
        let rows = model.rows();
        let shell = super::row_lines(&rows[0]);

        assert_eq!(shell[0].spans[0].content.as_ref(), " New session · shell ");
        assert_eq!(
            shell[1].spans[0].content.as_ref(),
            "   Choose a directory, then run codex or opencode"
        );
        assert!(
            shell
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| !span.content.contains("picker"))
        );
    }

    #[test]
    fn new_inherits_the_selected_workstreams_provider_and_location_context() {
        let (snapshot, active, _) = snapshot();
        let mut model = D17Model::new(snapshot);
        model.select_next();
        assert_eq!(model.selected(), Some(D17RowId::Workstream(active)));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            D17Command::NewAtSameLocation {
                source_workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
                provider: ProviderKind::Codex,
            }
        );
    }

    #[test]
    fn onboarding_runtime_stays_visible_but_refuses_attach_and_new() {
        let (mut snapshot, active, _) = snapshot();
        snapshot.workstreams[0].runtime = Some(D17RuntimeSnapshot {
            runtime_id: RuntimeId::from(Uuid::from_u128(5)),
            status: RuntimeStatus::Starting,
            revision: Revision::INITIAL,
        });
        snapshot.workstreams[0].onboarding = Some(D17OnboardingStatus::ActionFenced);
        let mut model = D17Model::new(snapshot);
        model.select_next();

        assert_eq!(model.selected(), Some(D17RowId::Workstream(active)));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            D17Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            D17Command::None
        );
        let rows = model.rows();
        let workstream = rows
            .iter()
            .find(|row| row.id() == Some(D17RowId::Workstream(active)))
            .expect("active onboarding row");
        let lines = super::row_lines(workstream);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content == "onboarding")
        );
    }

    #[test]
    fn archived_page_has_no_shell_card_and_cannot_start_a_new_session() {
        let (snapshot, _, archived) = snapshot();
        let mut model = D17Model::new(snapshot);
        let _ = model.handle_key(crossterm::event::KeyCode::Char('.'));
        assert_eq!(model.selected(), Some(D17RowId::Workstream(archived)));
        assert!(
            model
                .rows()
                .iter()
                .all(|row| !matches!(row, D17Row::ProvisionalShell))
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            D17Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            D17Command::None
        );
    }
}
