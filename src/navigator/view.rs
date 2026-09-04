//! Pure Workstreams interaction model.
//!
//! This module turns the bounded schema-15 snapshot into display/action
//! intent. It does not open state, materialize a provisional shell, attach a
//! provider, or render a provider pane.

use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::{
    domain::{
        Clock, ProviderKind, Revision, RuntimeId, RuntimeStatus, SystemClock, WorkstreamId,
        WorkstreamLifecycle,
    },
    snapshot::{OnboardingStatus, Snapshot, WorkstreamSnapshot},
};

/// The only ordinary Navigator pages.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Page {
    #[default]
    Workstreams,
    Archived,
}

/// Presentation-local context shown on the provisional shell card. It is
/// derived from the seed or exact live shell cwd, never persisted, and never
/// used as registration or launch authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellLocation {
    cwd: String,
}

impl ShellLocation {
    pub(crate) fn cwd(label: &str) -> Self {
        Self {
            cwd: safe_shell_location_label(label),
        }
    }
}

impl Page {
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
pub(crate) enum RowId {
    ProvisionalShell,
    Workstream(WorkstreamId),
}

/// One rendered Workstreams row. Project headings are context only and can
/// never become an action target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Row {
    ProvisionalShell { location: ShellLocation },
    ProjectHeader { display_name: String },
    Workstream(WorkstreamSnapshot),
}

/// The bounded setup variants that can be shown in a contextual readiness
/// guide. The guide never carries provider argv, prompts, or profile paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverSetupKind {
    Create,
    Update,
    TrustReview,
}

impl Row {
    #[must_use]
    pub(crate) const fn id(&self) -> Option<RowId> {
        match self {
            Self::ProvisionalShell { .. } => Some(RowId::ProvisionalShell),
            Self::ProjectHeader { .. } => None,
            Self::Workstream(workstream) => Some(RowId::Workstream(workstream.workstream_id)),
        }
    }

    #[must_use]
    pub(crate) fn render_height(&self) -> usize {
        match self {
            Self::ProjectHeader { .. } => 1,
            Self::ProvisionalShell { .. } | Self::Workstream(_) => 2,
        }
    }
}

/// The only effects a terminal controller may request. Provider kind and
/// location are carried only for contextual same-session actions; a new shell
/// deliberately has neither field because the native command owns both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
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
    Start {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    Recover {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    Archive {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Restore {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    /// Opens the contextual setup guide before a Codex action can proceed.
    /// The guide carries no provider argv or profile path.
    AcceptObserverSetup {
        kind: ObserverSetupKind,
    },
    /// Dismisses the contextual observer setup guide without any mutation.
    CancelObserverSetup,
    ShowGuidance(&'static str),
}

/// Process-local action confirmation state. It deliberately retains only the
/// exact bounded context required by the selected action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Modal {
    ConfirmArchive {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    ObserverConsent {
        kind: ObserverSetupKind,
    },
}

/// The exact bordered list geometry shared by rendering and mouse hit
/// testing. Footer growth therefore cannot shift the clickable card region
/// away from what is visible on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ListGeometry {
    pub(crate) outer: Rect,
    pub(crate) inner: Rect,
}

/// The process-local cursor and page state. It intentionally contains no
/// provider chooser, browser cursor, directory selection, or Project action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Model {
    snapshot: Snapshot,
    shell_location: ShellLocation,
    page: Page,
    selected: Option<RowId>,
    guidance: Option<&'static str>,
    modal: Option<Modal>,
    help_visible: bool,
}

/// Thin Navigator wrapper. It owns only presentation-local selection and
/// rendering state; state, shell materialization, provider launch, and tmux
/// attachment stay in the controller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Navigator {
    model: Model,
    terminal_focused: bool,
}

impl Navigator {
    #[must_use]
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        Self {
            model: Model::new(snapshot),
            terminal_focused: true,
        }
    }

    /// Updates only the ephemeral visual focus cue from a terminal focus
    /// event. Tmux remains the focus authority; this value never authorizes an
    /// action, changes selection, or enters durable state.
    pub(crate) const fn set_terminal_focused(&mut self, focused: bool) {
        self.terminal_focused = focused;
    }

    pub(crate) const fn model_mut(&mut self) -> &mut Model {
        &mut self.model
    }

    pub(crate) fn replace_snapshot(&mut self, snapshot: Snapshot) {
        self.model.replace_snapshot(snapshot);
    }

    pub(crate) fn set_shell_location(&mut self, location: ShellLocation) {
        self.model.set_shell_location(location);
    }

    /// Transfers the presentation-local cursor to the managed card created by
    /// one exact provisional Runtime promotion.
    pub(crate) fn select_runtime(&mut self, runtime_id: RuntimeId) -> bool {
        self.model.select_runtime(runtime_id)
    }

    /// Moves the process-local cursor to one exact active Workstream after a
    /// lifecycle/creation transition. It is never a durable mutation.
    pub(crate) fn select_workstream(&mut self, workstream_id: WorkstreamId) -> bool {
        self.model.select_workstream(workstream_id)
    }

    /// Sets bounded presentation-local guidance after an unavailable
    /// action. This never crosses into provider panes or durable state.
    pub(crate) fn set_guidance(&mut self, guidance: &'static str) {
        self.model.set_guidance(guidance);
    }

    /// Clears one exact presentation-local guidance message, if it is still
    /// current. A newer unrelated message remains visible.
    pub(crate) fn clear_guidance_if(&mut self, guidance: &'static str) {
        self.model.clear_guidance_if(guidance);
    }

    /// Shows the bounded readiness guide for a Codex observer action. The
    /// guide is presentation-local; native profile setup remains owned by the
    /// account-shell helper after the user explicitly consents there.
    pub(crate) fn request_observer_setup(&mut self, kind: ObserverSetupKind) {
        self.model.request_observer_setup(kind);
    }

    #[must_use]
    pub(crate) fn handle_key(&mut self, key: KeyCode) -> Command {
        self.model.handle_key(key)
    }

    /// Computes the exact list geometry used by the renderer for hit testing.
    #[must_use]
    pub(crate) fn list_geometry(&self, area: Rect) -> ListGeometry {
        list_geometry(area, &self.model)
    }

    /// Resolves one terminal coordinate to an actionable card. Both lines
    /// of a card resolve to the same identity; project headings and footer
    /// coordinates deliberately do not resolve.
    #[must_use]
    pub(crate) fn row_at(&self, area: Rect, column: u16, row: u16) -> Option<RowId> {
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

    /// Renders the read-only Workstreams/Archived surface. The renderer has no
    /// provider-pane, shell, state, or filesystem effect.
    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        self.render_at(frame, area, SystemClock.now_millis().ok());
    }

    /// Renders with an explicit wall-clock sample. Production callers use the
    /// system clock through [`Self::render`]; tests and visual probes can pass
    /// a deterministic value without depending on timing.
    pub(crate) fn render_at(&self, frame: &mut Frame<'_>, area: Rect, now_millis: Option<i64>) {
        render_model(frame, area, &self.model, self.terminal_focused, now_millis);
    }
}

impl Model {
    #[must_use]
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            shell_location: ShellLocation::cwd("unavailable"),
            page: Page::Workstreams,
            selected: Some(RowId::ProvisionalShell),
            guidance: None,
            modal: None,
            help_visible: false,
        }
    }

    #[must_use]
    pub(crate) const fn page(&self) -> Page {
        self.page
    }

    #[must_use]
    pub(crate) const fn selected(&self) -> Option<RowId> {
        self.selected
    }

    pub(crate) const fn guidance(&self) -> Option<&'static str> {
        self.guidance
    }

    #[must_use]
    pub(crate) const fn modal(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    #[must_use]
    pub(crate) const fn help_visible(&self) -> bool {
        self.help_visible
    }

    pub(crate) fn set_guidance(&mut self, guidance: &'static str) {
        self.guidance = Some(guidance);
    }

    /// Clears one exact presentation-local guidance message, if it is still
    /// current. A newer unrelated message remains visible.
    pub(crate) fn clear_guidance_if(&mut self, guidance: &'static str) {
        if self.guidance == Some(guidance) {
            self.guidance = None;
        }
    }

    pub(crate) fn request_observer_setup(&mut self, kind: ObserverSetupKind) {
        self.guidance = None;
        self.modal = Some(Modal::ObserverConsent { kind });
    }

    pub(crate) fn set_shell_location(&mut self, location: ShellLocation) {
        self.shell_location = location;
    }

    #[must_use]
    pub(crate) fn rows(&self) -> Vec<Row> {
        rows_for(&self.snapshot, self.page, &self.shell_location)
    }

    /// Replaces only passive snapshot data while retaining a still-visible
    /// cursor. The derived shell remains the default only on Workstreams.
    pub(crate) fn replace_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.selected = self
            .selected
            .filter(|selected| self.rows().iter().any(|row| row.id() == Some(*selected)))
            .or_else(|| (self.page == Page::Workstreams).then_some(RowId::ProvisionalShell))
            .or_else(|| self.rows().iter().find_map(Row::id));
    }

    pub(crate) fn select_next(&mut self) {
        if self.help_visible || self.modal.is_some() {
            return;
        }
        self.select_offset(1);
    }

    pub(crate) fn select_previous(&mut self) {
        if self.help_visible || self.modal.is_some() {
            return;
        }
        self.select_offset(-1);
    }

    /// Selects one exact visible card and performs its primary action. This is
    /// the mouse equivalent of selecting the row and pressing Enter.
    pub(crate) fn activate_row(&mut self, row_id: RowId) -> Command {
        if self.help_visible || self.modal.is_some() {
            return Command::None;
        }
        if !self.rows().iter().any(|row| row.id() == Some(row_id)) {
            return Command::None;
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
        self.page = Page::Workstreams;
        self.selected = Some(RowId::Workstream(workstream_id));
        true
    }

    pub(crate) fn select_workstream(&mut self, workstream_id: WorkstreamId) -> bool {
        if !self
            .snapshot
            .workstreams
            .iter()
            .any(|workstream| !workstream.archived && workstream.workstream_id == workstream_id)
        {
            return false;
        }
        self.page = Page::Workstreams;
        self.selected = Some(RowId::Workstream(workstream_id));
        true
    }

    /// Resolves one rendered list line to its exact actionable identity.
    /// Project headings occupy one line; the shell and managed cards occupy
    /// two stable lines each.
    #[must_use]
    pub(crate) fn row_id_at_render_line(&self, line: usize) -> Option<RowId> {
        let mut cursor = 0_usize;
        for row in self.rows() {
            let height = row.render_height();
            if (cursor..cursor.saturating_add(height)).contains(&line) {
                return row.id();
            }
            cursor = cursor.saturating_add(height);
        }
        None
    }

    /// Handles only 's direct page/navigation/session commands. Native
    /// provider key input never flows through this model.
    #[must_use]
    pub(crate) fn handle_key(&mut self, key: KeyCode) -> Command {
        if self.help_visible {
            return match key {
                KeyCode::Char('q') | KeyCode::Esc => {
                    self.help_visible = false;
                    Command::None
                }
                _ => Command::None,
            };
        }
        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }
        match key {
            KeyCode::Char('q') => Command::Quit,
            KeyCode::Char('?') => {
                self.help_visible = true;
                Command::None
            }
            KeyCode::Char('.') => {
                self.set_page(Page::Archived);
                Command::None
            }
            KeyCode::Char('w') | KeyCode::Esc => {
                self.set_page(Page::Workstreams);
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_previous();
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                Command::None
            }
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Char('n') => self.new_from_selected(),
            KeyCode::Char('x') => self.archive_selected(),
            KeyCode::Char('u') => self.restore_selected(),
            _ => Command::None,
        }
    }

    fn set_page(&mut self, page: Page) {
        self.page = page;
        self.selected = if page == Page::Workstreams {
            Some(RowId::ProvisionalShell)
        } else {
            self.rows().iter().find_map(Row::id)
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

    fn activate_selected(&self) -> Command {
        match self.selected {
            Some(RowId::ProvisionalShell) if self.page == Page::Workstreams => {
                Command::MaterializeProvisionalShell
            }
            Some(RowId::Workstream(workstream_id)) if self.page == Page::Workstreams => {
                self.primary_workstream_action(workstream_id)
            }
            _ => Command::None,
        }
    }

    fn new_from_selected(&self) -> Command {
        let Some(RowId::Workstream(workstream_id)) = self.selected else {
            return Command::None;
        };
        self.selected_workstream(workstream_id)
            .filter(|workstream| {
                !workstream.archived
                    && workstream.onboarding.is_none()
                    && workstream.lifecycle != WorkstreamLifecycle::RecoveryRequired
            })
            .map_or(Command::None, |workstream| Command::NewAtSameLocation {
                source_workstream_id: workstream.workstream_id,
                expected_workstream_revision: workstream.revision,
                provider: workstream.provider,
            })
    }

    fn primary_workstream_action(&self, workstream_id: WorkstreamId) -> Command {
        let Some(workstream) = self.selected_workstream(workstream_id) else {
            return Command::None;
        };
        match workstream.onboarding {
            Some(OnboardingStatus::ActionFenced) => {
                Command::ShowGuidance(ONBOARDING_IN_PROGRESS_GUIDANCE)
            }
            Some(OnboardingStatus::RecoveryRequired) => {
                Command::ShowGuidance(ONBOARDING_RECOVERY_GUIDANCE)
            }
            None if workstream.lifecycle == WorkstreamLifecycle::RecoveryRequired => {
                Command::Recover {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                }
            }
            None if workstream.lifecycle == WorkstreamLifecycle::Parked
                || workstream.runtime.is_none()
                || workstream.runtime.is_some_and(|runtime| {
                    matches!(
                        runtime.status,
                        RuntimeStatus::Stopped | RuntimeStatus::Unknown
                    )
                }) =>
            {
                Command::Start {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    provider: workstream.provider,
                }
            }
            None => workstream
                .runtime
                .map_or(Command::None, |runtime| Command::Attach {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    runtime_id: runtime.runtime_id,
                    expected_runtime_revision: runtime.revision,
                }),
        }
    }

    fn archive_selected(&mut self) -> Command {
        if self.page != Page::Workstreams {
            return Command::None;
        }
        let Some(RowId::Workstream(workstream_id)) = self.selected else {
            return Command::None;
        };
        let Some(workstream) = self.selected_workstream(workstream_id) else {
            return Command::None;
        };
        match workstream.onboarding {
            Some(OnboardingStatus::ActionFenced) => {
                Command::ShowGuidance(ONBOARDING_IN_PROGRESS_GUIDANCE)
            }
            Some(OnboardingStatus::RecoveryRequired) => {
                self.modal = Some(Modal::ConfirmArchive {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                });
                Command::None
            }
            None if !workstream.archived => {
                self.modal = Some(Modal::ConfirmArchive {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                });
                Command::None
            }
            None => Command::None,
        }
    }

    fn restore_selected(&self) -> Command {
        if self.page != Page::Archived {
            return Command::None;
        }
        let Some(RowId::Workstream(workstream_id)) = self.selected else {
            return Command::None;
        };
        self.selected_workstream(workstream_id)
            .filter(|workstream| workstream.archived)
            .map_or(Command::None, |workstream| Command::Restore {
                workstream_id,
                expected_workstream_revision: workstream.revision,
            })
    }

    fn handle_modal_key(&mut self, key: KeyCode) -> Command {
        if matches!(self.modal, Some(Modal::ObserverConsent { .. })) {
            return match key {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.modal = None;
                    Command::CancelObserverSetup
                }
                KeyCode::Enter | KeyCode::Char('y') => {
                    let Some(Modal::ObserverConsent { kind }) = self.modal.take() else {
                        return Command::None;
                    };
                    Command::AcceptObserverSetup { kind }
                }
                _ => Command::None,
            };
        }
        match key {
            KeyCode::Esc => {
                self.modal = None;
                Command::None
            }
            KeyCode::Char('n') if matches!(self.modal, Some(Modal::ConfirmArchive { .. })) => {
                self.modal = None;
                Command::None
            }
            KeyCode::Char('y') if matches!(self.modal, Some(Modal::ConfirmArchive { .. })) => {
                self.confirm_modal()
            }
            KeyCode::Enter => self.confirm_modal(),
            _ => Command::None,
        }
    }

    fn confirm_modal(&mut self) -> Command {
        let Some(modal) = self.modal.take() else {
            return Command::None;
        };
        match modal {
            Modal::ObserverConsent { kind } => Command::AcceptObserverSetup { kind },
            Modal::ConfirmArchive {
                workstream_id,
                expected_workstream_revision,
            } => Command::Archive {
                workstream_id,
                expected_workstream_revision,
            },
        }
    }

    fn selected_workstream(&self, id: WorkstreamId) -> Option<&WorkstreamSnapshot> {
        self.snapshot
            .workstreams
            .iter()
            .find(|workstream| workstream.workstream_id == id)
    }
}

const ONBOARDING_IN_PROGRESS_GUIDANCE: &str =
    "Managed session onboarding is still in progress; wait for exact provider proof";
const ONBOARDING_RECOVERY_GUIDANCE: &str =
    "Managed session requires onboarding recovery; archive is available after exact cleanup";
/// Returns the single semantic Workstream order shared by Navigator rows and
/// provider-pane cycling. Projects are ordered by the newest included member;
/// children then use their own activity sequence and stable ID tie-breakers.
pub(crate) fn workstreams_in_visual_order(
    snapshot: &Snapshot,
    archived: bool,
) -> Vec<&WorkstreamSnapshot> {
    let mut grouped = BTreeMap::<_, Vec<&WorkstreamSnapshot>>::new();
    for workstream in snapshot
        .workstreams
        .iter()
        .filter(|workstream| workstream.archived == archived)
    {
        grouped
            .entry(workstream.project_id)
            .or_default()
            .push(workstream);
    }
    let mut projects = grouped
        .into_iter()
        .map(|(project_id, mut workstreams)| {
            workstreams.sort_by(|left, right| {
                right
                    .last_activity_sequence
                    .cmp(&left.last_activity_sequence)
                    .then_with(|| left.workstream_id.cmp(&right.workstream_id))
            });
            let newest = workstreams
                .first()
                .map_or(0, |workstream| workstream.last_activity_sequence);
            (project_id, newest, workstreams)
        })
        .collect::<Vec<_>>();
    projects.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    projects
        .into_iter()
        .flat_map(|(_, _, workstreams)| workstreams)
        .collect()
}

fn rows_for(snapshot: &Snapshot, page: Page, shell_location: &ShellLocation) -> Vec<Row> {
    let archived = page == Page::Archived;
    let ordered = workstreams_in_visual_order(snapshot, archived);
    let mut rows = Vec::new();
    if page == Page::Workstreams {
        rows.push(Row::ProvisionalShell {
            location: shell_location.clone(),
        });
    }
    let mut seen_projects = BTreeSet::new();
    for workstream in ordered {
        if seen_projects.insert(workstream.project_id)
            && let Some(project) = snapshot
                .projects
                .iter()
                .find(|project| project.project_id == workstream.project_id)
        {
            rows.push(Row::ProjectHeader {
                display_name: project.display_name.clone(),
            });
        }
        rows.push(Row::Workstream(workstream.clone()));
    }
    rows
}

fn render_model(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &Model,
    terminal_focused: bool,
    now_millis: Option<i64>,
) {
    let content = navigator_inner(area);
    let footer_height = footer_height(content, model);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(content);
    let selected = model.selected();
    let rows = model.rows();
    let available_width = layout[0].width;
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let tree_last = rows
                .get(index.saturating_add(1))
                .is_none_or(|next| !same_project_workstream(row, next));
            let item = ListItem::new(row_lines_at(row, tree_last, available_width, now_millis));
            if row.id() == selected {
                item.style(Style::default().bg(Color::DarkGray))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let title = format!(" {} ", model.page().title());
    frame.render_widget(
        Block::default()
            .borders(navigator_borders())
            .border_style(Style::default().fg(Color::Green))
            .title(Span::styled(title, navigator_title_style(terminal_focused))),
        area,
    );
    frame.render_widget(List::new(items), layout[0]);
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
                    .borders(Borders::TOP)
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
    if model.help_visible() {
        render_help(frame, content, model.page());
    } else if let Some(modal) = model.modal() {
        render_modal(frame, content, modal);
    }
}

fn same_project_workstream(left: &Row, right: &Row) -> bool {
    match (left, right) {
        (Row::Workstream(left), Row::Workstream(right)) => left.project_id == right.project_id,
        _ => false,
    }
}

fn navigator_title_style(terminal_focused: bool) -> Style {
    Style::default()
        .fg(if terminal_focused {
            Color::Green
        } else {
            Color::DarkGray
        })
        .add_modifier(Modifier::BOLD)
}

fn navigator_borders() -> Borders {
    Borders::ALL
}

fn navigator_inner(area: Rect) -> Rect {
    Block::default().borders(navigator_borders()).inner(area)
}

/// Activity age is a neutral brightness ramp. It does not compete with the
/// provider, Project, or lifecycle color axes.
const AGE_UNKNOWN_COLOR: Color = Color::Indexed(244);
const AGE_RECENT_COLOR: Color = Color::Indexed(255);
const AGE_HOURLY_COLOR: Color = Color::Indexed(251);
const AGE_DAILY_COLOR: Color = Color::Indexed(247);
const AGE_WEEKLY_COLOR: Color = Color::Indexed(244);
const AGE_STALE_COLOR: Color = Color::Indexed(241);

const PROJECT_TREE_COLOR: Color = Color::Indexed(245);

fn row_lines_at(
    row: &Row,
    tree_last: bool,
    available_width: u16,
    now_millis: Option<i64>,
) -> Vec<Line<'static>> {
    match row {
        Row::ProvisionalShell { location } => {
            vec![
                Line::from(Span::styled(
                    "Shell",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )),
                shell_context_line(&location.cwd, available_width, Color::Cyan),
            ]
        }
        Row::ProjectHeader { display_name } => vec![Line::from(Span::styled(
            display_name.clone(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))],
        Row::Workstream(workstream) => {
            let branch = if tree_last { "└ " } else { "├ " };
            let continuation = if tree_last { "  " } else { "│ " };
            let (marker, marker_style) = workstream_marker(workstream);
            let title = workstream_name(workstream);
            let age = activity_label(workstream.last_activity_at_millis, now_millis);
            let age_style = Style::default().fg(activity_age_color(
                workstream.last_activity_at_millis,
                now_millis,
            ));
            vec![
                Line::from(context_line(
                    branch,
                    workstream.provider,
                    &age,
                    age_style,
                    available_width,
                )),
                Line::from(thread_line(
                    continuation,
                    marker,
                    marker_style,
                    &title,
                    available_width,
                )),
            ]
        }
    }
}

fn shell_context_line(value: &str, available_width: u16, color: Color) -> Line<'static> {
    let indent_width = usize::from(available_width).min(2);
    let indent = " ".repeat(indent_width);
    let content_budget = usize::from(available_width).saturating_sub(indent_width);
    Line::from(vec![
        Span::raw(indent),
        Span::styled(
            truncate_display(value, content_budget),
            Style::default().fg(color),
        ),
    ])
}

const fn provider_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Codex => "Codex",
        ProviderKind::OpenCode => "OpenCode",
    }
}

fn workstream_marker(workstream: &WorkstreamSnapshot) -> (&'static str, Style) {
    if workstream.onboarding == Some(OnboardingStatus::RecoveryRequired)
        || workstream.lifecycle == WorkstreamLifecycle::RecoveryRequired
    {
        ("!", Style::default().fg(Color::Red))
    } else if workstream.onboarding == Some(OnboardingStatus::ActionFenced) {
        ("…", Style::default().fg(Color::Cyan))
    } else if workstream.lifecycle == WorkstreamLifecycle::Parked {
        (" ", Style::default())
    } else {
        match workstream.runtime.map(|runtime| runtime.status) {
            Some(RuntimeStatus::Working) => ("●", Style::default().fg(Color::Yellow)),
            Some(RuntimeStatus::Starting) => ("…", Style::default().fg(Color::Cyan)),
            Some(RuntimeStatus::Unknown) => ("?", Style::default().fg(Color::Red)),
            Some(RuntimeStatus::Attention) => ("✓", Style::default().fg(Color::Green)),
            Some(RuntimeStatus::Stopped | RuntimeStatus::Idle) | None => (" ", Style::default()),
        }
    }
}

fn workstream_name(workstream: &WorkstreamSnapshot) -> String {
    workstream
        .native_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| workstream.workstream_id.short())
}

fn context_line(
    prefix: &str,
    provider_kind: ProviderKind,
    age: &str,
    age_style: Style,
    available_width: u16,
) -> Vec<Span<'static>> {
    let available_width = usize::from(available_width);
    let prefix = truncate_display(prefix, available_width);
    let prefix_width = display_width(&prefix);
    let content_width = available_width.saturating_sub(prefix_width);
    let provider = truncate_display(provider_label(provider_kind), content_width);
    let provider_width = display_width(&provider);
    let age_budget = content_width
        .saturating_sub(provider_width)
        .saturating_sub(usize::from(!provider.is_empty()));
    let age = truncate_display(age, age_budget);
    let padding = available_width.saturating_sub(
        prefix_width
            .saturating_add(provider_width)
            .saturating_add(display_width(&age)),
    );

    vec![
        Span::styled(prefix, Style::default().fg(PROJECT_TREE_COLOR)),
        Span::styled(provider, provider_color(provider_kind)),
        Span::raw(" ".repeat(padding)),
        Span::styled(age, age_style),
    ]
}

fn thread_line(
    prefix: &str,
    indicator: &str,
    indicator_style: Style,
    title: &str,
    available_width: u16,
) -> Vec<Span<'static>> {
    let available_width = usize::from(available_width);
    let prefix = truncate_display(prefix, available_width);
    let prefix_width = display_width(&prefix);
    let indicator = truncate_display(indicator, available_width.saturating_sub(prefix_width));
    let indicator_width = display_width(&indicator);
    let separator_budget = available_width
        .saturating_sub(prefix_width)
        .saturating_sub(indicator_width);
    let separator = if separator_budget > 0 { " " } else { "" };
    let title_budget = separator_budget.saturating_sub(display_width(separator));
    let title = truncate_display(title, title_budget);
    vec![
        Span::styled(prefix, Style::default().fg(PROJECT_TREE_COLOR)),
        Span::styled(indicator, indicator_style),
        Span::raw(separator),
        Span::styled(title, Style::default().fg(Color::White)),
    ]
}

const fn provider_color(provider: ProviderKind) -> Style {
    match provider {
        ProviderKind::Codex => Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        ProviderKind::OpenCode => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    }
}

fn activity_label(last_activity_at_millis: Option<i64>, now_millis: Option<i64>) -> String {
    relative_activity_age(last_activity_at_millis, now_millis)
        .unwrap_or_else(|| "activity unknown".to_owned())
}

fn relative_activity_age(
    last_activity_at_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Option<String> {
    let elapsed_seconds = activity_elapsed_seconds(last_activity_at_millis, now_millis)?;
    Some(match elapsed_seconds {
        0..=59 => "now".to_owned(),
        60..=3_599 => format!("{} min ago", elapsed_seconds / 60),
        3_600..=86_399 => format!("{} hr ago", elapsed_seconds / 3_600),
        86_400..=172_799 => "1 day ago".to_owned(),
        _ => format!("{} days ago", elapsed_seconds / 86_400),
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

fn activity_age_color(last_activity_at_millis: Option<i64>, now_millis: Option<i64>) -> Color {
    match activity_elapsed_seconds(last_activity_at_millis, now_millis) {
        None => AGE_UNKNOWN_COLOR,
        Some(0..=59) => AGE_RECENT_COLOR,
        Some(60..=3_599) => AGE_HOURLY_COLOR,
        Some(3_600..=86_399) => AGE_DAILY_COLOR,
        Some(86_400..=604_799) => AGE_WEEKLY_COLOR,
        Some(_) => AGE_STALE_COLOR,
    }
}

fn control_bindings(model: &Model) -> &'static [(&'static str, &'static str)] {
    if model.help_visible() {
        return &[];
    }
    if let Some(modal) = model.modal() {
        return match modal {
            Modal::ConfirmArchive { .. } => &[("Enter / y", "archive"), ("n / Esc", "cancel")],
            Modal::ObserverConsent { .. } => &[("Enter / y", "continue"), ("n / Esc", "cancel")],
        };
    }
    match model.page() {
        Page::Workstreams => match model.selected() {
            Some(RowId::Workstream(workstream_id))
                if model
                    .selected_workstream(workstream_id)
                    .is_some_and(|workstream| {
                        !workstream.archived
                            && workstream.onboarding.is_none()
                            && workstream.lifecycle != WorkstreamLifecycle::RecoveryRequired
                    }) =>
            {
                &[
                    ("n", "new"),
                    ("x", "archive"),
                    (".", "archived"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            }
            Some(RowId::Workstream(workstream_id))
                if model
                    .selected_workstream(workstream_id)
                    .is_some_and(|workstream| {
                        !workstream.archived
                            && workstream.onboarding != Some(OnboardingStatus::ActionFenced)
                    }) =>
            {
                &[
                    ("x", "archive"),
                    (".", "archived"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            }
            _ => &[(".", "archived"), ("?", "help"), ("q", "quit")],
        },
        Page::Archived => &[
            ("u", "restore"),
            ("w / Esc", "workstreams"),
            ("?", "help"),
            ("q", "quit"),
        ],
    }
}

/// Page-local Help content for the Navigator pane. Archive and Restore are
/// intentionally never advertised together.
fn help_lines(page: Page) -> Vec<Line<'static>> {
    let mut lines = vec![help_heading("Navigate"), help_binding("↑↓", "select")];
    match page {
        Page::Workstreams => {
            lines.extend([
                help_binding("Enter", "open"),
                help_binding(".", "archived"),
                help_binding("Esc", "back"),
                help_heading("Sessions"),
                help_binding("n", "new at location"),
                help_binding("x", "archive session"),
            ]);
        }
        Page::Archived => {
            lines.extend([
                help_binding("Esc", "back"),
                help_heading("Sessions"),
                help_binding("u", "restore session"),
            ]);
        }
    }
    lines
}

fn help_heading(title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

/// One left-aligned binding per row. The action labels all begin in the same
/// column, making the Help panel easy to scan.
fn help_binding(key: &'static str, action: &'static str) -> Line<'static> {
    const ACTION_COLUMN: usize = 8;
    let key_width = display_width(key);
    Line::from(vec![
        Span::styled(
            key,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(ACTION_COLUMN.saturating_sub(key_width))),
        Span::styled(action, Style::default().fg(Color::White)),
    ])
}

fn render_help(frame: &mut Frame<'_>, area: Rect, page: Page) {
    let overlay = help_overlay(area, page);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Paragraph::new(help_lines(page)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    " Help ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        overlay,
    );
}

/// Centers Help vertically at the Navigator's full inner width while fitting
/// its border and fixed rows exactly to content. A smaller terminal clips.
fn help_overlay(area: Rect, page: Page) -> Rect {
    let content_height =
        u16::try_from(help_lines(page).len().saturating_add(2)).unwrap_or(u16::MAX);
    let height = content_height.min(area.height).max(1);
    Rect::new(
        area.x,
        area.y
            .saturating_add(area.height.saturating_sub(height).saturating_div(2)),
        area.width,
        height,
    )
}

fn render_modal(frame: &mut Frame<'_>, area: Rect, modal: &Modal) {
    let (title, text) = match modal {
        Modal::ObserverConsent { kind } => (
            " Codex observer setup ",
            observer_setup_modal_text(*kind).to_owned(),
        ),
        Modal::ConfirmArchive { .. } => (
            " Archive session ",
            "Archive this managed session? Its live Runtime will be stopped first.\n\nEnter or y confirms; n or Esc cancels.".to_owned(),
        ),
    };
    let overlay = centered_rect(94, 52, area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        overlay,
    );
}

fn observer_setup_modal_text(kind: ObserverSetupKind) -> &'static str {
    match kind {
        ObserverSetupKind::Create => {
            "Codex needs the exact WSNav-owned observer profile before this action can start.\n\nWSNav will create the owned profile after your consent, then open Codex's native /hooks trust review in the right-hand pane. This selected action stays pending and resumes only after exact trust and presentation checks succeed.\n\nEnter or y continues; n or Esc cancels."
        }
        ObserverSetupKind::Update => {
            "The exact WSNav-owned Codex observer declaration needs an explicit update before this action can start.\n\nWSNav will update the owned declaration after your consent, then open Codex's native /hooks trust review in the right-hand pane. This selected action stays pending and resumes only after exact trust and presentation checks succeed.\n\nEnter or y continues; n or Esc cancels."
        }
        ObserverSetupKind::TrustReview => {
            "Codex observer trust needs native review or exact crash-recovery finalization before this action can start.\n\nWSNav will open Codex's native /hooks trust review in the right-hand pane when review is required, or finalize an already-completed exact review. This selected action stays pending and resumes only after exact readiness and presentation checks succeed.\n\nEnter or y continues; n or Esc cancels."
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let width = area
        .width
        .saturating_mul(percent_x)
        .saturating_div(100)
        .max(1);
    let height = area
        .height
        .saturating_mul(percent_y)
        .saturating_div(100)
        .max(1);
    Rect::new(
        area.x
            .saturating_add(area.width.saturating_sub(width).saturating_div(2)),
        area.y
            .saturating_add(area.height.saturating_sub(height).saturating_div(2)),
        width,
        height,
    )
}

fn controls_lines(model: &Model, width: u16) -> Vec<Line<'static>> {
    let bindings = control_bindings(model);
    if bindings.is_empty() {
        return Vec::new();
    }
    let key = Style::default().fg(Color::Yellow);
    let label = Style::default().fg(Color::Gray);
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1_usize;
    for (shortcut, description) in bindings {
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

fn controls_height(model: &Model, width: u16) -> u16 {
    u16::try_from(controls_lines(model, width).len()).unwrap_or(u16::MAX)
}

fn footer_height(area: Rect, model: &Model) -> u16 {
    let controls = controls_height(model, area.width);
    let desired = model.guidance().map_or(controls, |guidance| {
        status_block_height(area, guidance).saturating_add(controls)
    });
    desired.min(area.height.saturating_sub(1))
}

fn status_block_height(area: Rect, guidance: &str) -> u16 {
    let content_width = usize::from(area.width.max(1));
    let content_height = wrapped_display_line_count(guidance, content_width).max(1);
    u16::try_from(content_height)
        .unwrap_or(u16::MAX)
        .saturating_add(1)
        .min(area.height.saturating_sub(1))
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

fn truncate_display(value: &str, maximum: usize) -> String {
    if maximum == 0 {
        return String::new();
    }
    if display_width(value) <= maximum {
        return value.to_owned();
    }
    if maximum == 1 {
        return "…".to_owned();
    }
    let content_budget = maximum - 1;
    let mut result = String::new();
    let mut width = 0_usize;
    for character in value.chars() {
        let character_width = display_width(&character.to_string());
        if width.saturating_add(character_width) > content_budget {
            break;
        }
        result.push(character);
        width = width.saturating_add(character_width);
    }
    result.push('…');
    result
}

fn safe_shell_location_label(value: &str) -> String {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || is_unicode_format(character))
    {
        return "unavailable".to_owned();
    }
    let mut bounded = value.chars().take(256).collect::<String>();
    if bounded.len() < value.len() {
        bounded.push('…');
    }
    bounded
}

fn is_unicode_format(character: char) -> bool {
    matches!(
        character as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x13..=0x1A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

fn list_geometry(area: Rect, model: &Model) -> ListGeometry {
    let content = navigator_inner(area);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(footer_height(content, model)),
        ])
        .split(content);
    let outer = vertical[0];
    let inner = outer;
    ListGeometry { outer, inner }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Color, widgets::Borders};

    use super::{
        Command, Modal, Model, Navigator, ObserverSetupKind, Page, Row, RowId, ShellLocation,
        workstream_marker, workstreams_in_visual_order,
    };
    use crate::{
        domain::{
            LocationId, ProjectId, ProviderKind, Revision, RuntimeId, RuntimeStatus, WorkstreamId,
            WorkstreamLifecycle,
        },
        snapshot::{
            LocationSnapshot, OnboardingStatus, ProjectSnapshot, RuntimeSnapshot, Snapshot,
            WorkstreamSnapshot,
        },
    };

    fn snapshot() -> (Snapshot, WorkstreamId, WorkstreamId) {
        let project_id = ProjectId::from(Uuid::from_u128(1));
        let location_id = LocationId::from(Uuid::from_u128(2));
        let active = WorkstreamId::from(Uuid::from_u128(3));
        let archived = WorkstreamId::from(Uuid::from_u128(4));
        (
            Snapshot {
                projects: vec![ProjectSnapshot {
                    project_id,
                    display_name: "checkout".to_owned(),
                    locations: vec![LocationSnapshot {
                        location_id,
                        display_name: "checkout".to_owned(),
                        revision: Revision::INITIAL,
                        is_label_source: true,
                    }],
                }],
                workstreams: vec![
                    WorkstreamSnapshot {
                        project_id,
                        location_id,
                        workstream_id: active,
                        provider: ProviderKind::Codex,
                        lifecycle: WorkstreamLifecycle::Open,
                        archived: false,
                        last_activity_sequence: 1,
                        last_activity_at_millis: None,
                        revision: Revision::INITIAL,
                        runtime: None,
                        onboarding: None,
                        native_name: None,
                    },
                    WorkstreamSnapshot {
                        project_id,
                        location_id,
                        workstream_id: archived,
                        provider: ProviderKind::OpenCode,
                        lifecycle: WorkstreamLifecycle::Open,
                        archived: true,
                        last_activity_sequence: 1,
                        last_activity_at_millis: None,
                        revision: Revision::INITIAL,
                        runtime: None,
                        onboarding: None,
                        native_name: None,
                    },
                ],
                unresolved_operations: vec![],
            },
            active,
            archived,
        )
    }

    fn visual_order_snapshot() -> (Snapshot, Vec<WorkstreamId>, Vec<WorkstreamId>) {
        let project_low = ProjectId::from(Uuid::from_u128(10));
        let project_high = ProjectId::from(Uuid::from_u128(11));
        let location_low = LocationId::from(Uuid::from_u128(12));
        let location_high = LocationId::from(Uuid::from_u128(13));
        let low_first = WorkstreamId::from(Uuid::from_u128(20));
        let low_second = WorkstreamId::from(Uuid::from_u128(21));
        let high_first = WorkstreamId::from(Uuid::from_u128(22));
        let archived = WorkstreamId::from(Uuid::from_u128(23));
        let workstream =
            |project_id, location_id, workstream_id, archived, last_activity_sequence| {
                WorkstreamSnapshot {
                    project_id,
                    location_id,
                    workstream_id,
                    provider: ProviderKind::Codex,
                    lifecycle: WorkstreamLifecycle::Open,
                    archived,
                    last_activity_sequence,
                    last_activity_at_millis: None,
                    revision: Revision::INITIAL,
                    runtime: None,
                    onboarding: None,
                    native_name: None,
                }
            };
        (
            Snapshot {
                projects: vec![
                    ProjectSnapshot {
                        project_id: project_high,
                        display_name: "high".to_owned(),
                        locations: vec![],
                    },
                    ProjectSnapshot {
                        project_id: project_low,
                        display_name: "low".to_owned(),
                        locations: vec![],
                    },
                ],
                workstreams: vec![
                    workstream(project_low, location_low, low_first, false, 7),
                    workstream(project_low, location_low, low_second, false, 7),
                    workstream(project_high, location_high, high_first, false, 7),
                    workstream(project_high, location_high, archived, true, 99),
                ],
                unresolved_operations: vec![],
            },
            vec![low_first, low_second, high_first],
            vec![archived],
        )
    }

    #[test]
    fn visual_order_and_rows_share_newest_member_project_order() {
        let (snapshot, expected_active, expected_archived) = visual_order_snapshot();
        let ordered = workstreams_in_visual_order(&snapshot, false);
        assert_eq!(
            ordered
                .iter()
                .map(|workstream| workstream.workstream_id)
                .collect::<Vec<_>>(),
            expected_active
        );
        let model = Model::new(snapshot.clone());
        let row_workstreams = model
            .rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Workstream(workstream) => Some(workstream.workstream_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(row_workstreams, expected_active);
        assert_eq!(
            workstreams_in_visual_order(&snapshot, true)
                .into_iter()
                .map(|workstream| workstream.workstream_id)
                .collect::<Vec<_>>(),
            expected_archived
        );
    }

    #[test]
    fn workstreams_always_start_with_one_provisional_shell_card() {
        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);
        assert_eq!(model.page(), Page::Workstreams);
        assert_eq!(model.selected(), Some(RowId::ProvisionalShell));
        assert!(matches!(
            model.rows().first(),
            Some(Row::ProvisionalShell { .. })
        ));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            Command::MaterializeProvisionalShell
        );
    }

    #[test]
    fn terminal_focus_changes_only_the_navigator_title_color() {
        let (snapshot, _, _) = snapshot();
        let mut navigator = Navigator::new(snapshot);
        let model = navigator.model.clone();

        assert_eq!(
            super::navigator_title_style(navigator.terminal_focused).fg,
            Some(Color::Green)
        );
        navigator.set_terminal_focused(false);
        assert_eq!(navigator.model, model);
        assert_eq!(
            super::navigator_title_style(navigator.terminal_focused).fg,
            Some(Color::DarkGray)
        );
    }

    #[test]
    fn navigator_frame_wraps_the_list_and_footer() {
        let borders = super::navigator_borders();
        assert!(borders.contains(Borders::LEFT));
        assert!(borders.contains(Borders::TOP));
        assert!(borders.contains(Borders::BOTTOM));
        assert!(borders.contains(Borders::RIGHT));

        assert_eq!(
            super::navigator_inner(Rect::new(0, 0, 32, 24)),
            Rect::new(1, 1, 30, 22)
        );
    }

    #[test]
    fn selecting_an_active_workstream_from_archived_returns_to_workstreams() {
        let (snapshot, active, _) = snapshot();
        let mut model = Model::new(snapshot);
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('.')),
            Command::None
        );
        assert_eq!(model.page(), Page::Archived);
        assert!(model.select_workstream(active));
        assert_eq!(model.page(), Page::Workstreams);
        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
    }

    #[test]
    fn shell_card_renders_live_context_without_onboarding_instructions() {
        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);
        model.set_shell_location(ShellLocation::cwd("~/c/wsnav"));
        let rows = model.rows();
        let shell = super::row_lines_at(&rows[0], true, 30, None);

        assert_eq!(shell[0].spans[0].content.as_ref(), "Shell");
        assert_eq!(shell[1].spans[1].content.as_ref(), "~/c/wsnav");
        assert_eq!(shell.len(), 2);
        assert!(
            shell[1]
                .spans
                .iter()
                .map(|span| super::display_width(span.content.as_ref()))
                .sum::<usize>()
                <= 30
        );
        let narrow = super::row_lines_at(&rows[0], true, 1, None);
        assert!(
            narrow[1]
                .spans
                .iter()
                .map(|span| super::display_width(span.content.as_ref()))
                .sum::<usize>()
                <= 1
        );
        assert!(shell.iter().flat_map(|line| &line.spans).all(|span| {
            !span.content.contains("Choose a directory")
                && !span.content.contains("codex")
                && !span.content.contains("opencode")
        }));
    }

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn session_card_restores_compact_tree_name_and_right_aligned_age() {
        let (mut snapshot, active, _) = snapshot();
        let now = 10_000_000_i64;
        snapshot.workstreams[0].native_name = Some("native thread".to_owned());
        snapshot.workstreams[0].last_activity_at_millis = Some(now - 3 * 60 * 1_000);
        let row = Row::Workstream(snapshot.workstreams[0].clone());

        let lines = super::row_lines_at(&row, true, 30, Some(now));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].width(), 30);
        assert!(lines[1].width() <= 30);
        assert!(line_text(&lines[0]).starts_with("└ Codex"));
        assert!(line_text(&lines[0]).ends_with("3 min ago"));
        assert_eq!(line_text(&lines[1]), "    native thread");
        assert!(!line_text(&lines[0]).contains("starting"));
        assert!(!line_text(&lines[0]).contains("attention"));
        assert!(!line_text(&lines[1]).contains('·'));
        assert_eq!(active, snapshot.workstreams[0].workstream_id);
    }

    #[test]
    fn session_card_age_labels_have_deterministic_boundaries_and_unknown_state() {
        let now = 10_000_000_i64;
        let cases = [
            (0_i64, "now"),
            (59, "now"),
            (60, "1 min ago"),
            (3_599, "59 min ago"),
            (3_600, "1 hr ago"),
            (86_399, "23 hr ago"),
            (86_400, "1 day ago"),
            (172_799, "1 day ago"),
            (172_800, "2 days ago"),
            (7 * 86_400, "7 days ago"),
        ];
        for (elapsed, expected) in cases {
            assert_eq!(
                super::relative_activity_age(Some(now - elapsed * 1_000), Some(now)),
                Some(expected.to_owned()),
                "elapsed seconds: {elapsed}"
            );
        }
        assert_eq!(super::relative_activity_age(None, Some(now)), None);
        assert_eq!(super::relative_activity_age(Some(now), None), None);
        assert_eq!(super::activity_label(None, Some(now)), "activity unknown");
        assert_eq!(super::activity_label(Some(now), None), "activity unknown");
        assert_eq!(super::activity_label(Some(now + 1_000), Some(now)), "now");
    }

    #[test]
    fn session_card_marker_precedence_is_compact_and_semantic() {
        let (snapshot, _, _) = snapshot();
        let mut workstream = snapshot.workstreams[0].clone();
        let runtime_id = RuntimeId::from(Uuid::from_u128(5));
        let marker = |workstream: &WorkstreamSnapshot| {
            let (value, style) = workstream_marker(workstream);
            (value, style.fg)
        };

        workstream.lifecycle = WorkstreamLifecycle::Parked;
        workstream.onboarding = Some(OnboardingStatus::RecoveryRequired);
        assert_eq!(marker(&workstream), ("!", Some(Color::Red)));

        workstream.lifecycle = WorkstreamLifecycle::Open;
        assert_eq!(marker(&workstream), ("!", Some(Color::Red)));

        workstream.onboarding = None;
        workstream.runtime = None;
        workstream.lifecycle = WorkstreamLifecycle::Parked;
        assert_eq!(marker(&workstream), (" ", None));
        workstream.lifecycle = WorkstreamLifecycle::Open;

        workstream.runtime = Some(RuntimeSnapshot {
            runtime_id,
            status: RuntimeStatus::Attention,
            revision: Revision::INITIAL,
        });
        assert_eq!(marker(&workstream), ("✓", Some(Color::Green)));

        workstream.lifecycle = WorkstreamLifecycle::Parked;
        assert_eq!(marker(&workstream), (" ", None));
        workstream.lifecycle = WorkstreamLifecycle::Open;

        workstream.onboarding = Some(OnboardingStatus::ActionFenced);
        assert_eq!(marker(&workstream), ("…", Some(Color::Cyan)));

        workstream.onboarding = None;
        workstream.runtime.as_mut().unwrap().status = RuntimeStatus::Starting;
        assert_eq!(marker(&workstream), ("…", Some(Color::Cyan)));

        workstream.runtime.as_mut().unwrap().status = RuntimeStatus::Working;
        assert_eq!(marker(&workstream), ("●", Some(Color::Yellow)));

        workstream.runtime.as_mut().unwrap().status = RuntimeStatus::Unknown;
        assert_eq!(marker(&workstream), ("?", Some(Color::Red)));

        for status in [RuntimeStatus::Idle, RuntimeStatus::Stopped] {
            workstream.runtime.as_mut().unwrap().status = status;
            assert_eq!(marker(&workstream), (" ", None));
        }
        workstream.runtime = None;
        assert_eq!(marker(&workstream), (" ", None));
    }

    #[test]
    fn session_card_tree_continuation_and_narrow_widths_remain_bounded() {
        let (mut snapshot, _, _) = snapshot();
        snapshot.workstreams[0].native_name = Some("a very long native thread title".to_owned());
        snapshot.workstreams[0].last_activity_at_millis = Some(1_000);
        let row = Row::Workstream(snapshot.workstreams[0].clone());

        let branch = super::row_lines_at(&row, false, 30, Some(100_000));
        assert!(line_text(&branch[0]).starts_with("├ Codex"));
        assert!(line_text(&branch[1]).starts_with("│  "));
        assert!(line_text(&branch[1]).ends_with('…'));

        for width in 0..=32 {
            let lines = super::row_lines_at(&row, width % 2 == 0, width, Some(100_000));
            assert!(
                lines.iter().all(|line| line.width() <= usize::from(width)),
                "width {width}: {lines:?}"
            );
        }
    }

    #[test]
    fn archived_cards_use_the_same_compact_shape_and_age_projection() {
        let (snapshot, _, archived) = snapshot();
        let mut model = Model::new(snapshot);
        let _ = model.handle_key(crossterm::event::KeyCode::Char('.'));
        let row = model
            .rows()
            .into_iter()
            .find(|row| row.id() == Some(RowId::Workstream(archived)))
            .expect("archived workstream row");
        let lines = super::row_lines_at(&row, true, 30, Some(10_000_000));
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).starts_with("└ OpenCode"));
        assert!(line_text(&lines[0]).ends_with("activity unknown"));
        assert!(line_text(&lines[1]).starts_with("   "));
        assert!(line_text(&lines[1]).ends_with(archived.short().as_str()));
        assert!(!line_text(&lines[1]).contains("Workstream"));
    }

    #[test]
    fn rendered_selection_changes_background_without_overwriting_card_semantics() {
        let (mut snapshot, active, _) = snapshot();
        snapshot.workstreams[0].native_name = Some("selected thread".to_owned());
        snapshot.workstreams[0].last_activity_at_millis = Some(9_999_000);
        let mut navigator = Navigator::new(snapshot);
        navigator.model_mut().select_next();
        let backend = TestBackend::new(32, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| navigator.render_at(frame, frame.area(), Some(10_000_000)))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let geometry = navigator.list_geometry(Rect::new(0, 0, 32, 24));
        let first_card_line = geometry.inner.y.saturating_add(3);
        assert_eq!(
            buffer[(geometry.inner.x, first_card_line)].bg,
            Color::DarkGray
        );
        assert_eq!(buffer[(geometry.inner.x, first_card_line)].symbol(), "└");
        assert_eq!(
            buffer[(
                geometry.inner.x.saturating_add(2),
                first_card_line.saturating_add(1)
            )]
                .symbol(),
            " ",
        );
        assert_eq!(navigator.model.selected(), Some(RowId::Workstream(active)));
    }

    #[test]
    fn every_card_line_is_an_exact_mouse_target_but_headers_are_not() {
        let (snapshot, active, _) = snapshot();
        let plain = Navigator::new(snapshot.clone());
        let area = Rect::new(0, 0, 32, 24);
        let plain_geometry = plain.list_geometry(area);
        let plain_x = plain_geometry.inner.x;
        assert_eq!(
            plain.row_at(area, plain_x, plain_geometry.inner.y),
            Some(RowId::ProvisionalShell)
        );
        assert_eq!(
            plain.row_at(area, plain_x, plain_geometry.inner.y.saturating_add(1)),
            Some(RowId::ProvisionalShell)
        );
        assert_eq!(
            plain.row_at(area, plain_x, plain_geometry.inner.y.saturating_add(2)),
            None
        );

        let mut navigator = Navigator::new(snapshot);
        navigator.set_shell_location(ShellLocation::cwd("~/c/wsnav"));
        let geometry = navigator.list_geometry(area);
        let x = geometry.inner.x;

        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y),
            Some(RowId::ProvisionalShell)
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(1)),
            Some(RowId::ProvisionalShell)
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(3)),
            Some(RowId::Workstream(active))
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(4)),
            Some(RowId::Workstream(active))
        );
        assert_eq!(
            navigator.row_at(area, x, geometry.inner.y.saturating_add(5)),
            None
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
        snapshot.workstreams[0].runtime = Some(RuntimeSnapshot {
            runtime_id,
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        });
        let mut model = Model::new(snapshot);

        assert_eq!(
            model.activate_row(RowId::Workstream(active)),
            Command::Attach {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
                runtime_id,
                expected_runtime_revision: Revision::INITIAL,
            }
        );
        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
    }

    #[test]
    fn promotion_transfers_selection_from_shell_to_its_managed_runtime_card() {
        let (snapshot, active, _) = snapshot();
        let mut model = Model::new(snapshot.clone());
        let runtime_id = RuntimeId::from(Uuid::from_u128(5));
        let mut promoted = snapshot;
        promoted.workstreams[0].runtime = Some(RuntimeSnapshot {
            runtime_id,
            status: RuntimeStatus::Starting,
            revision: Revision::INITIAL,
        });

        model.replace_snapshot(promoted);
        assert_eq!(model.selected(), Some(RowId::ProvisionalShell));
        assert!(model.select_runtime(runtime_id));
        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
    }

    #[test]
    fn narrow_footer_keeps_every_complete_binding_visible() {
        let (mut snapshot, _, _) = snapshot();
        snapshot.workstreams[0].runtime = Some(RuntimeSnapshot {
            runtime_id: RuntimeId::from(Uuid::from_u128(5)),
            status: RuntimeStatus::Idle,
            revision: Revision::INITIAL,
        });
        let mut model = Model::new(snapshot);
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
        for expected in ["n new", "x archive", ". archived", "q quit"] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}: {rendered}"
            );
        }
        assert!(!rendered.contains("↑↓ select"));
        assert!(!rendered.contains("Enter open"));
        assert!(!rendered.contains("p park"));
        assert!(lines.iter().all(|line| line.width() <= 32));
        assert_eq!(
            super::controls_height(&model, 32),
            u16::try_from(lines.len()).unwrap()
        );
    }

    #[test]
    fn compact_footer_omits_full_help_only_bindings() {
        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);

        let initial_bindings = super::control_bindings(&model);
        model.select_next();
        let workstreams_bindings = super::control_bindings(&model);
        let _ = model.handle_key(crossterm::event::KeyCode::Char('.'));
        let archived_bindings = super::control_bindings(&model);

        for bindings in [initial_bindings, workstreams_bindings, archived_bindings] {
            assert!(!bindings.iter().any(|(key, _)| *key == "↑↓"));
            assert!(
                !bindings
                    .iter()
                    .any(|(key, action)| *key == "Enter" && matches!(*action, "open" | "shell"))
            );
        }
        assert!(workstreams_bindings.iter().any(|(key, _)| *key == "x"));
        assert!(!workstreams_bindings.iter().any(|(key, _)| *key == "u"));
        assert!(!workstreams_bindings.iter().any(|(key, _)| *key == "p"));
        assert!(archived_bindings.iter().any(|(key, _)| *key == "u"));
        assert!(!archived_bindings.iter().any(|(key, _)| *key == "x"));
        assert!(!archived_bindings.iter().any(|(key, _)| *key == "p"));
    }

    #[test]
    fn help_is_full_width_colored_and_fits_its_content() {
        let overlay = super::help_overlay(Rect::new(0, 0, 32, 24), Page::Workstreams);
        assert_eq!(overlay, Rect::new(0, 7, 32, 10));
        assert_eq!(
            super::help_overlay(Rect::new(0, 0, 32, 18), Page::Workstreams),
            Rect::new(0, 4, 32, 10)
        );
        assert_eq!(
            super::help_overlay(Rect::new(0, 0, 32, 24), Page::Archived),
            Rect::new(0, 8, 32, 7)
        );

        let lines = super::help_lines(Page::Workstreams);
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().all(|line| line.width() <= 30));
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered[1], "↑↓      select");
        assert_eq!(rendered[2], "Enter   open");
        assert_eq!(rendered[4], "Esc     back");
        assert_eq!(rendered[6], "n       new at location");
        assert_eq!(rendered[7], "x       archive session");
        assert!(!rendered.iter().any(|line| line.contains("close help")));
        assert!(!rendered.iter().any(|line| line.contains("park")));
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(lines[6].spans[0].style.fg, Some(Color::Yellow));

        let archived_lines = super::help_lines(Page::Archived);
        let archived_text = archived_lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(archived_text.iter().any(|line| line == "Esc     back"));
        assert!(
            archived_text
                .iter()
                .any(|line| line.contains("u       restore session"))
        );
        assert!(
            !archived_text
                .iter()
                .any(|line| line.contains("archive session"))
        );
        assert!(
            !archived_text
                .iter()
                .any(|line| line.contains("new at location"))
        );

        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('?')),
            Command::None
        );
        assert!(model.help_visible());
        assert!(super::control_bindings(&model).is_empty());
        assert!(super::controls_lines(&model, 32).is_empty());
        assert_eq!(super::footer_height(Rect::new(0, 0, 32, 24), &model), 0);
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('?')),
            Command::None
        );
        assert!(model.help_visible());
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('q')),
            Command::None
        );
        assert!(!model.help_visible());
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('?')),
            Command::None
        );
        assert!(model.help_visible());
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Esc),
            Command::None
        );
        assert!(!model.help_visible());

        let backend = TestBackend::new(32, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                super::render_help(frame, Rect::new(0, 0, 32, 24), Page::Workstreams);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 7)].symbol(), "┌");
        assert_eq!(buffer[(31, 7)].symbol(), "┐");
        assert_eq!(buffer[(0, 16)].symbol(), "└");
        assert_eq!(buffer[(31, 16)].symbol(), "┘");
    }

    #[test]
    fn new_inherits_the_selected_workstreams_provider_and_location_context() {
        let (snapshot, active, _) = snapshot();
        let mut model = Model::new(snapshot);
        model.select_next();
        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            Command::NewAtSameLocation {
                source_workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
                provider: ProviderKind::Codex,
            }
        );
    }

    #[test]
    fn onboarding_runtime_stays_visible_but_refuses_attach_and_new() {
        let (mut snapshot, active, _) = snapshot();
        snapshot.workstreams[0].runtime = Some(RuntimeSnapshot {
            runtime_id: RuntimeId::from(Uuid::from_u128(5)),
            status: RuntimeStatus::Starting,
            revision: Revision::INITIAL,
        });
        snapshot.workstreams[0].onboarding = Some(OnboardingStatus::ActionFenced);
        let mut model = Model::new(snapshot);
        model.select_next();

        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            Command::ShowGuidance(super::ONBOARDING_IN_PROGRESS_GUIDANCE)
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            Command::None
        );
        let rows = model.rows();
        let workstream = rows
            .iter()
            .find(|row| row.id() == Some(RowId::Workstream(active)))
            .expect("active onboarding row");
        let lines = super::row_lines_at(workstream, true, 30, None);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("…"));
        assert!(!rendered.contains("onboarding"));
    }

    #[test]
    fn observer_setup_guide_requires_explicit_consent_and_decline_is_side_effect_free() {
        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);
        model.request_observer_setup(ObserverSetupKind::Create);
        assert_eq!(
            model.modal(),
            Some(&Modal::ObserverConsent {
                kind: ObserverSetupKind::Create,
            })
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            Command::CancelObserverSetup
        );
        assert!(model.modal().is_none());

        model.request_observer_setup(ObserverSetupKind::Update);
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            Command::AcceptObserverSetup {
                kind: ObserverSetupKind::Update,
            }
        );
        assert!(model.modal().is_none());
    }

    #[test]
    fn observer_setup_controls_are_bounded_and_native_review_is_named() {
        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);
        model.request_observer_setup(ObserverSetupKind::TrustReview);
        let bindings = super::control_bindings(&model);
        assert_eq!(
            bindings,
            &[("Enter / y", "continue"), ("n / Esc", "cancel")]
        );
        let text = super::observer_setup_modal_text(ObserverSetupKind::TrustReview);
        assert!(text.contains("native /hooks"));
        assert!(!text.contains("CODEX_HOME"));
        assert!(!text.contains("--profile"));
    }

    #[test]
    fn guidance_clear_only_removes_the_exact_current_message() {
        let (snapshot, _, _) = snapshot();
        let mut model = Model::new(snapshot);
        model.set_guidance("reconciliation unavailable");
        model.clear_guidance_if("reconciliation unavailable");
        assert_eq!(model.guidance(), None);

        model.set_guidance("another command failed");
        model.clear_guidance_if("reconciliation unavailable");
        assert_eq!(model.guidance(), Some("another command failed"));
    }

    #[test]
    fn archived_page_has_no_shell_card_and_cannot_start_a_new_session() {
        let (snapshot, _, archived) = snapshot();
        let mut model = Model::new(snapshot);
        let _ = model.handle_key(crossterm::event::KeyCode::Char('.'));
        assert_eq!(model.selected(), Some(RowId::Workstream(archived)));
        assert!(
            model
                .rows()
                .iter()
                .all(|row| !matches!(row, Row::ProvisionalShell { .. }))
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Esc),
            Command::None
        );
        assert_eq!(model.page(), Page::Workstreams);
        assert_eq!(model.selected(), Some(RowId::ProvisionalShell));
    }

    #[test]
    fn primary_action_uses_durable_lifecycle_not_runtime_guesswork() {
        {
            let (mut snapshot, active, _) = snapshot();
            snapshot.workstreams[0].lifecycle = WorkstreamLifecycle::Parked;
            let mut model = Model::new(snapshot);
            model.select_next();

            assert_eq!(
                model.handle_key(crossterm::event::KeyCode::Enter),
                Command::Start {
                    workstream_id: active,
                    expected_workstream_revision: Revision::INITIAL,
                    provider: ProviderKind::Codex,
                }
            );
        }

        let (mut snapshot, active, _) = snapshot();
        snapshot.workstreams[0].lifecycle = WorkstreamLifecycle::RecoveryRequired;
        let mut model = Model::new(snapshot);
        model.select_next();
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            Command::Recover {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
                provider: ProviderKind::Codex,
            }
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('n')),
            Command::None
        );
    }

    #[test]
    fn lifecycle_keys_emit_exact_reversible_action_revisions() {
        let (snapshot, active, archived) = snapshot();
        let mut model = Model::new(snapshot);
        model.select_next();

        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('u')),
            Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('p')),
            Command::None
        );
        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('x')),
            Command::None
        );
        assert_eq!(
            model.modal(),
            Some(&Modal::ConfirmArchive {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
            })
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Enter),
            Command::Archive {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
            }
        );

        let _ = model.handle_key(crossterm::event::KeyCode::Char('.'));
        assert_eq!(model.selected(), Some(RowId::Workstream(archived)));
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('x')),
            Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('u')),
            Command::Restore {
                workstream_id: archived,
                expected_workstream_revision: Revision::INITIAL,
            }
        );
    }

    #[test]
    fn onboarding_recovery_can_only_archive_after_exact_cleanup() {
        let (mut snapshot, active, _) = snapshot();
        snapshot.workstreams[0].onboarding = Some(OnboardingStatus::RecoveryRequired);
        let mut model = Model::new(snapshot);
        model.select_next();
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('p')),
            Command::None
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('x')),
            Command::None
        );
        assert_eq!(
            model.modal(),
            Some(&Modal::ConfirmArchive {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
            })
        );
    }
}
