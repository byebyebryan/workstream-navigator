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
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
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

/// The exact bordered list geometry shared by D17 rendering and mouse hit
/// testing. Footer growth therefore cannot shift the clickable card region
/// away from what is visible on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct D17ListGeometry {
    pub(crate) outer: Rect,
    pub(crate) inner: Rect,
}

/// The process-local D17 cursor and page state. It intentionally contains no
/// provider chooser, browser cursor, directory selection, or Project action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17Model {
    snapshot: D17Snapshot,
    page: D17Page,
    selected: Option<D17RowId>,
    guidance: Option<&'static str>,
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

    /// Transfers the presentation-local cursor to the managed card created by
    /// one exact provisional Runtime promotion.
    pub(crate) fn select_runtime(&mut self, runtime_id: RuntimeId) -> bool {
        self.model.select_runtime(runtime_id)
    }

    /// Sets bounded presentation-local guidance after an unavailable D17
    /// action. This never crosses into provider panes or durable state.
    pub(crate) fn set_guidance(&mut self, guidance: &'static str) {
        self.model.set_guidance(guidance);
    }

    #[must_use]
    pub(crate) fn handle_key(&mut self, key: KeyCode) -> D17Command {
        self.model.handle_key(key)
    }

    /// Computes the exact list geometry used by the renderer for hit testing.
    #[must_use]
    pub(crate) fn list_geometry(&self, area: Rect) -> D17ListGeometry {
        list_geometry(area, &self.model)
    }

    /// Resolves one terminal coordinate to an actionable D17 card. Both lines
    /// of a card resolve to the same identity; project headings and footer
    /// coordinates deliberately do not resolve.
    #[must_use]
    pub(crate) fn row_at(&self, area: Rect, column: u16, row: u16) -> Option<D17RowId> {
        let geometry = self.list_geometry(area);
        if column < geometry.inner.x
            || row < geometry.inner.y
            || column >= geometry.inner.x.saturating_add(geometry.inner.width)
            || row >= geometry.inner.y.saturating_add(geometry.inner.height)
        {
            return None;
        }
        self.model
            .row_id_at_render_line(usize::from(row.saturating_sub(geometry.inner.y)))
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
            guidance: None,
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

    pub(crate) const fn guidance(&self) -> Option<&'static str> {
        self.guidance
    }

    pub(crate) fn set_guidance(&mut self, guidance: &'static str) {
        self.guidance = Some(guidance);
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

    /// Selects one exact visible card and performs its primary action. This is
    /// the mouse equivalent of selecting the row and pressing Enter.
    pub(crate) fn activate_row(&mut self, row_id: D17RowId) -> D17Command {
        if !self.rows().iter().any(|row| row.id() == Some(row_id)) {
            return D17Command::None;
        }
        self.selected = Some(row_id);
        self.activate_selected()
    }

    /// Selects the active managed card that owns one exact Runtime. The
    /// Runtime identity comes from the consumed provisional marker, not from
    /// card ordering or provider metadata.
    pub(crate) fn select_runtime(&mut self, runtime_id: RuntimeId) -> bool {
        let workstream_id = self.snapshot.workstreams.iter().find_map(|workstream| {
            (!workstream.archived
                && workstream
                    .runtime
                    .is_some_and(|runtime| runtime.runtime_id == runtime_id))
            .then_some(workstream.workstream_id)
        });
        let Some(workstream_id) = workstream_id else {
            return false;
        };
        self.page = D17Page::Workstreams;
        self.selected = Some(D17RowId::Workstream(workstream_id));
        true
    }

    /// Resolves one rendered list line to its exact actionable identity.
    /// Project headings occupy one line and both card kinds occupy two.
    #[must_use]
    pub(crate) fn row_id_at_render_line(&self, line: usize) -> Option<D17RowId> {
        let mut cursor = 0_usize;
        for row in self.rows() {
            let height = match row {
                D17Row::ProjectHeader { .. } => 1,
                D17Row::ProvisionalShell | D17Row::Workstream(_) => 2,
            };
            if (cursor..cursor.saturating_add(height)).contains(&line) {
                return row.id();
            }
            cursor = cursor.saturating_add(height);
        }
        None
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
    let footer_height = footer_height(area, model);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
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
    if let Some(guidance) = model.guidance() {
        let controls_height = controls_height(model, layout[1].width).min(layout[1].height);
        let guidance_height = layout[1].height.saturating_sub(controls_height);
        let footer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(guidance_height),
                Constraint::Length(controls_height),
            ])
            .split(layout[1]);
        frame.render_widget(
            Paragraph::new(guidance).wrap(Wrap { trim: true }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow))
                    .title(Span::styled(" Status ", Style::default().fg(Color::Yellow))),
            ),
            footer[0],
        );
        frame.render_widget(
            Paragraph::new(controls_lines(model, footer[1].width)),
            footer[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(controls_lines(model, layout[1].width)),
            layout[1],
        );
    }
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

fn control_bindings(model: &D17Model) -> &'static [(&'static str, &'static str)] {
    match model.page() {
        D17Page::Workstreams => match model.selected() {
            Some(D17RowId::Workstream(workstream_id))
                if model
                    .selected_workstream(workstream_id)
                    .is_some_and(|workstream| {
                        !workstream.archived && workstream.onboarding.is_none()
                    }) =>
            {
                &[
                    ("↑↓", "select"),
                    ("Enter", "open"),
                    ("n", "new here"),
                    (".", "archived"),
                    ("q", "quit"),
                ]
            }
            Some(D17RowId::ProvisionalShell) => &[
                ("↑↓", "select"),
                ("Enter", "shell"),
                (".", "archived"),
                ("q", "quit"),
            ],
            _ => &[("↑↓", "select"), (".", "archived"), ("q", "quit")],
        },
        D17Page::Archived => &[("↑↓", "select"), ("w", "workstreams"), ("q", "quit")],
    }
}

fn controls_lines(model: &D17Model, width: u16) -> Vec<Line<'static>> {
    let key = Style::default().fg(Color::Yellow);
    let label = Style::default().fg(Color::Gray);
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1_usize;
    for (shortcut, description) in control_bindings(model) {
        let binding_width = display_width(shortcut)
            .saturating_add(1)
            .saturating_add(display_width(description));
        let separator_width = usize::from(used > 1) * 2;
        if used > 1
            && used
                .saturating_add(separator_width)
                .saturating_add(binding_width)
                > width
        {
            lines.push(Line::from(spans));
            spans = vec![Span::raw(" ")];
            used = 1;
        } else if separator_width > 0 {
            spans.push(Span::raw("  "));
            used = used.saturating_add(separator_width);
        }
        spans.push(Span::styled((*shortcut).to_owned(), key));
        spans.push(Span::raw(" "));
        spans.push(Span::styled((*description).to_owned(), label));
        used = used.saturating_add(binding_width);
    }
    lines.push(Line::from(spans));
    lines
}

fn controls_height(model: &D17Model, width: u16) -> u16 {
    u16::try_from(controls_lines(model, width).len())
        .unwrap_or(u16::MAX)
        .max(1)
}

fn footer_height(area: Rect, model: &D17Model) -> u16 {
    let controls = controls_height(model, area.width);
    let desired = model.guidance().map_or(controls, |guidance| {
        status_block_height(area, guidance).saturating_add(controls)
    });
    desired.min(area.height.saturating_sub(1))
}

fn status_block_height(area: Rect, guidance: &str) -> u16 {
    let content_width = usize::from(area.width.saturating_sub(2).max(1));
    let content_height = wrapped_display_line_count(guidance, content_width).max(1);
    u16::try_from(content_height)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height.saturating_sub(2))
}

fn wrapped_display_line_count(value: &str, width: usize) -> usize {
    debug_assert!(width > 0);
    value
        .split('\n')
        .map(|line| wrapped_logical_line_count(line, width))
        .sum()
}

fn wrapped_logical_line_count(value: &str, width: usize) -> usize {
    let mut lines = 1_usize;
    let mut used = 0_usize;
    for word in value.split_whitespace() {
        let word_width = display_width(word);
        if used > 0 && used.saturating_add(1).saturating_add(word_width) <= width {
            used = used.saturating_add(1).saturating_add(word_width);
            continue;
        }
        if used > 0 {
            lines = lines.saturating_add(1);
        }
        if word_width > width {
            lines = lines.saturating_add(word_width.saturating_sub(1) / width);
            used = word_width % width;
            if used == 0 {
                used = width;
            }
        } else {
            used = word_width;
        }
    }
    lines
}

fn display_width(value: &str) -> usize {
    Line::raw(value).width()
}

fn list_geometry(area: Rect, model: &D17Model) -> D17ListGeometry {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(footer_height(area, model)),
        ])
        .split(area);
    let outer = vertical[0];
    let inner = Rect::new(
        outer.x.saturating_add(1),
        outer.y.saturating_add(1),
        outer.width.saturating_sub(2),
        outer.height.saturating_sub(2),
    );
    D17ListGeometry { outer, inner }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use ratatui::layout::Rect;

    use super::{D17Command, D17Model, D17Navigator, D17Page, D17Row, D17RowId};
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
    fn both_lines_of_each_card_are_exact_mouse_targets_but_headers_are_not() {
        let (snapshot, active, _) = snapshot();
        let navigator = D17Navigator::new(snapshot);
        let area = Rect::new(0, 0, 32, 24);
        let geometry = navigator.list_geometry(area);
        let x = geometry.inner.x;

        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y),
            Some(D17RowId::ProvisionalShell)
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(1)),
            Some(D17RowId::ProvisionalShell)
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(2)),
            None
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(3)),
            Some(D17RowId::Workstream(active))
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(4)),
            Some(D17RowId::Workstream(active))
        );
        assert_eq!(
            navigator.row_at(
                area,
                x,
                geometry.outer.y.saturating_add(geometry.outer.height)
            ),
            None
        );
    }

    #[test]
    fn mouse_activation_selects_the_exact_managed_card_and_requests_attach() {
        let (mut snapshot, active, _) = snapshot();
        let runtime_id = RuntimeId::from(Uuid::from_u128(5));
        snapshot.workstreams[0].runtime = Some(D17RuntimeSnapshot {
            runtime_id,
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        });
        let mut model = D17Model::new(snapshot);

        assert_eq!(
            model.activate_row(D17RowId::Workstream(active)),
            D17Command::Attach {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
                runtime_id,
                expected_runtime_revision: Revision::INITIAL,
            }
        );
        assert_eq!(model.selected(), Some(D17RowId::Workstream(active)));
    }

    #[test]
    fn promotion_transfers_selection_from_shell_to_its_managed_runtime_card() {
        let (snapshot, active, _) = snapshot();
        let mut model = D17Model::new(snapshot.clone());
        let runtime_id = RuntimeId::from(Uuid::from_u128(5));
        let mut promoted = snapshot;
        promoted.workstreams[0].runtime = Some(D17RuntimeSnapshot {
            runtime_id,
            status: RuntimeStatus::Starting,
            revision: Revision::INITIAL,
        });

        model.replace_snapshot(promoted);
        assert_eq!(model.selected(), Some(D17RowId::ProvisionalShell));
        assert!(model.select_runtime(runtime_id));
        assert_eq!(model.selected(), Some(D17RowId::Workstream(active)));
    }

    #[test]
    fn narrow_footer_keeps_every_complete_binding_visible() {
        let (mut snapshot, _, _) = snapshot();
        snapshot.workstreams[0].runtime = Some(D17RuntimeSnapshot {
            runtime_id: RuntimeId::from(Uuid::from_u128(5)),
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        });
        let mut model = D17Model::new(snapshot);
        model.select_next();

        let lines = super::controls_lines(&model, 32);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "↑↓ select",
            "Enter open",
            "n new here",
            ". archived",
            "q quit",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}: {rendered}"
            );
        }
        assert!(lines.iter().all(|line| line.width() <= 32));
        assert_eq!(
            super::controls_height(&model, 32),
            u16::try_from(lines.len()).unwrap()
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
