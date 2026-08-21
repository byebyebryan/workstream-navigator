//! Pure D16 navigator model and controller.
//!
//! The D16 surface is deliberately a small, host-local presentation boundary.
//! It consumes the bounded [`crate::application::ApplicationSnapshot`] and
//! emits typed [`crate::application::ApplicationAction`] values for a caller
//! to execute.  It does not open state, inspect Git, talk to tmux, or attach a
//! provider while building rows or redrawing.

#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    reason = "D16's public names make the page/model/controller boundary explicit."
)]

use std::cmp::min;
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
    application::{
        ApplicationAction, ApplicationError, ApplicationOutcome, ApplicationSnapshot,
        AttachEvidence, AttentionKind, BrowserListing, BrowserPath, BrowserRootPath,
        ObserverReadinessGuide, OperationSnapshot, ProjectRefreshRequest, ProjectSnapshot,
        RevisedIdentity, WorkstreamSnapshot,
    },
    domain::{
        Clock, LocationId, OperationId, ProjectId, ProviderKind, Revision, RuntimeStatus,
        SystemClock, WorkstreamId, WorkstreamLifecycle,
    },
    presentation::{AttachmentPhase, AttachmentStatus},
};

const MAIN_VIEWPORT_ROWS: usize = 10;
const BROWSER_VIEWPORT_ROWS: usize = 10;

/// Keep selection distinct from semantic row foregrounds. A selected row only
/// receives this background; provider, Project, lifecycle, and age colors
/// remain unchanged.
const SELECTED_ROW_BACKGROUND: Color = Color::Indexed(236);
const PARKED_INDICATOR_COLOR: Color = Color::Indexed(110);

/// Activity age is a neutral brightness ramp. It does not compete with the
/// provider, Project, or lifecycle color axes.
const AGE_UNKNOWN_COLOR: Color = Color::Indexed(244);
const AGE_RECENT_COLOR: Color = Color::Indexed(255);
const AGE_HOURLY_COLOR: Color = Color::Indexed(251);
const AGE_DAILY_COLOR: Color = Color::Indexed(247);
const AGE_WEEKLY_COLOR: Color = Color::Indexed(244);
const AGE_STALE_COLOR: Color = Color::Indexed(241);

const PROJECT_TREE_COLOR: Color = Color::Indexed(245);
const PROVIDER_LABEL_PALETTE: [Color; 2] = [Color::Indexed(209), Color::Indexed(80)];
const PROJECT_MARKER_PALETTE: [Color; 12] = [
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

/// The only three D16 pages.  Selection is process-local and is never part of
/// the application snapshot or durable state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum D16Page {
    #[default]
    Workstreams,
    Projects,
    Archived,
}

impl D16Page {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Workstreams => "Workstreams",
            Self::Projects => "Projects",
            Self::Archived => "Archived",
        }
    }

    #[must_use]
    pub const fn key(self) -> Option<char> {
        match self {
            Self::Workstreams => None,
            Self::Projects => Some(','),
            Self::Archived => Some('.'),
        }
    }
}

/// Stable, opaque identity used to retain an in-memory cursor across a
/// passive snapshot replacement.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum D16RowId {
    Project(ProjectId),
    Location(LocationId),
    Workstream(WorkstreamId),
    Operation(OperationId),
}

/// A display-only Project header.  It is intentionally a row, rather than an
/// action target: Project actions are exposed through explicit controller
/// methods and never through a header selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D16ProjectHeader {
    pub project_id: ProjectId,
    pub display_name: String,
}

/// A host-local Location row.  Its Project and Location revisions are carried
/// so `n` can produce an exact dormant-location creation action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D16LocationRow {
    pub project_id: ProjectId,
    pub location_id: LocationId,
    pub display_name: String,
    pub revision: Revision,
    /// True when this Location is one of several children beneath a visible
    /// Project header. A single Location is flattened so its label source is
    /// not rendered as a duplicate Project name.
    pub grouped_under_project: bool,
}

/// A Workstream row carrying only the bounded snapshot rendered by its card.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D16WorkstreamRow {
    pub workstream: WorkstreamSnapshot,
}

/// A bounded unresolved operation displayed in the Workstreams page's
/// recovery section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct D16OperationRow {
    pub operation: OperationSnapshot,
}

/// One row in the reduced navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum D16Row {
    ProjectHeader(D16ProjectHeader),
    Location(D16LocationRow),
    Workstream(D16WorkstreamRow),
    Operation(D16OperationRow),
}

impl D16Row {
    #[must_use]
    pub const fn id(&self) -> Option<D16RowId> {
        match self {
            // A header has no cursor identity.  Its Project ID remains in the
            // display DTO, but can never become a selection or action target.
            Self::ProjectHeader(_) => None,
            Self::Location(row) => Some(D16RowId::Location(row.location_id)),
            Self::Workstream(row) => Some(D16RowId::Workstream(row.workstream.workstream_id)),
            Self::Operation(row) => Some(D16RowId::Operation(row.operation.operation_id)),
        }
    }

    /// Project headers are visible context only; no direct operation can use
    /// a Project header as a target.
    #[must_use]
    pub const fn is_actionable(&self) -> bool {
        !matches!(self, Self::ProjectHeader(_))
    }
}

/// A browser's process-local state.  The actual listing is returned by the
/// host-local application action; no redraw performs filesystem work.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct D16ProjectBrowser {
    pub root_label: String,
    pub path: BrowserPath,
    pub include_hidden: bool,
    pub filter: String,
    pub listing: Option<BrowserListing>,
    pub selected: usize,
    pub scroll: usize,
}

/// A command emitted by the pure D16 controller.  The caller owns execution
/// of every command; in particular, `Attach` is deliberately not performed by
/// this model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum D16Command {
    None,
    Quit,
    Apply(ApplicationAction),
    Attach(AttachEvidence),
    AcceptObserverGuide(ObserverReadinessGuide),
}

/// The rendered list bounds used by both drawing and mouse hit testing.
/// Keeping the outer and inner rectangles together prevents mouse coordinates
/// from being interpreted against the terminal origin or a stale row offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct D16ListGeometry {
    pub outer: Rect,
    pub inner: Rect,
    pub viewport_rows: usize,
}

/// One action-specific modal.  These are process-local interaction states,
/// never capability or mutation authority by themselves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum D16Modal {
    ConfirmArchive {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    },
    Rename {
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        value: String,
    },
    SetBrowserRoot {
        expected_revision: Revision,
        value: String,
    },
}

/// The process-local intent waiting for a provider selection.  It contains
/// only the exact opaque IDs and revisions required to construct one typed
/// application action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum D16ProviderRequest {
    NewAtLocation {
        project_id: ProjectId,
        location_id: LocationId,
        expected_project_revision: Revision,
        expected_location_revision: Revision,
    },
    NewAtSameLocation {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    RegisterLocation {
        relative_path: BrowserPath,
        expected_browser_revision: Revision,
    },
}

impl D16ProviderRequest {
    fn action(&self, provider: ProviderKind) -> ApplicationAction {
        match self {
            Self::NewAtLocation {
                project_id,
                location_id,
                expected_project_revision,
                expected_location_revision,
            } => ApplicationAction::NewAtLocation {
                project_id: *project_id,
                location_id: *location_id,
                expected_project_revision: *expected_project_revision,
                expected_location_revision: *expected_location_revision,
                provider,
            },
            Self::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
            } => ApplicationAction::NewAtSameLocation {
                source_workstream_id: *source_workstream_id,
                expected_workstream_revision: *expected_workstream_revision,
                provider,
            },
            Self::RegisterLocation {
                relative_path,
                expected_browser_revision,
            } => ApplicationAction::RegisterLocation {
                relative_path: relative_path.clone(),
                expected_browser_revision: *expected_browser_revision,
                provider,
            },
        }
    }
}

/// A bounded process-local provider chooser.  Provider capability evidence is
/// copied only as the small typed provider list; no provider payload or host
/// selection state is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D16ProviderChooser {
    pub providers: Vec<ProviderKind>,
    pub selected: usize,
    pub request: D16ProviderRequest,
}

impl D16Command {
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Read-only D16 presentation model.  The snapshot is replaced by the caller
/// after an explicit application refresh; selection, page, browser, guide,
/// and message are all process-local state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D16Model {
    snapshot: ApplicationSnapshot,
    page: D16Page,
    selected: Option<D16RowId>,
    observer_guide: Option<ObserverReadinessGuide>,
    browser: Option<D16ProjectBrowser>,
    provider_chooser: Option<D16ProviderChooser>,
    modal: Option<D16Modal>,
    pending_action: Option<ApplicationAction>,
    observed_attachment: Option<(uuid::Uuid, AttachmentPhase, WorkstreamId)>,
    message: Option<String>,
    scroll: usize,
    help_visible: bool,
    help_scroll: usize,
}

impl D16Model {
    #[must_use]
    pub fn new(snapshot: ApplicationSnapshot) -> Self {
        let mut model = Self {
            snapshot,
            page: D16Page::Workstreams,
            selected: None,
            observer_guide: None,
            browser: None,
            provider_chooser: None,
            modal: None,
            pending_action: None,
            observed_attachment: None,
            message: None,
            scroll: 0,
            help_visible: false,
            help_scroll: 0,
        };
        model.selected = model.row_ids().first().copied();
        model
    }

    #[must_use]
    pub const fn snapshot(&self) -> &ApplicationSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub const fn page(&self) -> D16Page {
        self.page
    }

    #[must_use]
    pub const fn selected_id(&self) -> Option<D16RowId> {
        self.selected
    }

    #[must_use]
    pub const fn observer_guide(&self) -> Option<ObserverReadinessGuide> {
        self.observer_guide
    }

    #[must_use]
    pub const fn browser(&self) -> Option<&D16ProjectBrowser> {
        self.browser.as_ref()
    }

    #[must_use]
    pub const fn provider_chooser(&self) -> Option<&D16ProviderChooser> {
        self.provider_chooser.as_ref()
    }

    #[must_use]
    pub const fn modal(&self) -> Option<&D16Modal> {
        self.modal.as_ref()
    }

    #[must_use]
    pub const fn scroll(&self) -> usize {
        self.scroll
    }

    #[must_use]
    pub const fn help_visible(&self) -> bool {
        self.help_visible
    }

    #[must_use]
    pub const fn message(&self) -> Option<&String> {
        self.message.as_ref()
    }

    /// Returns the pending action retained while contextual observer guidance
    /// is awaiting explicit consent and native review.
    #[must_use]
    pub fn pending_action(&self) -> Option<&ApplicationAction> {
        self.pending_action.as_ref()
    }

    /// Replaces the bounded status text shown by the local navigator.
    pub fn set_message(&mut self, message: impl Into<String>) {
        let mut message = message.into();
        if message.chars().count() > 256 {
            message = message.chars().take(256).collect();
        }
        self.message = Some(message);
    }

    /// Records one exact presentation attachment status in process-local UI
    /// state. A status from another attempt cannot clear or overwrite the
    /// current row marker.
    pub fn observe_attachment(&mut self, status: &AttachmentStatus) -> bool {
        let observation = (status.attempt_id, status.phase, status.workstream_id);
        let changed = self.observed_attachment != Some(observation);
        self.observed_attachment = Some(observation);
        if changed {
            match status.phase {
                AttachmentPhase::Pending => self.set_message("provider attachment starting"),
                AttachmentPhase::Running => {
                    self.set_message("provider attached; use the native provider UI directly");
                }
                AttachmentPhase::Completed => self
                    .set_message("provider detached; press Enter or click this row to reconnect"),
                AttachmentPhase::Failed => {
                    self.set_message("attachment failed; press Enter or click row to retry");
                }
            }
        }
        changed
    }

    /// Clears a completed/failed attachment marker only when it belongs to the
    /// exact attempt being observed.
    pub fn clear_attachment(&mut self, attempt_id: uuid::Uuid) {
        if self
            .observed_attachment
            .is_some_and(|(current, _, _)| current == attempt_id)
        {
            self.observed_attachment = None;
        }
    }

    /// Returns current page rows in the order supplied by the application
    /// projection.  No sorting or external observation occurs here.
    #[must_use]
    pub fn rows(&self) -> Vec<D16Row> {
        match self.page {
            D16Page::Workstreams => self.workstream_rows(false),
            D16Page::Archived => self.workstream_rows(true),
            D16Page::Projects => self.project_rows(),
        }
    }

    /// Returns the row currently selected on this page, if it is visible.
    #[must_use]
    pub fn selected_row(&self) -> Option<D16Row> {
        let selected = self.selected?;
        self.rows()
            .into_iter()
            .find(|row| row.id() == Some(selected))
    }

    /// Replaces the passive projection while retaining a visible opaque
    /// selection where possible.  The page itself never changes implicitly.
    pub fn replace_snapshot(&mut self, snapshot: ApplicationSnapshot) {
        self.snapshot = snapshot;
        let ids = self.row_ids();
        if self
            .selected
            .is_none_or(|selected| !ids.contains(&selected))
        {
            self.selected = ids.first().copied();
        }
        self.scroll = self.scroll.min(self.rows().len().saturating_sub(1));
        self.ensure_main_selection_visible(MAIN_VIEWPORT_ROWS);
    }

    /// Selects a process-local page.  The current row is retained when that
    /// identity exists on the destination page; otherwise the first row is
    /// selected.
    pub fn select_page(&mut self, page: D16Page) {
        self.page = page;
        let ids = self.row_ids();
        if self.selected.is_none_or(|id| !ids.contains(&id)) {
            self.selected = ids.first().copied();
        }
        self.scroll = 0;
        self.ensure_main_selection_visible(MAIN_VIEWPORT_ROWS);
    }

    pub fn select_next(&mut self) {
        self.move_selection(1);
        self.ensure_main_selection_visible(MAIN_VIEWPORT_ROWS);
    }

    pub fn select_previous(&mut self) {
        self.move_selection(usize::MAX);
        self.ensure_main_selection_visible(MAIN_VIEWPORT_ROWS);
    }

    /// Selects one exact visible actionable row and performs the primary
    /// Workstream activation used by a left-button release.  Pages, headers,
    /// and every modal remain non-authority.
    pub fn activate_row(&mut self, row_id: D16RowId) -> D16Command {
        if self.help_visible
            || self.browser.is_some()
            || self.provider_chooser.is_some()
            || self.observer_guide.is_some()
            || self.modal.is_some()
            || !self.row_ids().contains(&row_id)
        {
            return D16Command::None;
        }
        self.selected = Some(row_id);
        self.ensure_main_selection_visible(MAIN_VIEWPORT_ROWS);
        if self.page == D16Page::Workstreams && matches!(row_id, D16RowId::Workstream(_)) {
            self.attach_selected()
        } else {
            D16Command::None
        }
    }

    /// Resolves one rendered list line to its exact actionable identity.
    /// Project headers never resolve, and both lines of a Workstream card map
    /// to the same opaque Workstream ID.
    #[must_use]
    pub fn row_id_at_render_line(&self, viewport_rows: usize, line: usize) -> Option<D16RowId> {
        let (_, rows) = self.visible_rows(viewport_rows);
        let mut cursor = 0_usize;
        for row in rows {
            let height = usize::from(matches!(row, D16Row::Workstream(_))) + 1;
            if (cursor..cursor.saturating_add(height)).contains(&line) {
                return row.id();
            }
            cursor = cursor.saturating_add(height);
        }
        None
    }

    /// Returns a bounded page window and its row offset. The selected row is
    /// kept inside the window even when a passive snapshot changes row count.
    #[must_use]
    pub fn visible_rows(&self, viewport: usize) -> (usize, Vec<D16Row>) {
        let rows = self.rows();
        if rows.is_empty() {
            return (0, rows);
        }
        let viewport = viewport.max(1);
        let max_start = rows.len().saturating_sub(viewport);
        let mut start = self.scroll.min(max_start);
        if let Some(selected) = self
            .selected
            .and_then(|id| rows.iter().position(|row| row.id() == Some(id)))
        {
            if selected < start {
                start = selected;
            } else if selected >= start.saturating_add(viewport) {
                start = selected
                    .saturating_add(1)
                    .saturating_sub(viewport)
                    .min(max_start);
            }
        }
        let visible = rows
            .into_iter()
            .skip(start)
            .take(viewport)
            .collect::<Vec<_>>();
        (start, visible)
    }

    /// Opens the project browser and emits its first passive listing request.
    pub fn open_project_browser(&mut self) -> D16Command {
        self.select_page(D16Page::Projects);
        self.browser = Some(D16ProjectBrowser {
            root_label: self.snapshot.project_browser.root_label.clone(),
            ..D16ProjectBrowser::default()
        });
        self.browser_list_command()
    }

    /// Closes only the process-local browser modal.
    pub fn close_project_browser(&mut self) {
        self.browser = None;
    }

    /// Updates the browser with a result returned by the application facade.
    /// This method only changes in-memory presentation state.
    pub fn accept_browser_listing(&mut self, listing: BrowserListing) {
        let Some(browser) = self.browser.as_mut() else {
            self.message = Some("browser result without an open browser".to_owned());
            return;
        };
        browser.path = listing.relative_path.clone();
        browser.root_label.clone_from(&listing.root_label);
        browser.include_hidden = listing.include_hidden;
        browser.selected = min(
            browser.selected,
            filtered_browser_entries_for(&listing, &browser.filter)
                .len()
                .saturating_sub(1),
        );
        browser.scroll = min(
            browser.scroll,
            listing.entries.len().saturating_sub(BROWSER_VIEWPORT_ROWS),
        );
        browser.listing = Some(listing);
        ensure_browser_selection_visible(browser);
    }

    /// Records an application outcome without executing any follow-up effect.
    /// Restore success returns to Workstreams and retains the restored row's
    /// opaque ID for the next passive snapshot, without launching or
    /// attaching a Runtime.
    pub fn accept_outcome(&mut self, outcome: ApplicationOutcome) {
        match outcome {
            ApplicationOutcome::ObserverReadinessRequired(guide) => {
                self.observer_guide = Some(guide);
            }
            ApplicationOutcome::BrowserListed(listing) => self.accept_browser_listing(listing),
            ApplicationOutcome::Applied { identity } => {
                self.accept_revised_identity(identity);
                self.pending_action = None;
                self.observer_guide = None;
            }
            ApplicationOutcome::Created { workstream_id, .. } => {
                self.page = D16Page::Workstreams;
                self.selected = Some(D16RowId::Workstream(workstream_id));
                self.pending_action = None;
                self.observer_guide = None;
            }
            ApplicationOutcome::ProjectRefreshed { .. } => {
                self.pending_action = None;
                self.observer_guide = None;
            }
        }
    }

    /// Clears contextual guidance after the caller cancels it.  No observer
    /// preparation or native trust action is performed here.
    pub fn dismiss_observer_guide(&mut self) {
        self.observer_guide = None;
        self.pending_action = None;
    }

    /// Emits the exact contextual guide for explicit interactive acceptance.
    /// This does not prepare, trust, or mutate anything and never changes the
    /// current page.
    pub fn accept_observer_guide(&self) -> D16Command {
        self.observer_guide
            .map_or(D16Command::None, D16Command::AcceptObserverGuide)
    }

    /// Returns a revision-checked Project refresh action.  Project headers are
    /// display-only in the row controller, so callers provide the explicit ID.
    pub fn refresh_project(&mut self, project_id: ProjectId) -> Option<D16Command> {
        let project = self.project(project_id)?;
        Some(
            self.record_action(ApplicationAction::RefreshProject(ProjectRefreshRequest {
                project_id,
                expected_project_revision: project.revision,
            })),
        )
    }

    /// Returns an explicit host-local browser-root action.  The root is
    /// request-only and never appears in the snapshot or row model.
    pub fn set_browser_root(
        &mut self,
        root_path: BrowserRootPath,
        expected_revision: Revision,
    ) -> D16Command {
        self.record_action(ApplicationAction::SetProjectBrowserRoot {
            root_path,
            expected_revision,
        })
    }

    /// Pure keyboard controller for the reduced surface.
    #[allow(
        clippy::too_many_lines,
        reason = "the ordered modal and page dispatch keeps input precedence auditable"
    )]
    pub fn handle_key(&mut self, key: KeyCode) -> D16Command {
        if self.help_visible {
            match key {
                KeyCode::Char('?' | 'q') | KeyCode::Esc => {
                    self.help_visible = false;
                    self.help_scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                }
                _ => {}
            }
            return D16Command::None;
        }
        if self.provider_chooser.is_some() {
            return self.handle_provider_chooser_key(key);
        }
        if self.modal.is_some() {
            return self.handle_action_modal_key(key);
        }
        if self.browser.is_some() {
            return self.handle_browser_key(key);
        }
        if self.observer_guide.is_some() {
            return match key {
                KeyCode::Esc => {
                    self.dismiss_observer_guide();
                    D16Command::None
                }
                KeyCode::Enter => self.accept_observer_guide(),
                _ => D16Command::None,
            };
        }
        match key {
            KeyCode::Char('q') => D16Command::Quit,
            KeyCode::Char('?') => {
                self.help_visible = true;
                self.help_scroll = 0;
                D16Command::None
            }
            KeyCode::Char(',') => {
                if self.page == D16Page::Projects {
                    self.select_page(D16Page::Workstreams);
                } else {
                    self.select_page(D16Page::Projects);
                }
                D16Command::None
            }
            KeyCode::Char('.') => {
                if self.page == D16Page::Archived {
                    self.select_page(D16Page::Workstreams);
                } else {
                    self.select_page(D16Page::Archived);
                }
                D16Command::None
            }
            KeyCode::Esc => {
                if self.page != D16Page::Workstreams {
                    self.select_page(D16Page::Workstreams);
                }
                D16Command::None
            }
            KeyCode::Up => {
                self.select_previous();
                D16Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                D16Command::None
            }
            KeyCode::Char('a') if self.page == D16Page::Projects => self.open_project_browser(),
            KeyCode::Enter if self.page == D16Page::Workstreams => self.attach_selected(),
            KeyCode::Char('n') if self.page == D16Page::Workstreams => {
                self.new_at_selected_workstream()
            }
            KeyCode::Char('f') if self.page == D16Page::Workstreams => {
                self.fork_selected_workstream()
            }
            KeyCode::Char('p') if self.page == D16Page::Workstreams => {
                self.park_selected_workstream()
            }
            KeyCode::Char('x') if self.page == D16Page::Workstreams => {
                self.archive_selected_workstream()
            }
            KeyCode::Char('a') if self.page == D16Page::Workstreams => self.acknowledge_selected(),
            KeyCode::Char('u') if self.page == D16Page::Archived => self.restore_selected(),
            KeyCode::Char('n') if self.page == D16Page::Projects => self.new_at_selected_location(),
            KeyCode::Char('r') if self.page == D16Page::Projects => self.refresh_selected_project(),
            KeyCode::Char('r') if self.page == D16Page::Workstreams => {
                if matches!(self.selected_row(), Some(D16Row::Operation(_))) {
                    self.recover_selected_operation()
                } else {
                    self.begin_rename_selected()
                }
            }
            KeyCode::Char('b') if self.page == D16Page::Projects => {
                self.modal = Some(D16Modal::SetBrowserRoot {
                    expected_revision: self.snapshot.project_browser.revision,
                    value: String::new(),
                });
                D16Command::None
            }
            _ => D16Command::None,
        }
    }

    fn workstream_rows(&self, archived: bool) -> Vec<D16Row> {
        let groups = if archived {
            &self.snapshot.archived_project_groups
        } else {
            &self.snapshot.active_project_groups
        };
        let mut rows = Vec::new();
        for group in groups {
            let Some(project) = self.project(group.project_id) else {
                continue;
            };
            rows.push(D16Row::ProjectHeader(Self::project_header(project)));
            for workstream in &group.workstreams {
                rows.push(D16Row::Workstream(D16WorkstreamRow {
                    workstream: workstream.clone(),
                }));
            }
        }
        if !archived && !self.snapshot.unresolved_operations.is_empty() {
            for operation in &self.snapshot.unresolved_operations {
                rows.push(D16Row::Operation(D16OperationRow {
                    operation: *operation,
                }));
            }
        }
        rows
    }

    fn project_rows(&self) -> Vec<D16Row> {
        let mut rows = Vec::new();
        for project in &self.snapshot.projects {
            let grouped_under_project = project.locations.len() > 1;
            if grouped_under_project {
                rows.push(D16Row::ProjectHeader(Self::project_header(project)));
            }
            for location in &project.locations {
                rows.push(D16Row::Location(D16LocationRow {
                    project_id: project.project_id,
                    location_id: location.location_id,
                    display_name: location.display_name.clone(),
                    revision: location.revision,
                    grouped_under_project,
                }));
            }
        }
        rows
    }

    fn project_header(project: &ProjectSnapshot) -> D16ProjectHeader {
        D16ProjectHeader {
            project_id: project.project_id,
            display_name: project.display_name.clone(),
        }
    }

    fn project(&self, project_id: ProjectId) -> Option<&ProjectSnapshot> {
        self.snapshot
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
    }

    fn row_ids(&self) -> Vec<D16RowId> {
        self.rows()
            .into_iter()
            .filter_map(|row| row.is_actionable().then(|| row.id()).flatten())
            .collect()
    }

    fn ensure_main_selection_visible(&mut self, viewport: usize) {
        let rows = self.rows();
        let Some(selected) = self
            .selected
            .and_then(|id| rows.iter().position(|row| row.id() == Some(id)))
        else {
            self.scroll = self.scroll.min(rows.len().saturating_sub(viewport.max(1)));
            return;
        };
        let viewport = viewport.max(1);
        let max_start = rows.len().saturating_sub(viewport);
        self.scroll = self.scroll.min(max_start);
        if selected < self.scroll {
            self.scroll = selected;
        } else if selected >= self.scroll.saturating_add(viewport) {
            self.scroll = selected
                .saturating_add(1)
                .saturating_sub(viewport)
                .min(max_start);
        }
    }

    fn move_selection(&mut self, delta: usize) {
        let ids = self.row_ids();
        if ids.is_empty() {
            self.selected = None;
            return;
        }
        let Some(current) = self
            .selected
            .and_then(|selected| ids.iter().position(|id| *id == selected))
        else {
            self.selected = ids.first().copied();
            return;
        };
        let next = if delta == usize::MAX {
            if current == 0 {
                ids.len() - 1
            } else {
                current - 1
            }
        } else {
            (current + delta) % ids.len()
        };
        self.selected = ids.get(next).copied();
    }

    fn selected_location(&self) -> Option<D16LocationRow> {
        match self.selected_row()? {
            D16Row::Location(row) => Some(row),
            _ => None,
        }
    }

    fn selected_operation(&self) -> Option<OperationSnapshot> {
        match self.selected_row()? {
            D16Row::Operation(row) => Some(row.operation),
            _ => None,
        }
    }

    fn selected_workstream_owned(&self) -> Option<D16WorkstreamRow> {
        match self.selected_row()? {
            D16Row::Workstream(row) => Some(row),
            _ => None,
        }
    }

    fn attach_selected(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if row.workstream.lifecycle == crate::domain::WorkstreamLifecycle::RecoveryRequired {
            return self.record_action(ApplicationAction::Recover {
                workstream_id: row.workstream.workstream_id,
                expected_revision: row.workstream.revision,
                provider: row.workstream.provider,
            });
        }
        let Some(runtime) = row.workstream.runtime else {
            return self.record_action(ApplicationAction::Start {
                workstream_id: row.workstream.workstream_id,
                expected_revision: row.workstream.revision,
                provider: row.workstream.provider,
            });
        };
        if matches!(
            runtime.status,
            RuntimeStatus::Stopped | RuntimeStatus::Unknown
        ) {
            return self.record_action(ApplicationAction::Start {
                workstream_id: row.workstream.workstream_id,
                expected_revision: row.workstream.revision,
                provider: row.workstream.provider,
            });
        }
        D16Command::Attach(AttachEvidence {
            workstream_id: row.workstream.workstream_id,
            runtime_id: runtime.runtime_id,
            expected_workstream_revision: row.workstream.revision,
            expected_runtime_revision: runtime.revision,
        })
    }

    fn new_at_selected_workstream(&mut self) -> D16Command {
        if self.snapshot.active_workstreams().next().is_none() {
            self.select_page(D16Page::Projects);
            if self
                .snapshot
                .projects
                .iter()
                .any(|project| !project.locations.is_empty())
            {
                return D16Command::None;
            }
            return self.open_project_browser();
        }
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if row.workstream.archived {
            return D16Command::None;
        }
        self.request_provider(
            D16ProviderRequest::NewAtSameLocation {
                source_workstream_id: row.workstream.workstream_id,
                expected_workstream_revision: row.workstream.revision,
            },
            Some(row.workstream.provider),
        )
    }

    fn fork_selected_workstream(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if row.workstream.archived {
            return D16Command::None;
        }
        self.record_action(ApplicationAction::Fork {
            source_workstream_id: row.workstream.workstream_id,
            expected_workstream_revision: row.workstream.revision,
            provider: row.workstream.provider,
        })
    }

    fn park_selected_workstream(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if row.workstream.archived {
            return D16Command::None;
        }
        self.record_action(ApplicationAction::Park {
            workstream_id: row.workstream.workstream_id,
            expected_revision: row.workstream.revision,
        })
    }

    fn archive_selected_workstream(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if row.workstream.archived {
            return D16Command::None;
        }
        self.modal = Some(D16Modal::ConfirmArchive {
            workstream_id: row.workstream.workstream_id,
            expected_revision: row.workstream.revision,
        });
        D16Command::None
    }

    fn begin_rename_selected(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        let rename_available = self
            .snapshot
            .provider_capabilities
            .iter()
            .any(|capability| {
                capability.provider == row.workstream.provider
                    && capability.eligible_for_resume()
                    && capability.navigator_rename
            });
        if !rename_available {
            self.message =
                Some("the selected provider does not support navigator Rename".to_owned());
            return D16Command::None;
        }
        self.modal = Some(D16Modal::Rename {
            workstream_id: row.workstream.workstream_id,
            expected_revision: row.workstream.revision,
            value: row.workstream.native_name.unwrap_or_default(),
        });
        D16Command::None
    }

    fn handle_action_modal_key(&mut self, key: KeyCode) -> D16Command {
        match key {
            KeyCode::Esc => {
                self.modal = None;
                D16Command::None
            }
            KeyCode::Char('n') if matches!(self.modal, Some(D16Modal::ConfirmArchive { .. })) => {
                self.modal = None;
                D16Command::None
            }
            KeyCode::Char('y') if matches!(self.modal, Some(D16Modal::ConfirmArchive { .. })) => {
                self.confirm_action_modal()
            }
            KeyCode::Enter => self.confirm_action_modal(),
            KeyCode::Backspace => {
                if let Some(value) = modal_text_mut(self.modal.as_mut()) {
                    value.pop();
                }
                D16Command::None
            }
            KeyCode::Char(character) if !character.is_control() => {
                if let Some(value) = modal_text_mut(self.modal.as_mut())
                    && value.chars().count() < 256
                {
                    value.push(character);
                }
                D16Command::None
            }
            _ => D16Command::None,
        }
    }

    fn confirm_action_modal(&mut self) -> D16Command {
        let Some(modal) = self.modal.take() else {
            return D16Command::None;
        };
        match modal {
            D16Modal::ConfirmArchive {
                workstream_id,
                expected_revision,
            } => self.record_action(ApplicationAction::Archive {
                workstream_id,
                expected_revision,
            }),
            D16Modal::Rename {
                workstream_id,
                expected_revision,
                value,
            } => {
                if value.trim().is_empty() {
                    self.message =
                        Some("Rename requires a non-empty native thread name".to_owned());
                    self.modal = Some(D16Modal::Rename {
                        workstream_id,
                        expected_revision,
                        value,
                    });
                    return D16Command::None;
                }
                self.record_action(ApplicationAction::Rename {
                    workstream_id,
                    expected_revision,
                    name: value,
                })
            }
            D16Modal::SetBrowserRoot {
                expected_revision,
                value,
            } => {
                let Ok(root_path) = BrowserRootPath::new(value.clone()) else {
                    self.message =
                        Some("browser root must be an absolute normalized path".to_owned());
                    self.modal = Some(D16Modal::SetBrowserRoot {
                        expected_revision,
                        value,
                    });
                    return D16Command::None;
                };
                self.set_browser_root(root_path, expected_revision)
            }
        }
    }

    fn restore_selected(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if !row.workstream.archived {
            return D16Command::None;
        }
        self.record_action(ApplicationAction::Restore {
            workstream_id: row.workstream.workstream_id,
            expected_revision: row.workstream.revision,
        })
    }

    fn acknowledge_selected(&mut self) -> D16Command {
        let Some(row) = self.selected_workstream_owned() else {
            return D16Command::None;
        };
        if !row.workstream.attention.result_unseen {
            return D16Command::None;
        }
        self.record_action(ApplicationAction::AcknowledgeAttention {
            workstream_id: row.workstream.workstream_id,
            expected_revision: row.workstream.attention.revision,
            kind: AttentionKind::Result,
        })
    }

    fn new_at_selected_location(&mut self) -> D16Command {
        let Some(row) = self.selected_location() else {
            return D16Command::None;
        };
        let Some(project) = self.project(row.project_id) else {
            return D16Command::None;
        };
        self.request_provider(
            D16ProviderRequest::NewAtLocation {
                project_id: row.project_id,
                location_id: row.location_id,
                expected_project_revision: project.revision,
                expected_location_revision: row.revision,
            },
            None,
        )
    }

    fn recover_selected_operation(&mut self) -> D16Command {
        let Some(operation) = self.selected_operation() else {
            return D16Command::None;
        };
        let Some(capability) = self
            .snapshot
            .provider_capabilities
            .iter()
            .find(|capability| capability.provider == operation.provider)
        else {
            self.message = Some("no local provider is ready for recovery".to_owned());
            return D16Command::None;
        };
        if !capability.eligible_for_resume() {
            self.message = Some("the operation provider is not ready for recovery".to_owned());
            return D16Command::None;
        }
        self.record_action(ApplicationAction::RecoverOperation {
            operation_id: operation.operation_id,
            expected_revision: operation.revision,
            provider: operation.provider,
        })
    }

    fn refresh_selected_project(&mut self) -> D16Command {
        let project_id = match self.selected_row() {
            Some(D16Row::Location(row)) => row.project_id,
            _ => return D16Command::None,
        };
        self.refresh_project(project_id).unwrap_or(D16Command::None)
    }

    fn eligible_new_providers(&self) -> Vec<ProviderKind> {
        let mut providers = self
            .snapshot
            .provider_capabilities
            .iter()
            .filter(|capability| capability.eligible_for_new())
            .map(|capability| capability.provider)
            .collect::<Vec<_>>();
        providers.sort_unstable();
        providers
    }

    fn request_provider(
        &mut self,
        request: D16ProviderRequest,
        preferred: Option<ProviderKind>,
    ) -> D16Command {
        let providers = self.eligible_new_providers();
        match providers.as_slice() {
            [] => {
                self.message = Some("no local provider is ready for a new Workstream".to_owned());
                D16Command::None
            }
            [provider] => self.record_action(request.action(*provider)),
            _ => {
                let selected = preferred
                    .and_then(|provider| providers.iter().position(|item| *item == provider))
                    .unwrap_or(0);
                self.provider_chooser = Some(D16ProviderChooser {
                    providers,
                    selected,
                    request,
                });
                D16Command::None
            }
        }
    }

    fn record_action(&mut self, action: ApplicationAction) -> D16Command {
        self.pending_action = Some(action.clone());
        D16Command::Apply(action)
    }

    fn accept_revised_identity(&mut self, identity: RevisedIdentity) {
        if let RevisedIdentity::Workstream(workstream_id, _) = identity {
            if matches!(self.pending_action, Some(ApplicationAction::Restore { .. })) {
                self.page = D16Page::Workstreams;
            }
            self.selected = Some(D16RowId::Workstream(workstream_id));
        }
    }

    fn browser_list_command(&mut self) -> D16Command {
        let Some(browser) = self.browser.as_ref() else {
            return D16Command::None;
        };
        self.record_action(ApplicationAction::ListProjectBrowser {
            relative_path: browser.path.clone(),
            include_hidden: browser.include_hidden,
        })
    }

    fn handle_browser_key(&mut self, key: KeyCode) -> D16Command {
        match key {
            KeyCode::Esc => {
                self.close_project_browser();
                D16Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(browser) = self.browser.as_mut() {
                    browser.selected = browser.selected.saturating_sub(1);
                    ensure_browser_selection_visible(browser);
                }
                D16Command::None
            }
            KeyCode::Down => {
                if let Some(browser) = self.browser.as_mut() {
                    let count = browser.listing.as_ref().map_or(0, |listing| {
                        filtered_browser_entries_for(listing, &browser.filter).len()
                    });
                    if count > 0 {
                        browser.selected = min(browser.selected.saturating_add(1), count - 1);
                        ensure_browser_selection_visible(browser);
                    }
                }
                D16Command::None
            }
            KeyCode::Char('.') => {
                if let Some(browser) = self.browser.as_mut() {
                    browser.include_hidden = !browser.include_hidden;
                    browser.selected = 0;
                    browser.scroll = 0;
                }
                self.browser_list_command()
            }
            KeyCode::Left => self.browser_parent_command(),
            KeyCode::Right => self.browser_child_command(),
            KeyCode::Enter => self.browser_enter_command(),
            KeyCode::Backspace => {
                if let Some(browser) = self.browser.as_mut() {
                    browser.filter.pop();
                    browser.selected = 0;
                    browser.scroll = 0;
                    ensure_browser_selection_visible(browser);
                }
                D16Command::None
            }
            KeyCode::Char(character) if !character.is_control() => {
                if let Some(browser) = self.browser.as_mut()
                    && browser.filter.chars().count() < 128
                {
                    browser.filter.push(character);
                    browser.selected = 0;
                    browser.scroll = 0;
                    ensure_browser_selection_visible(browser);
                }
                D16Command::None
            }
            _ => D16Command::None,
        }
    }

    fn handle_provider_chooser_key(&mut self, key: KeyCode) -> D16Command {
        match key {
            KeyCode::Esc => {
                self.provider_chooser = None;
                D16Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(chooser) = self.provider_chooser.as_mut() {
                    chooser.selected = chooser.selected.saturating_sub(1);
                }
                D16Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(chooser) = self.provider_chooser.as_mut() {
                    chooser.selected = min(
                        chooser.selected.saturating_add(1),
                        chooser.providers.len().saturating_sub(1),
                    );
                }
                D16Command::None
            }
            KeyCode::Enter => {
                let Some(chooser) = self.provider_chooser.take() else {
                    return D16Command::None;
                };
                let Some(provider) = chooser.providers.get(chooser.selected).copied() else {
                    return D16Command::None;
                };
                let action = chooser.request.action(provider);
                if matches!(chooser.request, D16ProviderRequest::RegisterLocation { .. }) {
                    self.browser = None;
                }
                self.record_action(action)
            }
            _ => D16Command::None,
        }
    }

    fn browser_parent_command(&mut self) -> D16Command {
        let Some(browser) = self.browser.as_mut() else {
            return D16Command::None;
        };
        let value = browser.path.as_str();
        let parent = value
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent.trim_end_matches('/'));
        let path = if parent.is_empty() {
            BrowserPath::root()
        } else {
            match BrowserPath::new(parent.to_owned()) {
                Ok(path) => path,
                Err(_) => return D16Command::None,
            }
        };
        browser.path = path;
        browser.listing = None;
        browser.filter.clear();
        browser.selected = 0;
        browser.scroll = 0;
        self.browser_list_command()
    }

    fn browser_child_command(&mut self) -> D16Command {
        let Some((path, entry)) = self.selected_browser_entry() else {
            return D16Command::None;
        };
        let child = join_browser_path(path.as_str(), &entry.name).ok();
        let Some(child) = child else {
            return D16Command::None;
        };
        let Some(browser) = self.browser.as_mut() else {
            return D16Command::None;
        };
        browser.path = child;
        browser.listing = None;
        browser.filter.clear();
        browser.selected = 0;
        browser.scroll = 0;
        self.browser_list_command()
    }

    fn browser_enter_command(&mut self) -> D16Command {
        let Some((path, entry)) = self.selected_browser_entry() else {
            return D16Command::None;
        };
        let Some(relative_path) = join_browser_path(path.as_str(), &entry.name).ok() else {
            return D16Command::None;
        };
        if !entry.is_git_repository {
            self.message = Some("plain folder selected; use Right to enter it".to_owned());
            return D16Command::None;
        }
        let Some(expected_browser_revision) = self
            .browser
            .as_ref()
            .and_then(|browser| browser.listing.as_ref())
            .map(|listing| listing.revision)
        else {
            return D16Command::None;
        };
        self.request_provider(
            D16ProviderRequest::RegisterLocation {
                relative_path,
                expected_browser_revision,
            },
            None,
        )
    }

    fn selected_browser_entry(&self) -> Option<(&BrowserPath, crate::application::BrowserEntry)> {
        let browser = self.browser.as_ref()?;
        let listing = browser.listing.as_ref()?;
        let entries = filtered_browser_entries_for(listing, &browser.filter);
        let entry = (*entries.get(browser.selected)?).clone();
        Some((&browser.path, entry))
    }
}

/// Thin controller wrapper used by a future D16 activation path.  It owns the
/// model and only forwards pure events; no application backend is stored here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D16Navigator {
    model: D16Model,
}

impl D16Navigator {
    #[must_use]
    pub fn new(snapshot: ApplicationSnapshot) -> Self {
        Self {
            model: D16Model::new(snapshot),
        }
    }

    #[must_use]
    pub const fn model(&self) -> &D16Model {
        &self.model
    }

    pub const fn model_mut(&mut self) -> &mut D16Model {
        &mut self.model
    }

    pub fn replace_snapshot(&mut self, snapshot: ApplicationSnapshot) {
        self.model.replace_snapshot(snapshot);
    }

    pub fn handle_key(&mut self, key: KeyCode) -> D16Command {
        self.model.handle_key(key)
    }

    pub fn accept_outcome(&mut self, outcome: ApplicationOutcome) {
        self.model.accept_outcome(outcome);
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        render_model(frame, area, &self.model, SystemClock.now_millis().ok());
    }

    /// Renders with an explicit wall-clock sample. Production callers use the
    /// system clock through [`Self::render`]; tests and visual probes can pass
    /// a deterministic value without depending on timing.
    pub fn render_at(&self, frame: &mut Frame<'_>, area: Rect, now_millis: Option<i64>) {
        render_model(frame, area, &self.model, now_millis);
    }

    /// Computes the exact list geometry used by the renderer for hit testing.
    #[must_use]
    pub fn list_geometry(&self, area: Rect) -> D16ListGeometry {
        list_geometry(area, model_status(&self.model), self.model.page)
    }

    /// Resolves a terminal coordinate to one exact actionable row. Project
    /// headers and coordinates outside the bordered list intentionally return
    /// no identity.
    #[must_use]
    pub fn row_at(&self, area: Rect, column: u16, row: u16) -> Option<D16RowId> {
        let geometry = self.list_geometry(area);
        if column < geometry.inner.x
            || row < geometry.inner.y
            || column >= geometry.inner.x.saturating_add(geometry.inner.width)
            || row >= geometry.inner.y.saturating_add(geometry.inner.height)
        {
            return None;
        }
        let line = usize::from(row.saturating_sub(geometry.inner.y));
        self.model
            .row_id_at_render_line(geometry.viewport_rows, line)
    }
}

fn join_browser_path(parent: &str, child: &str) -> Result<BrowserPath, ApplicationError> {
    let value = if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    };
    BrowserPath::new(value)
}

fn ensure_browser_selection_visible(browser: &mut D16ProjectBrowser) {
    let count = browser.listing.as_ref().map_or(0, |listing| {
        filtered_browser_entries_for(listing, &browser.filter).len()
    });
    let viewport = BROWSER_VIEWPORT_ROWS;
    let max_start = count.saturating_sub(viewport);
    browser.scroll = browser.scroll.min(max_start);
    if browser.selected < browser.scroll {
        browser.scroll = browser.selected;
    } else if browser.selected >= browser.scroll.saturating_add(viewport) {
        browser.scroll = browser
            .selected
            .saturating_add(1)
            .saturating_sub(viewport)
            .min(max_start);
    }
}

fn filtered_browser_entries_for<'a>(
    listing: &'a BrowserListing,
    filter: &str,
) -> Vec<&'a crate::application::BrowserEntry> {
    if filter.is_empty() {
        return listing.entries.iter().collect();
    }
    let filter = filter.to_lowercase();
    listing
        .entries
        .iter()
        .filter(|entry| entry.name.to_lowercase().contains(&filter))
        .collect()
}

fn modal_text_mut(modal: Option<&mut D16Modal>) -> Option<&mut String> {
    match modal? {
        D16Modal::Rename { value, .. } | D16Modal::SetBrowserRoot { value, .. } => Some(value),
        D16Modal::ConfirmArchive { .. } => None,
    }
}

fn model_status(model: &D16Model) -> Option<&str> {
    model.message.as_deref().or_else(|| {
        model
            .observer_guide
            .as_ref()
            .map(|_| "observer readiness guidance is awaiting an explicit choice")
    })
}

fn footer_height(area: Rect, status: Option<&str>, page: D16Page) -> u16 {
    let controls = controls_height(page, area.width);
    let desired = status.map_or(controls, |status| {
        status_block_height(area, status).saturating_add(controls)
    });
    desired.min(area.height.saturating_sub(1))
}

fn status_block_height(area: Rect, status: &str) -> u16 {
    let content_width = usize::from(area.width.saturating_sub(2).max(1));
    let content_height = wrapped_display_line_count(status, content_width).max(1);
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

fn list_geometry(area: Rect, status: Option<&str>, page: D16Page) -> D16ListGeometry {
    let footer_height = footer_height(area, status, page);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(area);
    let outer = vertical[0];
    let inner = Rect::new(
        outer.x.saturating_add(1),
        outer.y.saturating_add(1),
        outer.width.saturating_sub(2),
        outer.height.saturating_sub(2),
    );
    D16ListGeometry {
        outer,
        inner,
        viewport_rows: match page {
            D16Page::Projects => usize::from(inner.height).max(1),
            D16Page::Workstreams | D16Page::Archived => usize::from(inner.height / 2).max(1),
        },
    }
}

fn render_model(frame: &mut Frame<'_>, area: Rect, model: &D16Model, now_millis: Option<i64>) {
    let status = model_status(model);
    let footer_height = footer_height(area, status, model.page);
    let geometry = list_geometry(area, status, model.page);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(area);
    let all_rows = model.rows();
    let viewport = geometry.viewport_rows;
    let (start, rows) = model.visible_rows(viewport);
    let selected = model.selected;
    let project_colors = visible_project_colors(&rows);
    let available_width = geometry.inner.width;
    let items = rows
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            let is_selected = row.id() == selected;
            let global_index = start.saturating_add(offset);
            let tree_last = all_rows
                .get(global_index.saturating_add(1))
                .is_none_or(|next| !same_project_row(row, next));
            let item = ListItem::new(row_lines(
                row,
                tree_last,
                &project_colors,
                available_width,
                now_millis,
            ));
            if is_selected {
                item.style(Style::default().bg(SELECTED_ROW_BACKGROUND))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let title = format!(" {} ", model.page.title());
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
    );
    frame.render_widget(list, vertical[0]);

    if let Some(status) = status {
        let controls_height = controls_height(model.page, area.width).min(vertical[1].height);
        let status_height = vertical[1].height.saturating_sub(controls_height);
        let footer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(status_height),
                Constraint::Length(controls_height),
            ])
            .split(vertical[1]);
        frame.render_widget(
            Paragraph::new(status).wrap(Wrap { trim: true }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow))
                    .title(Span::styled(" Status ", Style::default().fg(Color::Yellow))),
            ),
            footer[0],
        );
        frame.render_widget(
            Paragraph::new(controls_lines(model.page, footer[1].width)),
            footer[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(controls_lines(model.page, vertical[1].width)),
            vertical[1],
        );
    }

    if let Some(browser) = &model.browser {
        render_browser(frame, area, browser);
    }
    if let Some(chooser) = &model.provider_chooser {
        render_provider_chooser(frame, area, chooser);
    }
    if let Some(modal) = &model.modal {
        render_action_modal(frame, area, modal);
    }
    if let Some(guide) = model.observer_guide {
        render_observer_guide(frame, area, guide);
    }
    if model.help_visible {
        render_help(frame, area, model.page, model.help_scroll);
    }
}

fn same_project_row(left: &D16Row, right: &D16Row) -> bool {
    row_project_id(left)
        .zip(row_project_id(right))
        .is_some_and(|(left, right)| left == right)
}

#[allow(clippy::too_many_lines)]
fn row_lines(
    row: &D16Row,
    tree_last: bool,
    project_colors: &BTreeMap<ProjectId, Color>,
    available_width: u16,
    now_millis: Option<i64>,
) -> Vec<Line<'static>> {
    match row {
        D16Row::ProjectHeader(row) => vec![Line::from(Span::styled(
            row.display_name.clone(),
            Style::default()
                .fg(project_accent(row.project_id, project_colors))
                .add_modifier(Modifier::BOLD),
        ))],
        D16Row::Location(row) => {
            let prefix = if row.grouped_under_project {
                if tree_last { "└ " } else { "├ " }
            } else {
                ""
            };
            let name_budget = usize::from(available_width).saturating_sub(display_width(prefix));
            vec![Line::from(vec![
                Span::styled(prefix.to_owned(), Style::default().fg(PROJECT_TREE_COLOR)),
                Span::styled(
                    truncate_display(&row.display_name, name_budget),
                    Style::default()
                        .fg(project_accent(row.project_id, project_colors))
                        .add_modifier(Modifier::BOLD),
                ),
            ])]
        }
        D16Row::Workstream(row) => {
            let branch = if tree_last { "└ " } else { "├ " };
            let continuation = if tree_last { "  " } else { "│ " };
            let (marker, marker_style) = workstream_marker(&row.workstream);
            let title = workstream_name(&row.workstream);
            let age = activity_label(row.workstream.last_activity_at_millis, now_millis);
            let age_style = Style::default().fg(activity_age_color(
                row.workstream.last_activity_at_millis,
                now_millis,
            ));
            vec![
                Line::from(context_line(
                    branch,
                    row.workstream.provider,
                    age,
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
        D16Row::Operation(row) => vec![Line::from(vec![
            Span::styled("  Recovery", Style::default().fg(Color::Red)),
            Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
            Span::styled(
                format!("{:?} · {:?}", row.operation.kind, row.operation.phase),
                Style::default().fg(Color::Red),
            ),
            Span::styled(" · ", Style::default().fg(PROJECT_TREE_COLOR)),
            Span::styled("r retry", Style::default().fg(Color::Yellow)),
        ])],
    }
}

const fn provider_display(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Codex => "Codex",
        ProviderKind::OpenCode => "OpenCode",
    }
}

fn workstream_marker(workstream: &WorkstreamSnapshot) -> (&'static str, Style) {
    if workstream.lifecycle == WorkstreamLifecycle::Parked {
        ("p", Style::default().fg(PARKED_INDICATOR_COLOR))
    } else if workstream.attention.recovery_unseen
        || workstream.lifecycle == WorkstreamLifecycle::RecoveryRequired
    {
        ("!", Style::default().fg(Color::Red))
    } else if workstream.attention.result_unseen {
        ("✓", Style::default().fg(Color::Green))
    } else if workstream
        .runtime
        .is_some_and(|runtime| runtime.observer_degraded)
    {
        ("?", Style::default().fg(Color::Red))
    } else {
        match workstream.runtime.map(|runtime| runtime.status) {
            Some(RuntimeStatus::Working) => ("●", Style::default().fg(Color::Yellow)),
            Some(RuntimeStatus::Starting) => ("…", Style::default().fg(Color::Cyan)),
            Some(RuntimeStatus::Unknown) => ("?", Style::default().fg(Color::Red)),
            Some(RuntimeStatus::Stopped | RuntimeStatus::Attention | RuntimeStatus::Idle)
            | None => (" ", Style::default()),
        }
    }
}

fn workstream_name(workstream: &WorkstreamSnapshot) -> String {
    workstream
        .native_name
        .clone()
        .unwrap_or_else(|| workstream.workstream_id.short())
}

fn context_line(
    prefix: &str,
    provider_kind: ProviderKind,
    age: String,
    age_style: Style,
    available_width: u16,
) -> Vec<Span<'static>> {
    let available_width = usize::from(available_width);
    let prefix_width = display_width(prefix);
    let content_width = available_width.saturating_sub(prefix_width);
    let provider = truncate_display(provider_display(provider_kind), content_width);
    let provider_width = display_width(&provider);
    let age_budget = content_width
        .saturating_sub(provider_width)
        .saturating_sub(usize::from(!provider.is_empty()));
    let age = truncate_display(&age, age_budget);
    let padding =
        available_width.saturating_sub(prefix_width + provider_width + display_width(&age));

    vec![
        Span::styled(prefix.to_owned(), Style::default().fg(PROJECT_TREE_COLOR)),
        Span::styled(
            provider,
            Style::default()
                .fg(provider_color(provider_kind))
                .add_modifier(Modifier::BOLD),
        ),
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
    let fixed_width = display_width(prefix) + display_width(indicator) + 1;
    let title_budget = usize::from(available_width).saturating_sub(fixed_width);
    let title = truncate_display(title, title_budget);
    vec![
        Span::styled(prefix.to_owned(), Style::default().fg(PROJECT_TREE_COLOR)),
        Span::styled(indicator.to_owned(), indicator_style),
        Span::raw(" "),
        Span::styled(title, Style::default().fg(Color::White)),
    ]
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

fn display_width(value: &str) -> usize {
    Line::raw(value).width()
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

const fn provider_color(provider: ProviderKind) -> Color {
    match provider {
        ProviderKind::Codex => PROVIDER_LABEL_PALETTE[0],
        ProviderKind::OpenCode => PROVIDER_LABEL_PALETTE[1],
    }
}

fn project_accent(project_id: ProjectId, project_colors: &BTreeMap<ProjectId, Color>) -> Color {
    project_colors
        .get(&project_id)
        .copied()
        .unwrap_or(PROJECT_MARKER_PALETTE[0])
}

fn visible_project_colors(rows: &[D16Row]) -> BTreeMap<ProjectId, Color> {
    let project_ids = rows
        .iter()
        .filter_map(row_project_id)
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

fn row_project_id(row: &D16Row) -> Option<ProjectId> {
    match row {
        D16Row::ProjectHeader(row) => Some(row.project_id),
        D16Row::Location(row) => Some(row.project_id),
        D16Row::Workstream(row) => Some(row.workstream.project_id),
        D16Row::Operation(_) => None,
    }
}

fn stable_color_index(seed: &[u8], palette_len: usize) -> usize {
    debug_assert!(palette_len > 0);
    let hash = seed.iter().fold(0_u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u64::from(*byte))
    });
    usize::try_from(hash % u64::try_from(palette_len).unwrap()).unwrap()
}

fn control_bindings(page: D16Page) -> &'static [(&'static str, &'static str)] {
    match page {
        D16Page::Workstreams => &[
            ("↑↓", "select"),
            ("n", "new"),
            ("f", "fork"),
            ("p", "park"),
            ("r", "rename"),
            ("x", "archive"),
            (",", "projects"),
            (".", "archived"),
            ("?", "help"),
        ],
        D16Page::Projects => &[
            ("↑↓", "select"),
            ("a", "add"),
            ("b", "root"),
            ("n", "new"),
            ("r", "refresh"),
            (",", "workstreams"),
            (".", "archived"),
            ("?", "help"),
        ],
        D16Page::Archived => &[
            ("↑↓", "select"),
            ("u", "restore"),
            (",", "projects"),
            (".", "workstreams"),
            ("?", "help"),
        ],
    }
}

fn controls_height(page: D16Page, width: u16) -> u16 {
    u16::try_from(controls_lines(page, width).len())
        .unwrap_or(u16::MAX)
        .max(1)
}

fn controls_lines(page: D16Page, width: u16) -> Vec<Line<'static>> {
    let key = Style::default().fg(Color::Yellow);
    let label = Style::default().fg(Color::Gray);
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1_usize;
    for (shortcut, description) in control_bindings(page) {
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

fn render_browser(frame: &mut Frame<'_>, area: Rect, browser: &D16ProjectBrowser) {
    let width = area.width.min(70);
    let height = area.height.min(16);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let mut lines = vec![Line::from(Span::styled(
        if browser.path.as_str().is_empty() {
            format!("root: {}", browser.root_label)
        } else {
            format!("path: {}/{}", browser.root_label, browser.path.as_str())
        },
        Style::default().fg(Color::Cyan),
    ))];
    if let Some(listing) = &browser.listing {
        let entries = filtered_browser_entries_for(listing, &browser.filter);
        let viewport = usize::from(height.saturating_sub(4)).max(1);
        let start = browser_visible_start(browser, viewport);
        for (index, entry) in entries.iter().enumerate().skip(start).take(viewport) {
            let marker = if index == browser.selected {
                "> "
            } else {
                "  "
            };
            let suffix = if entry.is_git_repository {
                " [git]"
            } else {
                "/"
            };
            let style = if index == browser.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{marker}{}{suffix}", entry.name),
                style,
            )));
        }
    } else {
        lines.push(Line::raw("  loading host-local directories…"));
    }
    if !browser.filter.is_empty() {
        lines.push(Line::raw(format!("filter: {}", browser.filter)));
    }
    lines.push(Line::raw(
        "← parent · → enter · . hidden · Enter register · Esc close",
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Project browser ");
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn browser_visible_start(browser: &D16ProjectBrowser, viewport: usize) -> usize {
    let count = browser.listing.as_ref().map_or(0, |listing| {
        filtered_browser_entries_for(listing, &browser.filter).len()
    });
    let viewport = viewport.max(1);
    let max_start = count.saturating_sub(viewport);
    let mut start = browser.scroll.min(max_start);
    if browser.selected < start {
        start = browser.selected;
    } else if browser.selected >= start.saturating_add(viewport) {
        start = browser
            .selected
            .saturating_add(1)
            .saturating_sub(viewport)
            .min(max_start);
    }
    start
}

fn render_provider_chooser(frame: &mut Frame<'_>, area: Rect, chooser: &D16ProviderChooser) {
    let width = area.width.min(46);
    let height = area.height.min(8);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let mut lines = vec![Line::raw("Choose provider for this new Workstream")];
    for (index, provider) in chooser.providers.iter().enumerate() {
        let marker = if index == chooser.selected {
            "> "
        } else {
            "  "
        };
        lines.push(Line::raw(format!("{marker}{provider}")));
    }
    lines.push(Line::raw("↑/↓ choose · Enter accept · Esc cancel"));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Provider ")),
        popup,
    );
}

fn render_observer_guide(frame: &mut Frame<'_>, area: Rect, guide: ObserverReadinessGuide) {
    let width = area.width.min(62);
    let height = area.height.min(10);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let (change, authority) = match guide.evidence.readiness {
        crate::application::ObserverReadiness::SetupRequired => (
            "Enter prepares the exact owned wsnav-observer declaration.",
            "No provider action occurs before native trust review.",
        ),
        crate::application::ObserverReadiness::UpdateRequired => (
            "Enter replaces the exact owned observer declaration.",
            "Provider-owned settings remain preserved; native trust is required again.",
        ),
        crate::application::ObserverReadiness::TrustReviewRequired => (
            "Enter opens native review; no declaration write is requested.",
            "Trust is recorded only after the exact native review is verified.",
        ),
        crate::application::ObserverReadiness::Foreign => (
            "Foreign observer ownership is present; this action is refused.",
            "Dismiss and reconcile ownership outside this action.",
        ),
        crate::application::ObserverReadiness::Modified => (
            "The owned observer declaration changed; this action is refused.",
            "Restore exact ownership before retrying.",
        ),
        crate::application::ObserverReadiness::Disabled => (
            "Observer integration is disabled; this action is refused.",
            "Re-enable it through an explicit host-local policy change.",
        ),
        crate::application::ObserverReadiness::Ambiguous
        | crate::application::ObserverReadiness::Unknown
        | crate::application::ObserverReadiness::Ready => (
            "Observer readiness is ambiguous; this action is refused.",
            "Retry after a fresh read-only readiness check.",
        ),
    };
    let revision = guide.evidence.integration_revision.map_or_else(
        || "none".to_owned(),
        |revision| revision.value().to_string(),
    );
    let lines = vec![
        Line::raw("Observer readiness guide"),
        Line::raw(change),
        Line::raw(authority),
        Line::raw(format!("Captured integration revision: {revision}")),
        Line::raw("Enter accept · Esc dismiss · other keys inert"),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Observer guide "),
        ),
        popup,
    );
}

fn render_action_modal(frame: &mut Frame<'_>, area: Rect, modal: &D16Modal) {
    let width = area.width.min(62);
    let height = area.height.min(8);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let (title, lines) = match modal {
        D16Modal::ConfirmArchive { .. } => (
            " Archive Workstream ",
            vec![
                Line::raw("Archive this Workstream? A live Runtime may be parked first."),
                Line::raw("Enter/y confirm · n/Esc cancel · other keys inert"),
            ],
        ),
        D16Modal::Rename { value, .. } => (
            " Rename native thread ",
            vec![
                Line::raw(value.clone()),
                Line::raw("Enter apply · Esc cancel"),
            ],
        ),
        D16Modal::SetBrowserRoot { value, .. } => (
            " Project browser root ",
            vec![
                Line::raw(value.clone()),
                Line::raw("Enter apply absolute path · Esc cancel"),
            ],
        ),
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(title)),
        popup,
    );
}

fn render_help(frame: &mut Frame<'_>, area: Rect, page: D16Page, scroll: usize) {
    let bindings: &[(&str, &str, Color)] = match page {
        D16Page::Workstreams => &[
            ("↑↓", "Select", Color::Gray),
            ("Enter", "Open", Color::Green),
            ("n", "New here", Color::Green),
            ("f", "Fork", Color::White),
            ("p", "Park", PARKED_INDICATOR_COLOR),
            ("r", "Rename", Color::White),
            ("x", "Archive", Color::Red),
            ("a", "Clear attention", Color::Green),
            (",", "Projects", Color::Cyan),
            (".", "Archived", Color::Cyan),
            ("?/Esc/q", "Close help", Color::Cyan),
        ],
        D16Page::Projects => &[
            ("↑↓", "Select checkout", Color::Gray),
            ("a", "Add checkout", Color::Green),
            ("b", "Browser root", Color::White),
            ("n", "New here", Color::Green),
            ("r", "Refresh project", Color::White),
            (",", "Workstreams", Color::Cyan),
            (".", "Archived", Color::Cyan),
            ("?/Esc/q", "Close help", Color::Cyan),
        ],
        D16Page::Archived => &[
            ("↑↓", "Select", Color::Gray),
            ("u", "Restore", Color::Green),
            (",", "Projects", Color::Cyan),
            (".", "Workstreams", Color::Cyan),
            ("?/Esc/q", "Close help", Color::Cyan),
        ],
    };
    let desired_height = u16::try_from(bindings.len().saturating_add(2)).unwrap_or(u16::MAX);
    let height = area.height.min(desired_height);
    let popup = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height),
        area.width,
        height,
    );
    frame.render_widget(Clear, popup);
    let inner_rows = usize::from(height.saturating_sub(2)).max(1);
    let first = scroll.min(bindings.len().saturating_sub(inner_rows));
    let content_width = usize::from(popup.width.saturating_sub(2).max(1));
    let key_column = 7_usize.min(content_width.saturating_sub(1));
    let lines = bindings
        .iter()
        .skip(first)
        .take(inner_rows)
        .map(|(shortcut, action, color)| {
            let key_width = display_width(shortcut);
            let gap = key_column.saturating_sub(key_width).saturating_add(1);
            let action_budget = content_width.saturating_sub(key_width).saturating_sub(gap);
            Line::from(vec![
                Span::styled(
                    (*shortcut).to_owned(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ".repeat(gap)),
                Span::styled(
                    truncate_display(action, action_budget),
                    Style::default().fg(*color),
                ),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    format!(" {} keys ", page.title()),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        popup,
    );
}
