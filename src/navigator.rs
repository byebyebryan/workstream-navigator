//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, stdout},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::mpsc::{self, Receiver, Sender},
    thread,
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
    domain::{Clock, Revision, RuntimeStatus, SystemClock, WorkstreamId, WorkstreamLifecycle},
    presentation::{Presentation, PresentationError},
    provider::codex::names::{NameContext, resolve_name},
    runtime::{
        LinuxProcessProbe, PrivateRuntime, RuntimeError, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{
        ClientCatalog, ClientHost, ClientHostTransport, HostIdentity, HostRegistry, StateError,
        StateRoot, WorkstreamOverview,
    },
    transport::{HostClient, RemoteExecutable, SshDestination, SshEndpoint, SystemCommandRunner},
};

/// One bounded row rendered by the local navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorWorkstream {
    pub host: NavigatorHost,
    pub workstream_id: WorkstreamId,
    pub project_label: String,
    pub display_name: String,
    pub runtime_status: NavigatorRuntimeStatus,
    pub result_ready: bool,
    pub recovery_required: bool,
    pub attention_revision: Option<Revision>,
    pub last_activity_at_millis: Option<i64>,
    pub workstream_revision: Revision,
}

/// Presentation-only host location for a Workstream row. The reachability
/// value is a cached transport observation, never a provider lifecycle claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NavigatorHost {
    Local,
    Remote {
        alias: String,
        reachability: RemoteHostReachability,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteHostReachability {
    Reachable,
    Unreachable,
}

impl NavigatorHost {
    fn alias(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::Remote { alias, .. } => alias,
        }
    }

    const fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    const fn is_reachable(&self) -> bool {
        matches!(
            self,
            Self::Local
                | Self::Remote {
                    reachability: RemoteHostReachability::Reachable,
                    ..
                }
        )
    }
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
    pub unreachable_hosts: Vec<String>,
    pub unresolved_operation_count: usize,
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
    let mut registry = HostRegistry::open(root)?;
    crate::actions::reconcile_lost_runtimes(root, &mut registry)?;
    let host = registry.identity()?;
    let mut catalog = ClientCatalog::open(root)?;
    let executable = std::env::current_exe().map_err(NavigatorError::CurrentExecutable)?;
    let workstreams = registry
        .workstream_overviews()?
        .into_iter()
        .map(|overview| project_workstream(root, &mut catalog, &host, &executable, &overview))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LocalNavigatorSnapshot {
        workstreams,
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: registry.unresolved_operation_overviews()?.len(),
    })
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
        host: NavigatorHost::Local,
        workstream_id: overview.workstream_id,
        project_label: bounded_display(&project_label),
        display_name,
        runtime_status,
        result_ready,
        recovery_required,
        attention_revision,
        last_activity_at_millis: overview.last_activity_at_millis,
        workstream_revision: overview.revision,
    })
}

fn project_remote_workstream(
    host_alias: &str,
    workstream: &crate::protocol::SnapshotWorkstream,
    host_reachable: bool,
) -> Result<NavigatorWorkstream, NavigatorError> {
    let attention_revision = workstream
        .attention_revision
        .map(Revision::try_from)
        .transpose()
        .map_err(NavigatorError::InvalidRemoteSnapshot)?;
    let workstream_revision =
        Revision::try_from(workstream.revision).map_err(NavigatorError::InvalidRemoteSnapshot)?;
    let runtime_status = if workstream.recovery_required {
        NavigatorRuntimeStatus::RecoveryRequired
    } else {
        navigator_runtime_status(workstream.runtime_status)
    };
    Ok(NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: host_alias.to_owned(),
            reachability: if host_reachable {
                RemoteHostReachability::Reachable
            } else {
                RemoteHostReachability::Unreachable
            },
        },
        workstream_id: workstream.workstream_id,
        project_label: bounded_display(&workstream.project_display_name),
        display_name: bounded_display(&workstream.display_name),
        runtime_status,
        result_ready: workstream.result_ready,
        recovery_required: workstream.recovery_required,
        attention_revision,
        last_activity_at_millis: workstream.last_activity_at_millis,
        workstream_revision,
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
            "starting".to_owned()
        } else {
            "untitled".to_owned()
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

const REMOTE_POLL_INTERVAL: Duration = Duration::from_secs(3);
const REMOTE_FOCUSED_POLL_INTERVAL: Duration = Duration::from_millis(750);
const REMOTE_INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const REMOTE_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Non-durable client presentation state for bounded asynchronous SSH refresh.
/// It retains the last accepted snapshot if a host becomes unavailable; an SSH
/// disconnect is never projected as a provider stop or attention clear.
struct RemoteMonitor {
    sender: Sender<RemotePollResult>,
    receiver: Receiver<RemotePollResult>,
    hosts: BTreeMap<String, CachedRemoteHost>,
}

struct CachedRemoteHost {
    workstreams: Vec<NavigatorWorkstream>,
    unresolved_operation_count: usize,
    reachable: bool,
    pending: bool,
    next_poll: Instant,
    backoff: Duration,
}

struct RemotePollResult {
    alias: String,
    outcome: Result<crate::protocol::SnapshotResponse, ()>,
}

impl RemoteMonitor {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            hosts: BTreeMap::new(),
        }
    }

    fn refresh(
        &mut self,
        catalog: &ClientCatalog,
        selected_host: Option<&str>,
    ) -> Result<(), NavigatorError> {
        let now = Instant::now();
        self.collect(now);
        let registered = catalog.ssh_hosts()?;
        let aliases = registered
            .iter()
            .map(|host| host.alias.clone())
            .collect::<BTreeSet<_>>();
        self.hosts.retain(|alias, _| aliases.contains(alias));
        for host in registered {
            let entry = self
                .hosts
                .entry(host.alias.clone())
                .or_insert_with(|| CachedRemoteHost {
                    workstreams: Vec::new(),
                    unresolved_operation_count: 0,
                    reachable: false,
                    pending: false,
                    next_poll: now,
                    backoff: REMOTE_INITIAL_BACKOFF,
                });
            if entry.reachable
                && selected_host.is_some_and(|selected| selected == host.alias)
                && entry.next_poll > now + REMOTE_FOCUSED_POLL_INTERVAL
            {
                entry.next_poll = now + REMOTE_FOCUSED_POLL_INTERVAL;
            }
            if entry.pending || entry.next_poll > now {
                continue;
            }
            entry.pending = true;
            let sender = self.sender.clone();
            thread::spawn(move || {
                let outcome = fetch_remote_snapshot(&host);
                let _ = sender.send(RemotePollResult {
                    alias: host.alias,
                    outcome,
                });
            });
        }
        Ok(())
    }

    fn collect(&mut self, now: Instant) {
        while let Ok(result) = self.receiver.try_recv() {
            let Some(host) = self.hosts.get_mut(&result.alias) else {
                continue;
            };
            host.pending = false;
            if let Ok(snapshot) = result.outcome {
                host.workstreams = snapshot
                    .workstreams
                    .iter()
                    .filter_map(|workstream| {
                        project_remote_workstream(&result.alias, workstream, true).ok()
                    })
                    .collect();
                host.unresolved_operation_count = usize::from(snapshot.unresolved_operation_count);
                host.reachable = true;
                host.backoff = REMOTE_INITIAL_BACKOFF;
                host.next_poll = now + REMOTE_POLL_INTERVAL;
            } else {
                host.reachable = false;
                host.next_poll = now + host.backoff;
                host.backoff = host.backoff.saturating_mul(2).min(REMOTE_MAX_BACKOFF);
            }
        }
    }

    fn combine(&self, mut local: LocalNavigatorSnapshot) -> LocalNavigatorSnapshot {
        for (alias, host) in &self.hosts {
            local
                .workstreams
                .extend(host.workstreams.iter().cloned().map(|mut workstream| {
                    workstream.host = NavigatorHost::Remote {
                        alias: alias.clone(),
                        reachability: if host.reachable {
                            RemoteHostReachability::Reachable
                        } else {
                            RemoteHostReachability::Unreachable
                        },
                    };
                    workstream
                }));
            if !host.reachable {
                local.unreachable_hosts.push(alias.clone());
            }
            local.unresolved_operation_count += host.unresolved_operation_count;
        }
        local
    }

    fn request_soon(&mut self, host_alias: &str) {
        if let Some(host) = self.hosts.get_mut(host_alias) {
            host.next_poll = Instant::now();
        }
    }
}

fn fetch_remote_snapshot(host: &ClientHost) -> Result<crate::protocol::SnapshotResponse, ()> {
    let ClientHostTransport::Ssh { destination } = &host.transport else {
        return Err(());
    };
    let destination = SshDestination::parse(destination).map_err(|_| ())?;
    let executable = host
        .executable_path
        .to_str()
        .ok_or(())
        .and_then(|value| RemoteExecutable::parse(value).map_err(|_| ()))?;
    let endpoint = SshEndpoint::new(destination, executable);
    let client = HostClient::new(SystemCommandRunner);
    client
        .probe_ssh(&endpoint)
        .map_err(|_| ())?
        .ensure_compatible_with_local()
        .map_err(|_| ())?;
    let hello = client.hello_ssh(&endpoint, "wsnav").map_err(|_| ())?;
    host.verify_hello(&hello).map_err(|_| ())?;
    client.snapshot_ssh(&endpoint).map_err(|_| ())
}

fn combined_snapshot(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    selected_host: Option<&str>,
) -> Result<LocalNavigatorSnapshot, NavigatorError> {
    let local = local_snapshot(root)?;
    let catalog = ClientCatalog::open(root)?;
    remote.refresh(&catalog, selected_host)?;
    Ok(remote.combine(local))
}

/// Pure navigator selection and rendering state.
#[derive(Clone, Debug, Default)]
pub struct NavigatorView {
    snapshot: LocalNavigatorSnapshot,
    selected: usize,
    attached: Option<(String, WorkstreamId)>,
    mouse_click: Option<MouseClickIntent>,
    message: Option<String>,
    spinner_frame: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseClickIntent {
    Blank,
    Row,
}

impl NavigatorView {
    #[must_use]
    pub fn new(snapshot: LocalNavigatorSnapshot) -> Self {
        Self {
            snapshot,
            selected: 0,
            attached: None,
            mouse_click: None,
            message: None,
            spinner_frame: 0,
        }
    }

    pub fn replace_snapshot(&mut self, snapshot: LocalNavigatorSnapshot) {
        let selected_id = self
            .selected()
            .map(|row| (row.host.alias().to_owned(), row.workstream_id));
        self.snapshot = snapshot;
        self.selected = selected_id
            .and_then(|(host_alias, workstream_id)| {
                self.snapshot.workstreams.iter().position(|row| {
                    row.host.alias() == host_alias && row.workstream_id == workstream_id
                })
            })
            .unwrap_or_else(|| {
                self.selected
                    .min(self.snapshot.workstreams.len().saturating_sub(1))
            });
        self.clear_inactive_attachment();
    }

    #[must_use]
    pub fn selected(&self) -> Option<&NavigatorWorkstream> {
        self.snapshot.workstreams.get(self.selected)
    }

    fn selected_host_alias(&self) -> Option<&str> {
        self.selected().map(|row| row.host.alias())
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

    fn select_workstream(&mut self, host_alias: &str, workstream_id: WorkstreamId) -> bool {
        let Some(selected) =
            self.snapshot.workstreams.iter().position(|row| {
                row.host.alias() == host_alias && row.workstream_id == workstream_id
            })
        else {
            return false;
        };
        self.selected = selected;
        true
    }

    fn is_attached_to(&self, workstream: &NavigatorWorkstream) -> bool {
        self.attached
            .as_ref()
            .is_some_and(|(host_alias, workstream_id)| {
                host_alias == workstream.host.alias() && *workstream_id == workstream.workstream_id
            })
    }

    fn mark_attached(&mut self, workstream: &NavigatorWorkstream) {
        self.attached = Some((workstream.host.alias().to_owned(), workstream.workstream_id));
    }

    fn clear_attached(&mut self, workstream: &NavigatorWorkstream) {
        if self.is_attached_to(workstream) {
            self.attached = None;
        }
    }

    fn clear_inactive_attachment(&mut self) {
        let Some((host_alias, workstream_id)) = &self.attached else {
            return;
        };
        let still_live = self.snapshot.workstreams.iter().any(|workstream| {
            workstream.host.alias() == host_alias
                && workstream.workstream_id == *workstream_id
                && !matches!(
                    workstream.runtime_status,
                    NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown
                )
        });
        if !still_live {
            self.attached = None;
        }
    }

    fn begin_mouse_click(&mut self, row: Option<usize>) {
        self.mouse_click = Some(if row.is_some() {
            MouseClickIntent::Row
        } else {
            MouseClickIntent::Blank
        });
        if let Some(row) = row {
            self.select_row(row);
        }
    }

    fn take_mouse_click(&mut self) -> Option<MouseClickIntent> {
        self.mouse_click.take()
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
        let help = self.message.clone().unwrap_or_else(|| {
            let operation_hint = (self.snapshot.unresolved_operation_count > 0).then(|| {
                format!(
                    "  ! {} operation{} needs recovery; use wsnav operations",
                    self.snapshot.unresolved_operation_count,
                    if self.snapshot.unresolved_operation_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                )
            });
            if self.snapshot.unreachable_hosts.is_empty() {
                format!(
                    "↑↓ select  Enter open/start/recover  n new  f fork  a acknowledge  p park  q close{}",
                    operation_hint.unwrap_or_default()
                )
            } else {
                format!(
                    "{} unavailable; showing cached state  ↑↓ select  Enter open/start/recover  n new  f fork  q close{}",
                    self.snapshot.unreachable_hosts.join(", "),
                    operation_hint.unwrap_or_default(),
                )
            }
        });
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

fn row_item(row: &NavigatorWorkstream, _selected: bool, spinner_frame: usize) -> ListItem<'static> {
    let (indicator, indicator_style) = status_indicator(row, spinner_frame);
    let project_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let thread_style = Style::default().fg(Color::White);
    ListItem::new(vec![
        Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("{} · ", row.host.alias()),
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(row.project_label.clone(), project_style),
        ]),
        Line::from(thread_line(row, indicator, indicator_style, thread_style)),
    ])
}

fn thread_line(
    row: &NavigatorWorkstream,
    indicator: &'static str,
    indicator_style: Style,
    thread_style: Style,
) -> Vec<Span<'static>> {
    let mut line = vec![
        Span::raw(" "),
        Span::styled(indicator, indicator_style),
        Span::raw(" "),
        Span::styled(row.display_name.clone(), thread_style),
    ];
    line.push(Span::styled(" · ", Style::default().fg(Color::Gray)));
    line.push(Span::styled(
        activity_label(row.last_activity_at_millis, SystemClock.now_millis().ok()),
        Style::default().fg(Color::Gray),
    ));
    line
}

fn activity_label(last_activity_at_millis: Option<i64>, now_millis: Option<i64>) -> String {
    relative_activity_age(last_activity_at_millis, now_millis)
        .unwrap_or_else(|| "activity unknown".to_owned())
}

fn relative_activity_age(
    last_activity_at_millis: Option<i64>,
    now_millis: Option<i64>,
) -> Option<String> {
    let elapsed_seconds = now_millis?
        .saturating_sub(last_activity_at_millis?)
        .max(0)
        .saturating_div(1_000);
    Some(match elapsed_seconds {
        0..=59 => "now".to_owned(),
        60..=3_599 => format!("{}m ago", elapsed_seconds / 60),
        3_600..=86_399 => format!("{}h ago", elapsed_seconds / 3_600),
        _ => format!("{}d ago", elapsed_seconds / 86_400),
    })
}

/// Returns a compact user-facing state from bounded lifecycle and attention
/// metadata. Ordinary idle is deliberately unmarked; active and completed
/// work stand out without consuming thread-title space.
fn status_indicator(row: &NavigatorWorkstream, spinner_frame: usize) -> (&'static str, Style) {
    if !row.host.is_reachable() {
        return ("?", Style::default().fg(Color::Red));
    }
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
    #[error("the local navigator action did not return one Workstream ID")]
    InvalidActionResult,
    #[error("remote host is unavailable")]
    RemoteHostUnavailable,
    #[error("remote host returned an invalid bounded snapshot")]
    InvalidRemoteSnapshot(#[source] crate::domain::DomainError),
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
    let mut remote = RemoteMonitor::new();
    let snapshot = combined_snapshot(root, &mut remote, None)?;
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
                    KeyCode::Enter => {
                        activate_selected(root, &presentation, &mut remote, &mut view);
                    }
                    KeyCode::Char('a') => acknowledge_selected(root, &mut remote, &mut view),
                    KeyCode::Char('p') => park_selected(root, &mut remote, &mut view),
                    KeyCode::Char('n') => {
                        create_workstream_selected(
                            root,
                            &presentation,
                            &mut remote,
                            &mut view,
                            CreationAction::Independent,
                        );
                    }
                    KeyCode::Char('f') => {
                        create_workstream_selected(
                            root,
                            &presentation,
                            &mut remote,
                            &mut view,
                            CreationAction::Fork,
                        );
                    }
                    _ => {}
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => view.select_next(),
                    MouseEventKind::ScrollUp => view.select_previous(),
                    MouseEventKind::Down(MouseButton::Left) => {
                        view.begin_mouse_click(view.row_from_y(mouse.row));
                    }
                    MouseEventKind::Up(MouseButton::Left) => match view.take_mouse_click() {
                        Some(MouseClickIntent::Row) => {
                            activate_selected(root, &presentation, &mut remote, &mut view);
                        }
                        Some(MouseClickIntent::Blank) => {
                            if let Err(error) = presentation.focus_navigator() {
                                view.set_message(action_message(&error));
                            }
                        }
                        None => {}
                    },
                    _ => {}
                },
                _ => {}
            }
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            let selected_host = view.selected_host_alias().map(str::to_owned);
            match combined_snapshot(root, &mut remote, selected_host.as_deref()) {
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

fn activate_selected(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no Workstream is registered; use wsnav register first");
        return;
    };
    if selected.host.is_remote() && !selected.host.is_reachable() {
        view.set_message("remote host is unavailable; cached state is not actionable");
        return;
    }
    if view.is_attached_to(&selected)
        && !matches!(
            selected.runtime_status,
            NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown
        )
    {
        if let Err(error) = presentation.focus_provider() {
            view.set_message(action_message(&error));
        }
        return;
    }
    let lifecycle_action = match selected.runtime_status {
        NavigatorRuntimeStatus::Parked | NavigatorRuntimeStatus::Unknown => Some("start"),
        NavigatorRuntimeStatus::RecoveryRequired => Some("recover"),
        NavigatorRuntimeStatus::Starting
        | NavigatorRuntimeStatus::Idle
        | NavigatorRuntimeStatus::Working
        | NavigatorRuntimeStatus::Attention => None,
    };
    if let Some(action) = lifecycle_action
        && let Err(error) = run_action(root, action, &selected, None)
    {
        view.set_message(action_message(&error));
        return;
    }
    remote.request_soon(selected.host.alias());
    refresh_view(root, remote, view);
    let attachment = if selected.host.is_remote() {
        presentation.attach_remote_workstream(selected.host.alias(), selected.workstream_id)
    } else {
        presentation.attach_workstream(selected.workstream_id)
    };
    if let Err(error) = attachment {
        view.set_message(action_message(&error));
        return;
    }
    view.mark_attached(&selected);
    if let Err(error) = presentation.focus_provider() {
        view.set_message(action_message(&error));
    } else {
        view.set_message("provider attached; use the native Codex UI directly");
    }
}

fn acknowledge_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        return;
    };
    let Some(revision) = selected.attention_revision else {
        view.set_message("no result or recovery attention to acknowledge");
        return;
    };
    match run_action(root, "acknowledge", &selected, Some(revision.value())) {
        Ok(()) => {
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("attention acknowledged");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn park_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        return;
    };
    match run_action(root, "park", &selected, None) {
        Ok(()) => {
            view.clear_attached(&selected);
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("Workstream parked; provider history is preserved");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreationAction {
    Independent,
    Fork,
}

impl CreationAction {
    const fn local_command(self) -> &'static str {
        match self {
            Self::Independent => "new-workstream",
            Self::Fork => "fork-workstream",
        }
    }

    const fn remote_command(self) -> &'static str {
        match self {
            Self::Independent => "new",
            Self::Fork => "fork",
        }
    }

    const fn success_message(self) -> &'static str {
        match self {
            Self::Independent => "new Workstream started; use the native Codex UI directly",
            Self::Fork => "forked Workstream started at the last completed native turn",
        }
    }
}

fn create_workstream_selected(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    action: CreationAction,
) {
    let Some(source) = view.selected().cloned() else {
        view.set_message("no Workstream is registered; use wsnav register first");
        return;
    };
    let destination = match run_creation_action(root, action, &source) {
        Ok(workstream_id) => workstream_id,
        Err(error) => {
            view.set_message(action_message(&error));
            return;
        }
    };
    remote.request_soon(source.host.alias());
    refresh_view(root, remote, view);
    if view.select_workstream(source.host.alias(), destination) {
        activate_selected(root, presentation, remote, view);
        return;
    }
    // A remote poll is asynchronous. Its control action has already created
    // and started the exact target, so attach it directly instead of making
    // the user repeat an action while waiting for the next bounded snapshot.
    let attachment = if source.host.is_remote() {
        presentation.attach_remote_workstream(source.host.alias(), destination)
    } else {
        presentation.attach_workstream(destination)
    };
    match attachment.and_then(|()| presentation.focus_provider()) {
        Ok(()) => view.set_message(action.success_message()),
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn refresh_view(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let selected_host = view.selected_host_alias().map(str::to_owned);
    match combined_snapshot(root, remote, selected_host.as_deref()) {
        Ok(snapshot) => view.replace_snapshot(snapshot),
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn run_action(
    root: &StateRoot,
    action: &str,
    workstream: &NavigatorWorkstream,
    revision: Option<i64>,
) -> Result<(), NavigatorError> {
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if workstream.host.is_remote() {
        if !workstream.host.is_reachable() {
            return Err(NavigatorError::RemoteHostUnavailable);
        }
        command
            .arg("host")
            .arg(action)
            .arg(workstream.host.alias())
            .arg(workstream.workstream_id.to_string());
        let revision = revision.unwrap_or_else(|| workstream.workstream_revision.value());
        command.arg(revision.to_string());
    } else {
        command
            .arg(action)
            .arg(workstream.workstream_id.to_string());
        if let Some(revision) = revision {
            command.arg(revision.to_string());
        }
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

fn run_creation_action(
    root: &StateRoot,
    action: CreationAction,
    source: &NavigatorWorkstream,
) -> Result<WorkstreamId, NavigatorError> {
    if source.host.is_remote() && !source.host.is_reachable() {
        return Err(NavigatorError::RemoteHostUnavailable);
    }
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if source.host.is_remote() {
        command
            .arg("host")
            .arg(action.remote_command())
            .arg(source.host.alias())
            .arg(source.workstream_id.to_string())
            .arg(source.workstream_revision.value().to_string());
    } else {
        command
            .arg(action.local_command())
            .arg(source.workstream_id.to_string());
    }
    let output = command.output().map_err(NavigatorError::ActionLaunch)?;
    if output.stdout.len() > 1024 || output.stderr.len() > 1024 {
        return Err(NavigatorError::ActionOutputTooLarge);
    }
    if !output.status.success() {
        return Err(NavigatorError::ActionFailed);
    }
    parse_created_workstream(&output.stdout)
}

fn parse_created_workstream(output: &[u8]) -> Result<WorkstreamId, NavigatorError> {
    let output = std::str::from_utf8(output).map_err(|_| NavigatorError::InvalidActionResult)?;
    let Some(identifier) = output.split_whitespace().last() else {
        return Err(NavigatorError::InvalidActionResult);
    };
    WorkstreamId::from_str(identifier).map_err(|_| NavigatorError::InvalidActionResult)
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        };
        let mut view = NavigatorView::new(snapshot);
        view.select_previous();
        assert_eq!(view.selected().map(|row| row.workstream_id), Some(second));
        view.select_next();
        assert_eq!(view.selected().map(|row| row.workstream_id), Some(first));
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });
        let first_row = view.selected().unwrap().clone();
        let second_row = view.snapshot.workstreams[1].clone();

        view.mark_attached(&first_row);
        assert!(view.is_attached_to(&first_row));
        assert!(!view.is_attached_to(&second_row));

        view.replace_snapshot(LocalNavigatorSnapshot {
            workstreams: vec![
                row(first, NavigatorRuntimeStatus::Parked),
                row(second, NavigatorRuntimeStatus::Idle),
            ],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });

        assert!(!view.is_attached_to(view.selected().unwrap()));
    }

    #[test]
    fn mouse_click_retains_blank_focus_and_row_activation_intent() {
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });

        view.begin_mouse_click(None);
        assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Blank));

        view.begin_mouse_click(Some(0));
        assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Row));
        assert_eq!(view.take_mouse_click(), None);
    }

    #[test]
    fn row_mapping_covers_both_lines_of_each_workstream() {
        let view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![
                row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle),
                row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked),
            ],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
    fn remote_projection_uses_the_host_project_display_name() {
        let remote = crate::protocol::SnapshotWorkstream {
            workstream_id: WorkstreamId::new(),
            location_id: crate::domain::LocationId::new(),
            project_display_name: "dms-power-status".to_owned(),
            display_name: "thread".to_owned(),
            runtime_id: None,
            runtime_status: RuntimeStatus::Idle,
            lifecycle: WorkstreamLifecycle::Open,
            result_ready: false,
            recovery_required: false,
            attention_revision: None,
            activity_sequence: 0,
            last_activity_at_millis: Some(1_000),
            revision: Revision::INITIAL.value(),
        };

        let projected = project_remote_workstream("snap", &remote, true).unwrap();

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
    fn renderer_shows_done_indicator_without_provider_content() {
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        let view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![NavigatorWorkstream {
                project_label: "project".to_owned(),
                display_name: "native thread".to_owned(),
                result_ready: true,
                ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
            }],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
        assert_eq!(project_cell.fg, Color::Cyan);
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
    fn renderer_makes_unresolved_operation_recovery_visible() {
        let mut terminal = Terminal::new(TestBackend::new(200, 8)).unwrap();
        let view = NavigatorView::new(LocalNavigatorSnapshot {
            unresolved_operation_count: 1,
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

        assert!(rendered.contains("operation needs recovery"));
        assert!(rendered.contains("wsnav operations"));
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

    #[test]
    fn unreachable_remote_keeps_cached_status_and_attention_without_claiming_a_stop() {
        let mut monitor = RemoteMonitor::new();
        let workstream_id = WorkstreamId::new();
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
                reachable: false,
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });
        view.select_next();

        view.replace_snapshot(LocalNavigatorSnapshot {
            workstreams: vec![remote, local],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
            parse_created_workstream(format!("forked workstream {destination}\n").as_bytes())
                .unwrap(),
            destination
        );
        assert!(matches!(
            parse_created_workstream(b"forked workstream not-an-id\n"),
            Err(NavigatorError::InvalidActionResult)
        ));
    }

    fn row(
        workstream_id: WorkstreamId,
        runtime_status: NavigatorRuntimeStatus,
    ) -> NavigatorWorkstream {
        NavigatorWorkstream {
            host: NavigatorHost::Local,
            workstream_id,
            project_label: "project".to_owned(),
            display_name: "thread".to_owned(),
            runtime_status,
            result_ready: false,
            recovery_required: false,
            attention_revision: None,
            last_activity_at_millis: None,
            workstream_revision: Revision::INITIAL,
        }
    }
}
