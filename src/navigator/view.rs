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
        OperationId, OperationKind, OperationPhase, ProviderKind, Revision, RuntimeId,
        RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
    },
    snapshot::{OnboardingStatus, OperationSnapshot, Snapshot, WorkstreamSnapshot},
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
    Operation(OperationId),
}

/// One rendered Workstreams row. Project headings are context only and can
/// never become an action target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Row {
    ProvisionalShell { location: ShellLocation },
    ProjectHeader { display_name: String },
    Workstream(WorkstreamSnapshot),
    Operation(OperationSnapshot),
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
            Self::Operation(operation) => Some(RowId::Operation(operation.operation_id)),
        }
    }

    #[must_use]
    pub(crate) fn render_height(&self) -> usize {
        match self {
            Self::ProjectHeader { .. } => 1,
            Self::ProvisionalShell { .. } | Self::Workstream(_) | Self::Operation(_) => 2,
        }
    }
}

/// The only effects a terminal controller may request. Provider kind and
/// location are carried only for contextual same-session actions; a new shell
/// deliberately has neither field because the native command owns both.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    Park {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Archive {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Restore {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    AcknowledgeResult {
        workstream_id: WorkstreamId,
        expected_attention_revision: Revision,
    },
    Fork {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    RecoverOperation {
        operation_id: OperationId,
        expected_operation_revision: Revision,
        provider: ProviderKind,
    },
    /// Opens the contextual setup guide before a Codex action can proceed.
    /// The guide carries no provider argv or profile path.
    AcceptObserverSetup {
        kind: ObserverSetupKind,
    },
    /// Dismisses the contextual observer setup guide without any mutation.
    CancelObserverSetup,
    Rename {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        name: String,
    },
    ShowGuidance(&'static str),
}

/// Process-local action confirmation/input state. It deliberately retains
/// only one exact Workstream revision and bounded display text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Modal {
    ConfirmArchive {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Rename {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        value: String,
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

    /// Renders the -only Workstreams/Archived surface. The renderer has no
    /// provider-pane, shell, state, or filesystem effect.
    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        render_model(frame, area, &self.model, self.terminal_focused);
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
                KeyCode::Char('?' | 'q') | KeyCode::Esc => {
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
            KeyCode::Char('f') => self.fork_selected(),
            KeyCode::Char('p') => self.park_selected(),
            KeyCode::Char('x') => self.archive_selected(),
            KeyCode::Char('u') => self.restore_selected(),
            KeyCode::Char('a') => self.acknowledge_selected(),
            KeyCode::Char('r') => self.rename_or_recover_selected(),
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

    fn fork_selected(&self) -> Command {
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
                Command::ShowGuidance(ONBOARDING_RECOVERY_GUIDANCE)
            }
            None if !workstream.archived => Command::Fork {
                source_workstream_id: workstream_id,
                expected_workstream_revision: workstream.revision,
                provider: workstream.provider,
            },
            None => Command::None,
        }
    }

    fn park_selected(&self) -> Command {
        let Some(RowId::Workstream(workstream_id)) = self.selected else {
            return Command::None;
        };
        let Some(workstream) = self.selected_workstream(workstream_id) else {
            return Command::None;
        };
        if workstream.archived {
            return Command::None;
        }
        match workstream.onboarding {
            Some(OnboardingStatus::ActionFenced) => {
                Command::ShowGuidance(ONBOARDING_IN_PROGRESS_GUIDANCE)
            }
            Some(OnboardingStatus::RecoveryRequired) | None => Command::Park {
                workstream_id,
                expected_workstream_revision: workstream.revision,
            },
        }
    }

    fn archive_selected(&mut self) -> Command {
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
                Command::ShowGuidance(ONBOARDING_RECOVERY_GUIDANCE)
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

    fn acknowledge_selected(&self) -> Command {
        if self.page != Page::Workstreams {
            return Command::None;
        }
        let Some(RowId::Workstream(workstream_id)) = self.selected else {
            return Command::None;
        };
        self.selected_workstream(workstream_id)
            .filter(|workstream| {
                !workstream.archived && workstream.onboarding.is_none() && workstream.result_unseen
            })
            .map_or(Command::None, |workstream| Command::AcknowledgeResult {
                workstream_id,
                expected_attention_revision: workstream.attention_revision,
            })
    }

    fn rename_or_recover_selected(&mut self) -> Command {
        match self.selected {
            Some(RowId::Operation(operation_id)) if self.page == Page::Workstreams => self
                .selected_operation(operation_id)
                .map_or(Command::None, |operation| Command::RecoverOperation {
                    operation_id,
                    expected_operation_revision: operation.revision,
                    provider: operation.provider,
                }),
            Some(RowId::Workstream(workstream_id)) if self.page == Page::Workstreams => {
                let Some(workstream) = self.selected_workstream(workstream_id) else {
                    return Command::None;
                };
                if workstream.onboarding == Some(OnboardingStatus::ActionFenced) {
                    return Command::ShowGuidance(ONBOARDING_IN_PROGRESS_GUIDANCE);
                }
                if workstream.onboarding == Some(OnboardingStatus::RecoveryRequired) {
                    return Command::ShowGuidance(ONBOARDING_RECOVERY_GUIDANCE);
                }
                if workstream.archived || workstream.provider != ProviderKind::Codex {
                    return Command::ShowGuidance(RENAME_UNAVAILABLE_GUIDANCE);
                }
                self.modal = Some(Modal::Rename {
                    workstream_id,
                    expected_workstream_revision: workstream.revision,
                    value: workstream.native_name.clone().unwrap_or_default(),
                });
                Command::None
            }
            _ => Command::None,
        }
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
            KeyCode::Backspace => {
                if let Some(Modal::Rename { value, .. }) = self.modal.as_mut() {
                    value.pop();
                }
                Command::None
            }
            KeyCode::Char(character) if !character.is_control() => {
                if let Some(Modal::Rename { value, .. }) = self.modal.as_mut()
                    && value.chars().count() < 256
                {
                    value.push(character);
                }
                Command::None
            }
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
            Modal::Rename {
                workstream_id,
                expected_workstream_revision,
                value,
            } if !value.trim().is_empty() => Command::Rename {
                workstream_id,
                expected_workstream_revision,
                name: value,
            },
            modal @ Modal::Rename { .. } => {
                self.modal = Some(modal);
                Command::ShowGuidance("Rename requires a non-empty native thread name")
            }
        }
    }

    fn selected_workstream(&self, id: WorkstreamId) -> Option<&WorkstreamSnapshot> {
        self.snapshot
            .workstreams
            .iter()
            .find(|workstream| workstream.workstream_id == id)
    }

    fn selected_operation(&self, id: OperationId) -> Option<&OperationSnapshot> {
        self.snapshot
            .unresolved_operations
            .iter()
            .find(|operation| operation.operation_id == id)
    }
}

const ONBOARDING_IN_PROGRESS_GUIDANCE: &str =
    "Managed session onboarding is still in progress; wait for exact provider proof";
const ONBOARDING_RECOVERY_GUIDANCE: &str =
    "Managed session requires onboarding recovery; only Park is currently available";
const RENAME_UNAVAILABLE_GUIDANCE: &str = "The selected provider does not support navigator Rename";

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
    if page == Page::Workstreams {
        rows.extend(
            snapshot
                .unresolved_operations
                .iter()
                .copied()
                .map(Row::Operation),
        );
    }
    rows
}

fn render_model(frame: &mut Frame<'_>, area: Rect, model: &Model, terminal_focused: bool) {
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
        .map(|row| {
            let item = ListItem::new(row_lines(row, available_width));
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
        render_help(frame, content);
    } else if let Some(modal) = model.modal() {
        render_modal(frame, content, modal);
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

fn row_lines(row: &Row, available_width: u16) -> Vec<Line<'static>> {
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
            let runtime = match workstream.onboarding {
                Some(OnboardingStatus::ActionFenced) => "onboarding",
                Some(OnboardingStatus::RecoveryRequired) => "recovery",
                None if workstream.lifecycle == WorkstreamLifecycle::Parked => "parked",
                None if workstream.lifecycle == WorkstreamLifecycle::RecoveryRequired => "recovery",
                None => workstream
                    .runtime
                    .map_or("stopped", |runtime| runtime_status_label(runtime.status)),
            };
            let title = workstream
                .native_name
                .clone()
                .unwrap_or_else(|| workstream.workstream_id.short());
            let indicator = if workstream.onboarding == Some(OnboardingStatus::RecoveryRequired)
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
        Row::Operation(operation) => vec![
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    operation_kind_label(operation.kind),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    operation_phase_label(operation.phase),
                    Style::default().fg(Color::Red),
                ),
            ]),
            Line::from(vec![
                Span::raw("  ! "),
                Span::styled(
                    operation.operation_id.short(),
                    Style::default().fg(Color::White),
                ),
            ]),
        ],
    }
}

const fn operation_kind_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Onboard => "Onboarding",
        OperationKind::Start => "Start",
        OperationKind::Fork => "Fork",
    }
}

const fn operation_phase_label(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::Prepared => "prepared",
        OperationPhase::CapabilityIssued => "issued",
        OperationPhase::RuntimeOwnedLaunching | OperationPhase::ProviderExecStarted => "onboarding",
        OperationPhase::ProviderPreparation => "preparing",
        OperationPhase::ProviderExecProven => "proven",
        OperationPhase::ExternalEffectStarted
        | OperationPhase::ExecFailedKnownAbsent
        | OperationPhase::AwaitingReconciliation
        | OperationPhase::RecoveryRequired => "recovery",
        OperationPhase::RolledBack => "rolled back",
        OperationPhase::Committed => "committed",
        OperationPhase::Failed => "failed",
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

fn control_bindings(model: &Model) -> &'static [(&'static str, &'static str)] {
    if model.help_visible() {
        return &[("? / Esc", "close help")];
    }
    if let Some(modal) = model.modal() {
        return match modal {
            Modal::ConfirmArchive { .. } => &[("Enter / y", "archive"), ("n / Esc", "cancel")],
            Modal::Rename { .. } => &[("Enter", "rename"), ("Esc", "cancel")],
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
                    ("n", "new here"),
                    ("f", "fork"),
                    ("p", "park"),
                    ("x", "archive"),
                    ("r", "rename"),
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
                            && workstream.onboarding == Some(OnboardingStatus::RecoveryRequired)
                    }) =>
            {
                &[
                    ("p", "park"),
                    (".", "archived"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            }
            Some(RowId::Operation(_)) => &[
                ("r", "recover"),
                (".", "archived"),
                ("?", "help"),
                ("q", "quit"),
            ],
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

/// Full-width, single-row Help content for the Navigator pane. It describes
/// only current direct controls; contextual restrictions remain explicit
/// rather than implying every key is always available.
fn help_lines() -> Vec<Line<'static>> {
    vec![
        help_heading("Navigate"),
        help_binding("↑↓", "select"),
        help_binding("Enter", "open / shell"),
        help_binding(".", "archived"),
        help_binding("w / Esc", "workstreams"),
        help_heading("Sessions"),
        help_binding("n", "new at location"),
        help_binding("f", "fork session"),
        help_binding("p", "park session"),
        help_binding("x", "archive session"),
        help_binding("u", "restore session"),
        help_binding("a", "acknowledge result"),
        help_binding("r", "rename / recover Fork"),
        help_binding("?/Esc/q", "close help"),
    ]
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
/// column, making the full-width sheet easy to scan without a table border.
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

fn render_help(frame: &mut Frame<'_>, area: Rect) {
    let overlay = help_overlay(area);
    frame.render_widget(Clear, overlay);
    frame.render_widget(
        Paragraph::new(help_lines()).block(
            Block::default()
                .borders(Borders::TOP)
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

/// Gives Help every available Navigator column while fitting its top rule and
/// fixed rows exactly to content. A shorter terminal necessarily clips, but
/// no height is otherwise reserved or wasted.
fn help_overlay(area: Rect) -> Rect {
    let content_height = u16::try_from(help_lines().len().saturating_add(1)).unwrap_or(u16::MAX);
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
            "Archive this managed session? Its live Runtime will be parked first.\n\nEnter or y confirms; n or Esc cancels.".to_owned(),
        ),
        Modal::Rename { value, .. } => (
            " Rename Codex session ",
            format!("Native thread name:\n\n{value}\n\nEnter confirms; Esc cancels."),
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

fn controls_height(model: &Model, width: u16) -> u16 {
    u16::try_from(controls_lines(model, width).len())
        .unwrap_or(u16::MAX)
        .max(1)
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

    use ratatui::{layout::Rect, style::Color, widgets::Borders};

    use super::{
        Command, Modal, Model, Navigator, ObserverSetupKind, Page, Row, RowId, ShellLocation,
        workstreams_in_visual_order,
    };
    use crate::{
        domain::{
            LocationId, OperationId, OperationKind, OperationPhase, ProjectId, ProviderKind,
            Revision, RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
        },
        snapshot::{
            LocationSnapshot, OnboardingStatus, OperationSnapshot, ProjectSnapshot,
            RuntimeSnapshot, Snapshot, WorkstreamSnapshot,
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
                        revision: Revision::INITIAL,
                        runtime: None,
                        onboarding: None,
                        native_name: None,
                        attention_revision: Revision::INITIAL,
                        result_unseen: false,
                        recovery_unseen: false,
                    },
                    WorkstreamSnapshot {
                        project_id,
                        location_id,
                        workstream_id: archived,
                        provider: ProviderKind::OpenCode,
                        lifecycle: WorkstreamLifecycle::Open,
                        archived: true,
                        last_activity_sequence: 1,
                        revision: Revision::INITIAL,
                        runtime: None,
                        onboarding: None,
                        native_name: None,
                        attention_revision: Revision::INITIAL,
                        result_unseen: false,
                        recovery_unseen: false,
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
                    revision: Revision::INITIAL,
                    runtime: None,
                    onboarding: None,
                    native_name: None,
                    attention_revision: Revision::INITIAL,
                    result_unseen: false,
                    recovery_unseen: false,
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
        let shell = super::row_lines(&rows[0], 30);

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
        let narrow = super::row_lines(&rows[0], 1);
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
        for expected in ["n new here", ". archived", "q quit"] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}: {rendered}"
            );
        }
        assert!(!rendered.contains("↑↓ select"));
        assert!(!rendered.contains("Enter open"));
        assert!(!rendered.contains("a acknowledge"));
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

        for bindings in [
            super::control_bindings(&model),
            {
                model.select_next();
                super::control_bindings(&model)
            },
            {
                let _ = model.handle_key(crossterm::event::KeyCode::Char('.'));
                super::control_bindings(&model)
            },
        ] {
            assert!(!bindings.iter().any(|(key, _)| *key == "↑↓"));
            assert!(!bindings.iter().any(|(key, _)| *key == "a"));
            assert!(
                !bindings
                    .iter()
                    .any(|(key, action)| *key == "Enter" && matches!(*action, "open" | "shell"))
            );
        }
    }

    #[test]
    fn help_is_full_width_colored_and_fits_its_content() {
        let overlay = super::help_overlay(Rect::new(0, 0, 32, 24));
        assert_eq!(overlay, Rect::new(0, 4, 32, 15));
        assert_eq!(
            super::help_overlay(Rect::new(0, 0, 32, 18)),
            Rect::new(0, 1, 32, 15)
        );

        let lines = super::help_lines();
        assert_eq!(lines.len(), 14);
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
        assert_eq!(rendered[6], "n       new at location");
        assert_eq!(rendered[12], "r       rename / recover Fork");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(lines[6].spans[0].style.fg, Some(Color::Yellow));
        assert_eq!(lines[13].spans[0].style.fg, Some(Color::Yellow));
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
        let lines = super::row_lines(workstream, 30);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content == "onboarding")
        );
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
        let (mut snapshot, active, archived) = snapshot();
        snapshot.workstreams[0].result_unseen = true;
        snapshot.workstreams[0].attention_revision = Revision::INITIAL.next();
        let mut model = Model::new(snapshot);
        model.select_next();

        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('p')),
            Command::Park {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
            }
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('f')),
            Command::Fork {
                source_workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
                provider: ProviderKind::Codex,
            }
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('a')),
            Command::AcknowledgeResult {
                workstream_id: active,
                expected_attention_revision: Revision::INITIAL.next(),
            }
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('x')),
            Command::None
        );
        model.select_next();
        assert_eq!(model.selected(), Some(RowId::Workstream(active)));
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
            model.handle_key(crossterm::event::KeyCode::Char('u')),
            Command::Restore {
                workstream_id: archived,
                expected_workstream_revision: Revision::INITIAL,
            }
        );
    }

    #[test]
    fn recovery_operation_rename_help_and_onboarding_fences_are_explicit() {
        {
            let (mut snapshot, active, _) = snapshot();
            let operation_id = OperationId::from(Uuid::from_u128(7));
            snapshot.unresolved_operations = vec![OperationSnapshot {
                operation_id,
                kind: OperationKind::Fork,
                provider: ProviderKind::Codex,
                source_workstream_id: Some(active),
                phase: OperationPhase::RecoveryRequired,
                revision: Revision::INITIAL,
            }];
            let mut model = Model::new(snapshot);
            model.select_next();
            assert_eq!(
                model.handle_key(crossterm::event::KeyCode::Char('r')),
                Command::None
            );
            assert!(matches!(model.modal(), Some(Modal::Rename { .. })));
            let _ = model.handle_key(crossterm::event::KeyCode::Esc);
            model.select_next();
            assert_eq!(model.selected(), Some(RowId::Operation(operation_id)));
            assert_eq!(
                model.handle_key(crossterm::event::KeyCode::Char('r')),
                Command::RecoverOperation {
                    operation_id,
                    expected_operation_revision: Revision::INITIAL,
                    provider: ProviderKind::Codex,
                }
            );
            assert_eq!(
                model.handle_key(crossterm::event::KeyCode::Char('?')),
                Command::None
            );
            assert!(model.help_visible());
            model.select_previous();
            assert_eq!(model.selected(), Some(RowId::Operation(operation_id)));
            assert_eq!(
                model.handle_key(crossterm::event::KeyCode::Char('q')),
                Command::None
            );
            assert!(!model.help_visible());
        }

        let (mut snapshot, active, _) = snapshot();
        snapshot.workstreams[0].onboarding = Some(OnboardingStatus::RecoveryRequired);
        let mut model = Model::new(snapshot);
        model.select_next();
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('p')),
            Command::Park {
                workstream_id: active,
                expected_workstream_revision: Revision::INITIAL,
            }
        );
        assert_eq!(
            model.handle_key(crossterm::event::KeyCode::Char('f')),
            Command::ShowGuidance(super::ONBOARDING_RECOVERY_GUIDANCE)
        );
    }
}
