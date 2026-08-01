//! Bounded local navigator projections and presentation state.
//!
//! This module deliberately deals in display metadata and explicit action
//! revisions. It never reads provider turns, terminal screens, prompts, or
//! provider payloads.

use std::{
    cmp::Ordering,
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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use thiserror::Error;

use crate::{
    domain::{
        Clock, HostId, ProjectId, Revision, RuntimeStatus, SystemClock, WorkstreamId,
        WorkstreamLifecycle,
    },
    presentation::{AttachmentPhase, AttachmentStatus, Presentation, PresentationError},
    process::{BoundedProcessError, output_bounded},
    provider::codex::names::{NameContext, resolve_name},
    runtime::{
        LinuxProcessProbe, PrivateRuntime, RuntimeError, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{
        ClientCatalog, ClientHost, ClientHostTransport, ClientProjectLocation, HostIdentity,
        HostRegistry, StateError, StateRoot, WorkstreamOverview,
    },
    transport::{HostClient, RemoteExecutable, SshDestination, SshEndpoint, SystemCommandRunner},
};

/// One bounded row rendered by the local navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorWorkstream {
    pub host: NavigatorHost,
    pub project_id: ProjectId,
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
    crate::repository::refresh_pending_metadata(&mut registry)?;
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
    let project = match catalog.local_project_location(host.host_id, overview.location_id)? {
        Some(project)
            if project_metadata_matches(
                &project,
                &overview.project_display_name,
                overview.remote_identity_fingerprint.as_deref(),
            ) =>
        {
            project
        }
        _ => catalog.register_local_project_location_with_identity(
            host,
            overview.location_id,
            executable,
            &overview.project_display_name,
            overview.remote_identity_fingerprint.as_deref(),
        )?,
    };
    Ok(NavigatorWorkstream {
        host: NavigatorHost::Local,
        project_id: project.project_id,
        workstream_id: overview.workstream_id,
        project_label: bounded_display(&project.display_name),
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
    catalog: &mut ClientCatalog,
    host_id: HostId,
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
    let project = match catalog.local_project_location(host_id, workstream.location_id)? {
        Some(project)
            if project_metadata_matches(
                &project,
                &workstream.project_display_name,
                workstream.repository_fingerprint.as_deref(),
            ) =>
        {
            project
        }
        _ => catalog.register_host_project_location(
            host_id,
            workstream.location_id,
            &workstream.project_display_name,
            workstream.repository_fingerprint.as_deref(),
        )?,
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
        project_id: project.project_id,
        workstream_id: workstream.workstream_id,
        project_label: bounded_display(&project.display_name),
        display_name: bounded_display(&workstream.display_name),
        runtime_status,
        result_ready: workstream.result_ready,
        recovery_required: workstream.recovery_required,
        attention_revision,
        last_activity_at_millis: workstream.last_activity_at_millis,
        workstream_revision,
    })
}

fn project_metadata_matches(
    project: &ClientProjectLocation,
    display_name: &str,
    repository_fingerprint: Option<&str>,
) -> bool {
    match repository_fingerprint {
        Some(fingerprint) => project.repository_fingerprint.as_deref() == Some(fingerprint),
        None if project.repository_fingerprint.is_some() => true,
        None => project.display_name == display_name,
    }
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
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
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
    host_id: HostId,
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
        catalog: &mut ClientCatalog,
        selected_host: Option<&str>,
    ) -> Result<(), NavigatorError> {
        let now = Instant::now();
        self.collect(now, catalog)?;
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
                    host_id: host.host_id,
                    outcome,
                });
            });
        }
        Ok(())
    }

    fn collect(&mut self, now: Instant, catalog: &mut ClientCatalog) -> Result<(), NavigatorError> {
        while let Ok(result) = self.receiver.try_recv() {
            let Some(host) = self.hosts.get_mut(&result.alias) else {
                continue;
            };
            host.pending = false;
            if let Ok(snapshot) = result.outcome {
                host.workstreams = snapshot
                    .workstreams
                    .iter()
                    .map(|workstream| {
                        project_remote_workstream(
                            catalog,
                            result.host_id,
                            &result.alias,
                            workstream,
                            true,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
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
        Ok(())
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
        local.workstreams.sort_by(compare_workstream_activity);
        local
    }

    fn request_soon(&mut self, host_alias: &str) {
        if let Some(host) = self.hosts.get_mut(host_alias) {
            host.next_poll = Instant::now();
        }
    }
}

/// Orders the combined client view by the same cross-host activity age it
/// displays. Per-host activity sequences remain authoritative only inside
/// their own durable host registry, so they cannot order rows from two hosts.
fn compare_workstream_activity(
    left: &NavigatorWorkstream,
    right: &NavigatorWorkstream,
) -> Ordering {
    right
        .last_activity_at_millis
        .cmp(&left.last_activity_at_millis)
        .then_with(|| left.host.alias().cmp(right.host.alias()))
        .then_with(|| left.project_id.cmp(&right.project_id))
        .then_with(|| left.workstream_id.cmp(&right.workstream_id))
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
    let mut catalog = ClientCatalog::open(root)?;
    remote.refresh(&mut catalog, selected_host)?;
    Ok(remote.combine(local))
}

/// Pure navigator selection and rendering state.
#[derive(Clone, Debug, Default)]
pub struct NavigatorView {
    snapshot: LocalNavigatorSnapshot,
    selected: usize,
    view_mode: NavigatorViewMode,
    attached: Option<(String, WorkstreamId)>,
    observed_attachment: Option<(uuid::Uuid, AttachmentPhase)>,
    rendered_offset: usize,
    rendered_mouse_rows: Vec<(u16, usize)>,
    mouse_click: Option<MouseClickIntent>,
    message: Option<String>,
    help_visible: bool,
    spinner_frame: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseClickIntent {
    Blank,
    Row,
}

/// Local presentation grouping only. It is deliberately not durable state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum NavigatorViewMode {
    #[default]
    Recent,
    Host,
    Project,
}

impl NavigatorViewMode {
    const fn next(self) -> Self {
        match self {
            Self::Recent => Self::Host,
            Self::Host => Self::Project,
            Self::Project => Self::Recent,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recent",
            Self::Host => "By host",
            Self::Project => "By project",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkstreamRowContext {
    Recent,
    Host,
    Project,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NavigatorListEntry {
    HostHeader {
        alias: String,
    },
    ProjectHeader {
        project_id: ProjectId,
        label: String,
    },
    Workstream {
        snapshot_index: usize,
        context: WorkstreamRowContext,
    },
}

impl NavigatorListEntry {
    const fn height(&self) -> u16 {
        match self {
            Self::HostHeader { .. } | Self::ProjectHeader { .. } => 1,
            Self::Workstream { .. } => 2,
        }
    }

    const fn workstream_index(&self) -> Option<usize> {
        match self {
            Self::Workstream { snapshot_index, .. } => Some(*snapshot_index),
            Self::HostHeader { .. } | Self::ProjectHeader { .. } => None,
        }
    }
}

impl NavigatorView {
    #[must_use]
    pub fn new(snapshot: LocalNavigatorSnapshot) -> Self {
        Self {
            snapshot,
            selected: 0,
            view_mode: NavigatorViewMode::Recent,
            attached: None,
            observed_attachment: None,
            rendered_offset: 0,
            rendered_mouse_rows: Vec::new(),
            mouse_click: None,
            message: None,
            help_visible: false,
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

    fn observe_attachment(&mut self, status: &AttachmentStatus) {
        let observation = (status.attempt_id, status.phase);
        let changed = self.observed_attachment != Some(observation);
        self.observed_attachment = Some(observation);
        match status.phase {
            AttachmentPhase::Pending | AttachmentPhase::Running => {
                self.attached = Some((status.host_alias.clone(), status.workstream_id));
                if changed {
                    self.set_message(if status.phase == AttachmentPhase::Pending {
                        "provider attachment starting"
                    } else {
                        "provider attached; use the native Codex UI directly"
                    });
                }
            }
            AttachmentPhase::Completed => {
                self.attached = None;
                if changed {
                    self.set_message(
                        "provider detached; press Enter or click this row to reconnect",
                    );
                }
            }
            AttachmentPhase::Failed => {
                self.attached = None;
                if changed {
                    self.set_message("attachment failed; press Enter or click row to retry");
                }
            }
        }
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

    fn toggle_help(&mut self) {
        self.help_visible = !self.help_visible;
    }

    fn dismiss_help(&mut self) {
        self.help_visible = false;
    }

    const fn help_visible(&self) -> bool {
        self.help_visible
    }

    fn cycle_view_mode(&mut self) {
        self.view_mode = self.view_mode.next();
    }

    const fn view_mode(&self) -> NavigatorViewMode {
        self.view_mode
    }

    fn advance_animation(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(frame.area());
        let entries = self.list_entries();
        let project_colors = visible_project_colors(&self.snapshot);
        let items = entries
            .iter()
            .map(|entry| {
                navigator_list_item(entry, &self.snapshot, &project_colors, self.spinner_frame)
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(
            entries
                .iter()
                .position(|entry| entry.workstream_index() == Some(self.selected)),
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Workstreams · {} ", self.view_mode().label())),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
            areas[0],
            &mut state,
        );
        self.rendered_offset = state.offset();
        self.update_rendered_mouse_rows(&entries, areas[0]);
        let help = self.footer_help();
        frame.render_widget(Paragraph::new(help).style(self.footer_style()), areas[1]);
        if self.help_visible {
            Self::render_help_overlay(frame, areas[0]);
        }
    }

    fn list_entries(&self) -> Vec<NavigatorListEntry> {
        match self.view_mode {
            NavigatorViewMode::Recent => self
                .snapshot
                .workstreams
                .iter()
                .enumerate()
                .map(|(snapshot_index, _)| NavigatorListEntry::Workstream {
                    snapshot_index,
                    context: WorkstreamRowContext::Recent,
                })
                .collect(),
            NavigatorViewMode::Host => {
                let mut groups = Vec::<(String, Vec<usize>)>::new();
                for (snapshot_index, row) in self.snapshot.workstreams.iter().enumerate() {
                    if let Some((_, indexes)) = groups
                        .iter_mut()
                        .find(|(alias, _)| alias == row.host.alias())
                    {
                        indexes.push(snapshot_index);
                    } else {
                        groups.push((row.host.alias().to_owned(), vec![snapshot_index]));
                    }
                }
                groups
                    .into_iter()
                    .flat_map(|(alias, indexes)| {
                        std::iter::once(NavigatorListEntry::HostHeader { alias }).chain(
                            indexes.into_iter().map(|snapshot_index| {
                                NavigatorListEntry::Workstream {
                                    snapshot_index,
                                    context: WorkstreamRowContext::Host,
                                }
                            }),
                        )
                    })
                    .collect()
            }
            NavigatorViewMode::Project => {
                let mut groups = Vec::<(ProjectId, String, Vec<usize>)>::new();
                for (snapshot_index, row) in self.snapshot.workstreams.iter().enumerate() {
                    if let Some((_, _, indexes)) = groups
                        .iter_mut()
                        .find(|(project_id, _, _)| *project_id == row.project_id)
                    {
                        indexes.push(snapshot_index);
                    } else {
                        groups.push((
                            row.project_id,
                            row.project_label.clone(),
                            vec![snapshot_index],
                        ));
                    }
                }
                groups
                    .into_iter()
                    .flat_map(|(project_id, label, indexes)| {
                        std::iter::once(NavigatorListEntry::ProjectHeader { project_id, label })
                            .chain(indexes.into_iter().map(|snapshot_index| {
                                NavigatorListEntry::Workstream {
                                    snapshot_index,
                                    context: WorkstreamRowContext::Project,
                                }
                            }))
                    })
                    .collect()
            }
        }
    }

    fn update_rendered_mouse_rows(&mut self, entries: &[NavigatorListEntry], area: Rect) {
        self.rendered_mouse_rows.clear();
        let content_top = area.y.saturating_add(1);
        let content_bottom = area.y.saturating_add(area.height.saturating_sub(1));
        let mut y = content_top;
        for entry in entries.iter().skip(self.rendered_offset) {
            if y >= content_bottom {
                break;
            }
            let next_y = y.saturating_add(entry.height()).min(content_bottom);
            if let Some(snapshot_index) = entry.workstream_index() {
                self.rendered_mouse_rows
                    .extend((y..next_y).map(|row_y| (row_y, snapshot_index)));
            }
            y = next_y;
        }
    }

    fn footer_help(&self) -> String {
        if self.help_visible {
            return "? or Esc closes help".to_owned();
        }
        if let Some(message) = &self.message {
            return message.clone();
        }
        let operation_hint = (self.snapshot.unresolved_operation_count > 0).then(|| {
            format!(
                "! {} operation{} needs recovery; use wsnav operations",
                self.snapshot.unresolved_operation_count,
                if self.snapshot.unresolved_operation_count == 1 {
                    ""
                } else {
                    "s"
                }
            )
        });
        if self.snapshot.workstreams.is_empty() {
            return format!(
                "No Workstreams yet; run wsnav register /path/to/git-checkout  ·  ? help{}",
                operation_hint.map_or_else(String::new, |hint| format!("  ·  {hint}"))
            );
        }
        let view_hint = format!("v view: {}  ·  ? help", self.view_mode().label());
        if let Some(operation_hint) = operation_hint {
            return format!("{operation_hint}  ·  {view_hint}");
        }
        if self.snapshot.unreachable_hosts.is_empty() {
            view_hint
        } else {
            format!(
                "{} unavailable; showing cached state  ·  {view_hint}",
                self.snapshot.unreachable_hosts.join(", "),
            )
        }
    }

    fn footer_style(&self) -> Style {
        if self.help_visible {
            Style::default().fg(Color::Cyan)
        } else if self.message.is_some()
            || self.snapshot.unresolved_operation_count > 0
            || !self.snapshot.unreachable_hosts.is_empty()
        {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Gray)
        }
    }

    fn render_help_overlay(frame: &mut Frame<'_>, area: Rect) {
        let overlay = centered_help_area(area);
        frame.render_widget(Clear, overlay);
        frame.render_widget(
            Paragraph::new(help_lines())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Keyboard shortcuts ")
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: true }),
            overlay,
        );
    }

    #[must_use]
    pub fn row_from_y(&self, y: u16) -> Option<usize> {
        self.rendered_mouse_rows
            .iter()
            .find_map(|(row_y, snapshot_index)| (*row_y == y).then_some(*snapshot_index))
    }
}

fn centered_help_area(area: Rect) -> Rect {
    const MAX_WIDTH: u16 = 52;
    const MAX_HEIGHT: u16 = 16;
    let width = area.width.saturating_sub(2).min(MAX_WIDTH);
    let height = area.height.saturating_sub(2).min(MAX_HEIGHT);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn help_lines() -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::Yellow);
    vec![
        Line::from(Span::styled("Navigation", heading)),
        Line::from(vec![
            Span::styled("↑/↓ or j/k", key),
            Span::raw("  select a Workstream"),
        ]),
        Line::from(vec![
            Span::styled("Enter", key),
            Span::raw("       open, start, or recover"),
        ]),
        Line::from(vec![
            Span::styled("Tab", key),
            Span::raw(" focus native agent   "),
            Span::styled("v", key),
            Span::raw(" change view"),
        ]),
        Line::raw(""),
        Line::from(Span::styled("Workstreams", heading)),
        Line::from(vec![
            Span::styled("n", key),
            Span::raw(" new Workstream     "),
            Span::styled("f", key),
            Span::raw(" fork at last settled turn"),
        ]),
        Line::from(vec![
            Span::styled("p", key),
            Span::raw(" park               "),
            Span::styled("a", key),
            Span::raw(" acknowledge attention"),
        ]),
        Line::raw(""),
        Line::from(Span::styled("Mouse", heading)),
        Line::raw("click row: open/focus   scroll: select"),
        Line::raw("click empty navigator space: focus navigator"),
        Line::raw(""),
        Line::from(vec![
            Span::styled("? / Esc", key),
            Span::raw(" close help; then "),
            Span::styled("q", key),
            Span::raw(" close navigator"),
        ]),
    ]
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn navigator_list_item(
    entry: &NavigatorListEntry,
    snapshot: &LocalNavigatorSnapshot,
    project_colors: &BTreeMap<ProjectId, Color>,
    spinner_frame: usize,
) -> ListItem<'static> {
    match entry {
        NavigatorListEntry::HostHeader { alias } => host_header_item(alias),
        NavigatorListEntry::ProjectHeader { project_id, label } => {
            project_header_item(*project_id, label, project_colors)
        }
        NavigatorListEntry::Workstream {
            snapshot_index,
            context,
        } => workstream_item(
            &snapshot.workstreams[*snapshot_index],
            *context,
            project_colors,
            spinner_frame,
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
    project_colors: &BTreeMap<ProjectId, Color>,
    spinner_frame: usize,
) -> ListItem<'static> {
    let (indicator, indicator_style) = status_indicator(row, spinner_frame);
    let thread_style = Style::default().fg(Color::White);
    ListItem::new(vec![
        workstream_context_line(row, context, project_colors),
        Line::from(thread_line(row, indicator, indicator_style, thread_style)),
    ])
}

fn workstream_context_line(
    row: &NavigatorWorkstream,
    context: WorkstreamRowContext,
    project_colors: &BTreeMap<ProjectId, Color>,
) -> Line<'static> {
    let host = || {
        Span::styled(
            row.host.alias().to_owned(),
            Style::default()
                .fg(host_color(row.host.alias()))
                .add_modifier(Modifier::BOLD),
        )
    };
    let project = || {
        Span::styled(
            row.project_label.clone(),
            Style::default()
                .fg(project_accent(row.project_id, project_colors))
                .add_modifier(Modifier::BOLD),
        )
    };
    match context {
        WorkstreamRowContext::Recent => Line::from(vec![
            Span::raw("   "),
            host(),
            Span::styled(" · ", Style::default().fg(Color::Gray)),
            project(),
        ]),
        WorkstreamRowContext::Host => Line::from(vec![
            Span::raw("   "),
            project_marker(row.project_id, project_colors),
            Span::raw(" "),
            project(),
        ]),
        WorkstreamRowContext::Project => Line::from(vec![Span::raw("   "), host()]),
    }
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

fn host_color(alias: &str) -> Color {
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
const HOST_LABEL_PALETTE: [Color; 4] = [
    Color::LightBlue,
    Color::Indexed(75),
    Color::Indexed(111),
    Color::Indexed(117),
];

/// Projects use a muted violet family that stays distinct from the cool host
/// axis and the green/yellow/red lifecycle-state colors.
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

/// Allocates distinct muted accents to concurrently visible Projects. The
/// Project IDs select a deterministic initial slot; collision probing prevents
/// a same-color wall for the normal dozen-project navigator scope.
fn visible_project_colors(snapshot: &LocalNavigatorSnapshot) -> BTreeMap<ProjectId, Color> {
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

fn thread_line(
    row: &NavigatorWorkstream,
    indicator: &'static str,
    indicator_style: Style,
    thread_style: Style,
) -> Vec<Span<'static>> {
    let now_millis = SystemClock.now_millis().ok();
    let mut line = vec![
        Span::raw(" "),
        Span::styled(indicator, indicator_style),
        Span::raw(" "),
        Span::styled(row.display_name.clone(), thread_style),
    ];
    line.push(Span::styled(" · ", Style::default().fg(Color::Gray)));
    line.push(Span::styled(
        activity_label(row.last_activity_at_millis, now_millis),
        Style::default().fg(activity_age_color(row.last_activity_at_millis, now_millis)),
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

/// Keeps age secondary to the Workstream title and lifecycle indicator. The
/// normal range stays neutral; only old unattended work receives a quiet warm
/// accent. This deliberately avoids green, yellow, and red, which belong to
/// lifecycle state.
fn activity_age_color(last_activity_at_millis: Option<i64>, now_millis: Option<i64>) -> Color {
    match activity_elapsed_seconds(last_activity_at_millis, now_millis) {
        None | Some(0..=3_599) => Color::DarkGray,
        Some(3_600..=86_399) => Color::Gray,
        Some(86_400..=604_799) => Color::Indexed(250),
        Some(_) => Color::Indexed(180),
    }
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
    #[error("the local navigator action could not be completed")]
    ActionProcess(#[source] BoundedProcessError),
    #[error("the local navigator action failed")]
    ActionFailed,
    #[error("the local navigator action did not return one Workstream ID")]
    InvalidActionResult,
    #[error("remote host is unavailable")]
    RemoteHostUnavailable,
    #[error("remote host returned an invalid bounded snapshot")]
    InvalidRemoteSnapshot(#[source] crate::domain::DomainError),
}

impl NavigatorError {
    fn from_action_process(source: BoundedProcessError) -> Self {
        match source {
            BoundedProcessError::OutputTooLarge => Self::ActionOutputTooLarge,
            other => Self::ActionProcess(other),
        }
    }
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
    refresh_attachment_status(&presentation, &mut view);
    let outcome: Result<(), NavigatorError> = loop {
        terminal.terminal.draw(|frame| view.render(frame))?;
        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if view.help_visible() => match key.code {
                    KeyCode::Char('?' | 'q') | KeyCode::Esc => {
                        view.dismiss_help();
                    }
                    _ => {}
                },
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Char('?') => view.toggle_help(),
                    KeyCode::Char('v') => view.cycle_view_mode(),
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
                Event::Mouse(mouse) if !view.help_visible() => match mouse.kind {
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
            refresh_attachment_status(&presentation, &mut view);
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
        view.set_message("no Workstream is registered; run wsnav register /path/to/git-checkout");
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
    let attachment = match attachment {
        Ok(attachment) => attachment,
        Err(error) => {
            view.set_message(action_message(&error));
            return;
        }
    };
    view.observe_attachment(&attachment);
    if let Err(error) = presentation.focus_provider() {
        view.set_message(action_message(&error));
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
        view.set_message("no Workstream is registered; run wsnav register /path/to/git-checkout");
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
    match attachment.and_then(|status| {
        view.observe_attachment(&status);
        presentation.focus_provider()
    }) {
        Ok(()) => view.set_message(action.success_message()),
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn refresh_attachment_status(presentation: &Presentation, view: &mut NavigatorView) {
    match presentation.attachment_status() {
        Ok(Some(status)) => view.observe_attachment(&status),
        Ok(None) => {}
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
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
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
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });

        assert!(!view.is_attached_to(view.selected().unwrap()));
    }

    #[test]
    fn terminal_attachment_outcome_allows_an_exact_same_row_retry() {
        let workstream_id = WorkstreamId::new();
        let workstream = row(workstream_id, NavigatorRuntimeStatus::Idle);
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![workstream.clone()],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![
                row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle),
                row(WorkstreamId::new(), NavigatorRuntimeStatus::Parked),
            ],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();

        terminal.draw(|frame| view.render(frame)).unwrap();

        assert_eq!(view.row_from_y(0), None);
        assert_eq!(view.row_from_y(1), Some(0));
        assert_eq!(view.row_from_y(2), Some(0));
        assert_eq!(view.row_from_y(3), Some(1));
        assert_eq!(view.row_from_y(4), Some(1));
        assert_eq!(view.row_from_y(5), None);
    }

    #[test]
    fn mouse_row_mapping_includes_the_rendered_scroll_offset() {
        let workstreams = (0..6)
            .map(|_| row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle))
            .collect::<Vec<_>>();
        let expected = workstreams[5].workstream_id;
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams,
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });
        view.selected = 5;
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();

        terminal.draw(|frame| view.render(frame)).unwrap();

        assert!(view.rendered_offset > 0);
        let clicked = view.row_from_y(3).unwrap();
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
            project_display_name: "dms-power-status".to_owned(),
            repository_fingerprint: None,
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
        catalog
            .register_local_project_location(
                &host,
                remote.location_id,
                Path::new("/bin/wsnav"),
                "dms-power-status",
            )
            .unwrap();

        let projected =
            project_remote_workstream(&mut catalog, host.host_id, "snap", &remote, true).unwrap();

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
    fn activity_age_color_is_quiet_until_a_workstream_is_stale() {
        assert_eq!(activity_age_color(None, Some(60_000)), Color::DarkGray);
        assert_eq!(
            activity_age_color(Some(0), Some(3_599_000)),
            Color::DarkGray
        );
        assert_eq!(activity_age_color(Some(0), Some(3_600_000)), Color::Gray);
        assert_eq!(
            activity_age_color(Some(0), Some(86_400_000)),
            Color::Indexed(250)
        );
        assert_eq!(
            activity_age_color(Some(0), Some(604_800_000)),
            Color::Indexed(180)
        );
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
    fn recent_context_uses_one_neutral_separator_between_color_axes() {
        let snapshot = LocalNavigatorSnapshot {
            workstreams: vec![NavigatorWorkstream {
                project_label: "project".to_owned(),
                ..row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)
            }],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        };
        let row = &snapshot.workstreams[0];
        let project_colors = visible_project_colors(&snapshot);

        let line = workstream_context_line(row, WorkstreamRowContext::Recent, &project_colors);

        assert_eq!(
            line.spans
                .iter()
                .filter(|span| span.content.as_ref() == " · ")
                .count(),
            1
        );
        assert!(!line.spans.iter().any(|span| span.content.as_ref() == "•"));
    }

    #[test]
    fn host_and_project_accents_use_separate_color_families() {
        assert_eq!(host_color("local"), HOST_LABEL_PALETTE[0]);
        for host_color in HOST_LABEL_PALETTE {
            assert!(!PROJECT_MARKER_PALETTE.contains(&host_color));
        }
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
        assert!(rendered.contains("? help"));
    }

    #[test]
    fn empty_navigator_requires_an_explicit_checkout_registration() {
        let view = NavigatorView::new(LocalNavigatorSnapshot::default());

        assert_eq!(
            view.footer_help(),
            "No Workstreams yet; run wsnav register /path/to/git-checkout  ·  ? help"
        );
    }

    #[test]
    fn help_toggle_is_navigator_local_state() {
        let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());

        assert!(!view.help_visible());
        view.toggle_help();
        assert!(view.help_visible());
        assert_eq!(view.footer_help(), "? or Esc closes help");

        view.dismiss_help();
        assert!(!view.help_visible());
    }

    #[test]
    fn view_mode_cycles_without_changing_the_selected_workstream() {
        let workstream_id = WorkstreamId::new();
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![row(workstream_id, NavigatorRuntimeStatus::Idle)],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });

        assert_eq!(view.view_mode(), NavigatorViewMode::Recent);
        view.cycle_view_mode();
        assert_eq!(view.view_mode(), NavigatorViewMode::Host);
        assert!(view.footer_help().contains("v view: By host"));
        view.cycle_view_mode();
        assert_eq!(view.view_mode(), NavigatorViewMode::Project);
        view.cycle_view_mode();

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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
        });

        view.cycle_view_mode();
        assert_eq!(
            view.list_entries(),
            vec![
                NavigatorListEntry::HostHeader {
                    alias: "snap".to_owned(),
                },
                NavigatorListEntry::Workstream {
                    snapshot_index: 0,
                    context: WorkstreamRowContext::Host,
                },
                NavigatorListEntry::HostHeader {
                    alias: "local".to_owned(),
                },
                NavigatorListEntry::Workstream {
                    snapshot_index: 1,
                    context: WorkstreamRowContext::Host,
                },
                NavigatorListEntry::Workstream {
                    snapshot_index: 2,
                    context: WorkstreamRowContext::Host,
                },
            ]
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|frame| view.render(frame)).unwrap();
        assert_eq!(view.row_from_y(1), None);
        assert_eq!(view.row_from_y(2), Some(0));
        assert_eq!(view.row_from_y(3), Some(0));

        view.cycle_view_mode();
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
                },
                NavigatorListEntry::Workstream {
                    snapshot_index: 1,
                    context: WorkstreamRowContext::Project,
                },
                NavigatorListEntry::ProjectHeader {
                    project_id: other_project,
                    label: "other".to_owned(),
                },
                NavigatorListEntry::Workstream {
                    snapshot_index: 2,
                    context: WorkstreamRowContext::Project,
                },
            ]
        );
        let project_colors = visible_project_colors(&view.snapshot);
        terminal.draw(|frame| view.render(frame)).unwrap();
        let project_header_label = &terminal.backend().buffer().content()[80 + 5];
        assert_eq!(project_header_label.symbol(), "s");
        assert_eq!(
            project_header_label.fg,
            project_accent(shared_project, &project_colors)
        );
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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

        assert!(rendered.contains("Keyboard shortcuts"));
        assert!(rendered.contains("Navigation"));
        assert!(rendered.contains("Workstreams"));
        assert!(rendered.contains("click row: open/focus"));
        assert!(rendered.contains("change view"));
        assert!(rendered.contains("? or Esc closes help"));
        assert!(!rendered.contains("provider pane"));
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
                reachable: true,
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
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
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
            project_id: ProjectId::new(),
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
