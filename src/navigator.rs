//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

use std::{
    io::{self, stdout},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use thiserror::Error;

use crate::{
    domain::{Revision, RuntimeStatus, WorkstreamId, WorkstreamLifecycle},
    presentation::{Presentation, PresentationError},
    provider::codex::names::{NameContext, resolve_name},
    runtime::{
        LinuxProcessProbe, PrivateRuntime, RuntimeError, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{ClientCatalog, HostIdentity, HostRegistry, StateError, StateRoot, WorkstreamOverview},
};

/// One bounded row rendered by the local navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorWorkstream {
    pub workstream_id: WorkstreamId,
    pub project_label: String,
    pub display_name: String,
    pub runtime_status: NavigatorRuntimeStatus,
    pub result_ready: bool,
    pub recovery_required: bool,
    pub attention_revision: Option<Revision>,
    pub workstream_revision: Revision,
}

/// Runtime information safe to expose in the navigator without process or
/// terminal details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigatorRuntimeStatus {
    Starting,
    Idle,
    Working,
    Attention,
    Parked,
    Unknown,
    RecoveryRequired,
}

impl NavigatorRuntimeStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle | Self::Attention => "idle",
            Self::Working => "working",
            Self::Parked => "parked",
            Self::Unknown => "unknown",
            Self::RecoveryRequired => "recovery required",
        }
    }
}

/// A complete bounded projection of the local host registry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalNavigatorSnapshot {
    pub workstreams: Vec<NavigatorWorkstream>,
}

/// Reads a fresh local-only navigator projection from the durable host state.
/// The caller controls polling; this function performs no provider I/O and no
/// mutation.
///
/// # Errors
///
/// Returns an error when the local registry cannot be opened or contains
/// invalid persisted state.
pub fn local_snapshot(root: &StateRoot) -> Result<LocalNavigatorSnapshot, NavigatorError> {
    let registry = HostRegistry::open(root)?;
    let host = registry.identity()?;
    let mut catalog = ClientCatalog::open(root)?;
    let executable = std::env::current_exe().map_err(NavigatorError::CurrentExecutable)?;
    let workstreams = registry
        .workstream_overviews()?
        .into_iter()
        .map(|overview| project_workstream(root, &mut catalog, &host, &executable, &overview))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LocalNavigatorSnapshot { workstreams })
}

fn project_workstream(
    root: &StateRoot,
    catalog: &mut ClientCatalog,
    host: &HostIdentity,
    executable: &Path,
    overview: &WorkstreamOverview,
) -> Result<NavigatorWorkstream, NavigatorError> {
    let recovery_required = overview.lifecycle == WorkstreamLifecycle::RecoveryRequired
        || overview
            .attention
            .as_ref()
            .and_then(|attention| attention.recovery_unseen_since_revision)
            .is_some();
    let result_ready = overview
        .attention
        .as_ref()
        .and_then(|attention| attention.result_unseen_since_revision)
        .is_some();
    let attention_revision = overview
        .attention
        .as_ref()
        .and_then(|attention| attention.result_unseen_since_revision)
        .or_else(|| {
            overview
                .attention
                .as_ref()
                .and_then(|attention| attention.recovery_unseen_since_revision)
        });
    let runtime_status = if recovery_required {
        NavigatorRuntimeStatus::RecoveryRequired
    } else {
        observed_runtime_status(root, overview)?
    };
    let display_name = bounded_display(&display_name(overview, runtime_status));
    let project_label = match catalog.local_project_location(host.host_id, overview.location_id)? {
        Some(project) => project.display_name,
        None => {
            catalog
                .register_local_project_location(
                    host,
                    overview.location_id,
                    executable,
                    &fallback_project_label(&overview.checkout_path),
                )?
                .display_name
        }
    };
    Ok(NavigatorWorkstream {
        workstream_id: overview.workstream_id,
        project_label: bounded_display(&project_label),
        display_name,
        runtime_status,
        result_ready,
        recovery_required,
        attention_revision,
        workstream_revision: overview.revision,
    })
}

fn observed_runtime_status(
    root: &StateRoot,
    overview: &WorkstreamOverview,
) -> Result<NavigatorRuntimeStatus, NavigatorError> {
    if overview.lifecycle == WorkstreamLifecycle::Parked {
        return Ok(NavigatorRuntimeStatus::Parked);
    }
    let Some(record) = &overview.runtime else {
        return Ok(NavigatorRuntimeStatus::Parked);
    };
    if record.status == RuntimeStatus::Stopped {
        return Ok(NavigatorRuntimeStatus::Unknown);
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), record.runtime_id),
    );
    match runtime.probe()? {
        RuntimeProbe::Live { .. } => Ok(navigator_runtime_status(record.status)),
        RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => Ok(NavigatorRuntimeStatus::Unknown),
    }
}

fn display_name(overview: &WorkstreamOverview, runtime_status: NavigatorRuntimeStatus) -> String {
    let Some(binding) = &overview.binding else {
        return if runtime_status == NavigatorRuntimeStatus::Starting {
            format!("starting · {}", overview.workstream_id.short())
        } else {
            format!("untitled · {}", overview.workstream_id.short())
        };
    };
    let context = if binding.start_source == "clear" {
        NameContext::Cutover {
            prior_effective_name: binding.predecessor_effective_name.as_deref(),
        }
    } else if runtime_status == NavigatorRuntimeStatus::Starting {
        NameContext::Starting
    } else {
        NameContext::Normal
    };
    resolve_name(
        binding.name_state,
        binding.observed_thread_name.as_deref(),
        binding.observed_thread_name.as_deref(),
        context,
        &overview.workstream_id.short(),
    )
    .text
}

fn fallback_project_label(checkout_path: &Path) -> String {
    checkout_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map_or_else(|| "local project".to_owned(), bounded_display)
}

fn bounded_display(value: &str) -> String {
    const MAX_DISPLAY_CHARS: usize = 64;
    let mut text = value.chars().take(MAX_DISPLAY_CHARS).collect::<String>();
    if value.chars().nth(MAX_DISPLAY_CHARS).is_some() {
        text.push('…');
    }
    text
}

const fn navigator_runtime_status(status: RuntimeStatus) -> NavigatorRuntimeStatus {
    match status {
        RuntimeStatus::Starting => NavigatorRuntimeStatus::Starting,
        RuntimeStatus::Idle => NavigatorRuntimeStatus::Idle,
        RuntimeStatus::Working => NavigatorRuntimeStatus::Working,
        RuntimeStatus::Attention => NavigatorRuntimeStatus::Attention,
        RuntimeStatus::Stopped => NavigatorRuntimeStatus::Parked,
        RuntimeStatus::Unknown | RuntimeStatus::Unreachable => NavigatorRuntimeStatus::Unknown,
    }
}

/// Pure navigator selection and rendering state.
#[derive(Clone, Debug, Default)]
pub struct NavigatorView {
    snapshot: LocalNavigatorSnapshot,
    selected: usize,
    message: Option<String>,
    spinner_frame: usize,
}

impl NavigatorView {
    #[must_use]
    pub fn new(snapshot: LocalNavigatorSnapshot) -> Self {
        Self {
            snapshot,
            selected: 0,
            message: None,
            spinner_frame: 0,
        }
    }

    pub fn replace_snapshot(&mut self, snapshot: LocalNavigatorSnapshot) {
        let selected_id = self.selected().map(|row| row.workstream_id);
        self.snapshot = snapshot;
        self.selected = selected_id
            .and_then(|workstream_id| {
                self.snapshot
                    .workstreams
                    .iter()
                    .position(|row| row.workstream_id == workstream_id)
            })
            .unwrap_or_else(|| {
                self.selected
                    .min(self.snapshot.workstreams.len().saturating_sub(1))
            });
    }

    #[must_use]
    pub fn selected(&self) -> Option<&NavigatorWorkstream> {
        self.snapshot.workstreams.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if !self.snapshot.workstreams.is_empty() {
            self.selected = (self.selected + 1) % self.snapshot.workstreams.len();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.snapshot.workstreams.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.snapshot.workstreams.len() - 1);
        }
    }

    pub fn select_row(&mut self, row: usize) {
        if row < self.snapshot.workstreams.len() {
            self.selected = row;
        }
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(bounded_display(&message.into()));
    }

    pub fn clear_message(&mut self) {
        self.message = None;
    }

    fn advance_animation(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }

    pub fn render(&self, frame: &mut Frame<'_>) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(frame.area());
        let items = self
            .snapshot
            .workstreams
            .iter()
            .enumerate()
            .map(|(index, row)| row_item(row, index == self.selected, self.spinner_frame))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select((!items.is_empty()).then_some(self.selected));
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Workstreams "),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
            areas[0],
            &mut state,
        );
        let help = self
            .message
            .as_deref()
            .unwrap_or("↑↓ select  Enter attach/start  a acknowledge  p park  q close");
        frame.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
            areas[1],
        );
    }

    #[must_use]
    pub fn row_from_y(&self, y: u16) -> Option<usize> {
        // List content starts immediately after the one-line top border. Each
        // Workstream intentionally has a project line and a native-thread line.
        y.checked_sub(1)
            .map(|offset| usize::from(offset / 2))
            .filter(|row| *row < self.snapshot.workstreams.len())
    }
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn row_item(row: &NavigatorWorkstream, selected: bool, spinner_frame: usize) -> ListItem<'static> {
    let (indicator, indicator_style) = status_indicator(row, spinner_frame);
    let project_style = if selected {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    };
    let thread_style = if selected {
        Style::default().fg(Color::White)
    } else {
        Style::default()
    };
    ListItem::new(vec![
        Line::from(vec![
            Span::raw("   "),
            Span::styled(row.project_label.clone(), project_style),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(indicator, indicator_style),
            Span::raw(" "),
            Span::styled(row.display_name.clone(), thread_style),
        ]),
    ])
}

/// Returns a compact user-facing state from bounded lifecycle and attention
/// metadata. Ordinary idle is deliberately unmarked; active and completed
/// work stand out without consuming thread-title space.
fn status_indicator(row: &NavigatorWorkstream, spinner_frame: usize) -> (&'static str, Style) {
    match row.runtime_status {
        NavigatorRuntimeStatus::RecoveryRequired => ("!", Style::default().fg(Color::Red)),
        NavigatorRuntimeStatus::Working => (
            SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()],
            Style::default().fg(Color::Yellow),
        ),
        NavigatorRuntimeStatus::Unknown => ("?", Style::default().fg(Color::Red)),
        NavigatorRuntimeStatus::Parked => ("‖", Style::default().fg(Color::DarkGray)),
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

/// Local navigator projection failures.
#[derive(Debug, Error)]
pub enum NavigatorError {
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Presentation(#[from] PresentationError),
    #[error("could not initialize the local terminal navigator: {0}")]
    Terminal(#[from] io::Error),
    #[error("the local navigator action could not be launched")]
    ActionLaunch(io::Error),
    #[error("the current wsnav executable cannot be resolved")]
    CurrentExecutable(io::Error),
    #[error("the local navigator action produced oversized diagnostics")]
    ActionOutputTooLarge,
    #[error("the local navigator action failed")]
    ActionFailed,
}

/// Runs the internal Ratatui process inside one owned presentation pane.
/// The presentation owner supplies the only private tmux socket this process
/// may mutate.
///
/// # Errors
///
/// Returns an error when the local terminal cannot be initialized, the private
/// presentation control path is invalid, or bounded local state/action calls
/// fail.
pub fn run_local_navigator(
    root: &StateRoot,
    socket: PathBuf,
    session_name: String,
) -> Result<(), NavigatorError> {
    let presentation = Presentation::from_control(root.base(), socket, session_name)?;
    let snapshot = local_snapshot(root)?;
    let mut view = NavigatorView::new(snapshot);
    let mut terminal = TerminalSession::enter()?;
    let mut last_refresh = Instant::now();
    let mut last_animation = Instant::now();
    let outcome: Result<(), NavigatorError> = loop {
        terminal.terminal.draw(|frame| view.render(frame))?;
        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Down | KeyCode::Char('j') => view.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => view.select_previous(),
                    KeyCode::Tab => {
                        if let Err(error) = presentation.focus_provider() {
                            view.set_message(action_message(&error));
                        }
                    }
                    KeyCode::Enter => activate_selected(root, &presentation, &mut view),
                    KeyCode::Char('a') => acknowledge_selected(root, &mut view),
                    KeyCode::Char('p') => park_selected(root, &mut view),
                    _ => {}
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => view.select_next(),
                    MouseEventKind::ScrollUp => view.select_previous(),
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(row) = view.row_from_y(mouse.row) {
                            view.select_row(row);
                            activate_selected(root, &presentation, &mut view);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            match local_snapshot(root) {
                Ok(snapshot) => view.replace_snapshot(snapshot),
                Err(error) => view.set_message(action_message(&error)),
            }
            last_refresh = Instant::now();
        }
        if last_animation.elapsed() >= Duration::from_millis(100) {
            view.advance_animation();
            last_animation = Instant::now();
        }
    };
    drop(terminal);
    let close = presentation.close();
    outcome?;
    close?;
    Ok(())
}

fn activate_selected(root: &StateRoot, presentation: &Presentation, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no Workstream is registered; use wsnav register first");
        return;
    };
    if matches!(
        selected.runtime_status,
        NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown
    ) && let Err(error) = run_action(root, "start", selected.workstream_id, None)
    {
        view.set_message(action_message(&error));
        return;
    }
    refresh_view(root, view);
    if let Err(error) = presentation.attach_workstream(selected.workstream_id) {
        view.set_message(action_message(&error));
        return;
    }
    if let Err(error) = presentation.focus_provider() {
        view.set_message(action_message(&error));
    } else {
        view.set_message("provider attached; use the native Codex UI directly");
    }
}

fn acknowledge_selected(root: &StateRoot, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        return;
    };
    let Some(revision) = selected.attention_revision else {
        view.set_message("no result or recovery attention to acknowledge");
        return;
    };
    match run_action(
        root,
        "acknowledge",
        selected.workstream_id,
        Some(revision.value()),
    ) {
        Ok(()) => {
            refresh_view(root, view);
            view.set_message("attention acknowledged");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn park_selected(root: &StateRoot, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        return;
    };
    match run_action(root, "park", selected.workstream_id, None) {
        Ok(()) => {
            refresh_view(root, view);
            view.set_message("Workstream parked; provider history is preserved");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn refresh_view(root: &StateRoot, view: &mut NavigatorView) {
    match local_snapshot(root) {
        Ok(snapshot) => view.replace_snapshot(snapshot),
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn run_action(
    root: &StateRoot,
    action: &str,
    workstream_id: WorkstreamId,
    revision: Option<i64>,
) -> Result<(), NavigatorError> {
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command
        .arg("--state-root")
        .arg(root.base())
        .arg(action)
        .arg(workstream_id.to_string());
    if let Some(revision) = revision {
        command.arg(revision.to_string());
    }
    let output = command.output().map_err(NavigatorError::ActionLaunch)?;
    if output.stdout.len() > 1024 || output.stderr.len() > 1024 {
        return Err(NavigatorError::ActionOutputTooLarge);
    }
    if output.status.success() {
        Ok(())
    } else {
        Err(NavigatorError::ActionFailed)
    }
}

fn action_message(error: &impl std::fmt::Display) -> String {
    bounded_display(&error.to_string())
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn enter() -> Result<Self, io::Error> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
                Err(error)
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn selection_wraps_without_persisting_focus() {
        let first = WorkstreamId::new();
        let second = WorkstreamId::new();
        let snapshot = LocalNavigatorSnapshot {
            workstreams: vec![
                row(first, NavigatorRuntimeStatus::Idle),
                row(second, NavigatorRuntimeStatus::Parked),
            ],
        };
        let mut view = NavigatorView::new(snapshot);
        view.select_previous();
        assert_eq!(view.selected().map(|row| row.workstream_id), Some(second));
        view.select_next();
        assert_eq!(view.selected().map(|row| row.workstream_id), Some(first));
    }

    #[test]
    fn row_mapping_covers_both_lines_of_each_workstream() {
        let view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![
                row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle),
                row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked),
            ],
        });
        assert_eq!(view.row_from_y(0), None);
        assert_eq!(view.row_from_y(1), Some(0));
        assert_eq!(view.row_from_y(2), Some(0));
        assert_eq!(view.row_from_y(3), Some(1));
        assert_eq!(view.row_from_y(4), Some(1));
        assert_eq!(view.row_from_y(5), None);
    }

    #[test]
    fn project_labels_do_not_expose_full_paths() {
        assert_eq!(
            fallback_project_label(Path::new("/private/place/project")),
            "project"
        );
    }

    #[test]
    fn renderer_shows_done_indicator_without_provider_content() {
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        let view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![NavigatorWorkstream {
                project_label: "project".to_owned(),
                display_name: "native thread".to_owned(),
                result_ready: true,
                ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
            }],
        });
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
        let project_cell = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .find(|cell| cell.symbol() == "p")
            .unwrap();
        assert_eq!(project_cell.fg, Color::White);
    }

    #[test]
    fn working_state_wins_over_a_prior_unacknowledged_result() {
        let row = NavigatorWorkstream {
            result_ready: true,
            ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Working)
        };

        assert_eq!(status_indicator(&row, 0).0, SPINNER_FRAMES[0]);
    }

    #[test]
    fn acknowledged_attention_returns_to_an_empty_idle_indicator() {
        let row = row(WorkstreamId::new(), NavigatorRuntimeStatus::Attention);

        assert_eq!(status_indicator(&row, 0).0, " ");
    }

    #[test]
    fn working_indicator_advances_between_spinner_frames() {
        let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());

        view.advance_animation();

        assert_eq!(view.spinner_frame, 1);
    }

    fn row(
        workstream_id: WorkstreamId,
        runtime_status: NavigatorRuntimeStatus,
    ) -> NavigatorWorkstream {
        NavigatorWorkstream {
            workstream_id,
            project_label: "project".to_owned(),
            display_name: "thread".to_owned(),
            runtime_status,
            result_ready: false,
            recovery_required: false,
            attention_revision: None,
            workstream_revision: Revision::INITIAL,
        }
    }
}
