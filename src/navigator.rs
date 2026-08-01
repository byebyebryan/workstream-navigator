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
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use thiserror::Error;

use crate::{
    domain::{
        Clock, HostId, LocationId, OperationId, OperationKind, OperationPhase, ProjectId, Revision,
        RuntimeStatus, SystemClock, WorkstreamId, WorkstreamLifecycle,
    },
    presentation::{AttachmentPhase, AttachmentStatus, Presentation, PresentationError},
    process::{BoundedProcessError, output_bounded},
    provider::codex::names::{NameContext, resolve_name},
    runtime::{
        LinuxProcessProbe, PrivateRuntime, RuntimeError, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{
        ClientCatalog, ClientHost, ClientHostTransport, ClientProjectLocation, HostIdentity,
        HostRegistry, IntegrationLifecycle, StateError, StateRoot, WorkstreamOverview,
    },
    transport::{HostClient, RemoteExecutable, SshDestination, SshEndpoint, SystemCommandRunner},
};

/// One bounded row rendered by the local navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorWorkstream {
    pub host: NavigatorHost,
    pub project_id: ProjectId,
    /// Opaque host-owned location identity used only for Project actions.
    pub location_id: LocationId,
    pub workstream_id: WorkstreamId,
    pub project_label: String,
    /// Bounded host-supplied location label; never a filesystem path.
    pub location_label: String,
    pub display_name: String,
    pub runtime_status: NavigatorRuntimeStatus,
    pub archived: bool,
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

const fn operation_kind_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Start => "Start",
        OperationKind::Fork => "Fork",
    }
}

const fn operation_phase_label(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::Prepared => "prepared",
        OperationPhase::ExternalEffectStarted => "external effect started",
        OperationPhase::AwaitingReconciliation => "awaiting reconciliation",
        OperationPhase::Committed => "committed",
        OperationPhase::RecoveryRequired => "recovery required",
        OperationPhase::Failed => "failed",
    }
}

/// A complete bounded projection of the local host registry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalNavigatorSnapshot {
    pub workstreams: Vec<NavigatorWorkstream>,
    pub hosts: Vec<NavigatorHostOverview>,
    pub unreachable_hosts: Vec<String>,
    pub unresolved_operation_count: usize,
    pub unresolved_operations: Vec<NavigatorOperation>,
}

/// Opaque recovery metadata. Request keys, paths, provider identifiers, and
/// effect evidence remain on the host; the operation ID is held only long
/// enough for the navigator to issue exact recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorOperation {
    pub host: NavigatorHost,
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub revision: Revision,
}

/// Bounded host presentation metadata independent of whether the host currently
/// has a visible Workstream. The client catalog and host handshake remain the
/// authority; this is only enough to render the Hosts page safely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorHostOverview {
    pub alias: String,
    pub reachability: RemoteHostReachability,
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
    let unresolved_operations = registry
        .unresolved_operation_overviews()?
        .into_iter()
        .map(|operation| NavigatorOperation {
            host: NavigatorHost::Local,
            operation_id: operation.operation_id,
            kind: operation.kind,
            phase: operation.phase,
            revision: operation.revision,
        })
        .collect::<Vec<_>>();
    Ok(LocalNavigatorSnapshot {
        workstreams,
        hosts: vec![NavigatorHostOverview {
            alias: "local".to_owned(),
            reachability: RemoteHostReachability::Reachable,
        }],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: unresolved_operations.len(),
        unresolved_operations,
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
        location_id: overview.location_id,
        workstream_id: overview.workstream_id,
        project_label: bounded_display(&project.display_name),
        location_label: bounded_display(&overview.project_display_name),
        display_name,
        runtime_status,
        archived: overview.archived_at_millis.is_some(),
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
        location_id: workstream.location_id,
        workstream_id: workstream.workstream_id,
        project_label: bounded_display(&project.display_name),
        location_label: bounded_display(&workstream.project_display_name),
        display_name: bounded_display(&workstream.display_name),
        runtime_status,
        archived: workstream.archived,
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
const MAX_NAVIGATOR_CHECKOUT_PATH_BYTES: usize = 4096;

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
    unresolved_operations: Vec<NavigatorOperation>,
    reachable: bool,
    pending: bool,
    next_poll: Instant,
    backoff: Duration,
}

struct RemotePollResult {
    alias: String,
    host_id: HostId,
    outcome: Result<
        (
            crate::protocol::SnapshotResponse,
            crate::protocol::OperationsResponse,
        ),
        (),
    >,
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
                    unresolved_operations: Vec::new(),
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
            if let Ok((snapshot, operations)) = result.outcome {
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
                host.unresolved_operations = operations
                    .operations
                    .into_iter()
                    .filter_map(|operation| {
                        Revision::try_from(operation.revision).ok().map(|revision| {
                            NavigatorOperation {
                                host: NavigatorHost::Remote {
                                    alias: result.alias.clone(),
                                    reachability: RemoteHostReachability::Reachable,
                                },
                                operation_id: operation.operation_id,
                                kind: operation.kind,
                                phase: operation.phase,
                                revision,
                            }
                        })
                    })
                    .collect();
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
            local.hosts.push(NavigatorHostOverview {
                alias: alias.clone(),
                reachability: if host.reachable {
                    RemoteHostReachability::Reachable
                } else {
                    RemoteHostReachability::Unreachable
                },
            });
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
            local
                .unresolved_operations
                .extend(host.unresolved_operations.iter().cloned());
        }
        local
            .hosts
            .sort_by(|left, right| left.alias.cmp(&right.alias));
        local
            .hosts
            .dedup_by(|left, right| left.alias == right.alias);
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

fn fetch_remote_snapshot(
    host: &ClientHost,
) -> Result<
    (
        crate::protocol::SnapshotResponse,
        crate::protocol::OperationsResponse,
    ),
    (),
> {
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
    let snapshot = client.snapshot_ssh(&endpoint).map_err(|_| ())?;
    let operations = client.operations_ssh(&endpoint).map_err(|_| ())?;
    Ok((snapshot, operations))
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
    page: NavigatorPage,
    detail: Option<NavigatorDetail>,
    selected_project: Option<ProjectId>,
    selected_project_location: usize,
    selected_host: Option<String>,
    selected_operation: usize,
    view_mode: NavigatorViewMode,
    workstream_scope: WorkstreamScope,
    attached: Option<(String, WorkstreamId)>,
    observed_attachment: Option<(uuid::Uuid, AttachmentPhase)>,
    rendered_offset: usize,
    rendered_mouse_rows: Vec<(u16, usize)>,
    rendered_project_rows: Vec<(u16, ProjectId)>,
    rendered_host_rows: Vec<(u16, String)>,
    rendered_page_tabs: Vec<(Rect, NavigatorPage)>,
    mouse_click: Option<MouseClickIntent>,
    message: Option<String>,
    help_visible: bool,
    help_scroll: u16,
    modal: Option<NavigatorModal>,
    spinner_frame: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseClickIntent {
    Blank,
    Row,
    Project,
    Host,
    Page(NavigatorPage),
}

/// The active navigator page. This remains process-local presentation state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum NavigatorPage {
    #[default]
    Workstreams,
    Projects,
    Hosts,
}

impl NavigatorPage {
    const fn label(self) -> &'static str {
        match self {
            Self::Workstreams => "Workstreams",
            Self::Projects => "Projects",
            Self::Hosts => "Hosts",
        }
    }

    const fn shortcut(self) -> char {
        match self {
            Self::Workstreams => '1',
            Self::Projects => '2',
            Self::Hosts => '3',
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NavigatorDetail {
    Recovery,
    Workstream {
        host_alias: String,
        workstream_id: WorkstreamId,
    },
    Project(ProjectId),
    Host(String),
}

/// Active and archived Workstreams are separate navigator visibility scopes.
/// This is local presentation state; archive authority remains revision-guarded
/// on the selected host.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum WorkstreamScope {
    #[default]
    Active,
    Archived,
}

impl WorkstreamScope {
    const fn next(self) -> Self {
        match self {
            Self::Active => Self::Archived,
            Self::Archived => Self::Active,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Archived => "Archived",
        }
    }

    const fn includes(self, workstream: &NavigatorWorkstream) -> bool {
        match self {
            Self::Active => !workstream.archived,
            Self::Archived => workstream.archived,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NavigatorModal {
    ConfirmArchive(NavigatorWorkstream),
    Rename {
        workstream: NavigatorWorkstream,
        value: String,
    },
    SelectRegistrationHost {
        hosts: Vec<NavigatorHost>,
        selected: usize,
    },
    RegisterCheckout {
        host: NavigatorHost,
        value: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NavigatorProjectOverview {
    project_id: ProjectId,
    label: String,
    workstream_count: usize,
    active_workstream_count: usize,
    archived_workstream_count: usize,
    locations: Vec<NavigatorProjectLocation>,
    latest_activity_at_millis: Option<i64>,
}

/// One host-owned `ProjectLocation` summarized without a repository path or
/// durable provider identifier. The opaque location ID remains presentation
/// action identity only.
#[derive(Clone, Debug, Eq, PartialEq)]
struct NavigatorProjectLocation {
    host: NavigatorHost,
    location_id: LocationId,
    label: String,
    active_workstream_count: usize,
    archived_workstream_count: usize,
    latest_activity_at_millis: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NavigatorHostSummary {
    alias: String,
    reachability: RemoteHostReachability,
    workstream_count: usize,
    latest_activity_at_millis: Option<i64>,
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
            Self::Recent => Self::Project,
            Self::Project => Self::Host,
            Self::Host => Self::Recent,
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
        tree_branch: Option<TreeBranch>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TreeBranch {
    is_last: bool,
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
        let mut view = Self {
            snapshot,
            selected: 0,
            page: NavigatorPage::Workstreams,
            detail: None,
            selected_project: None,
            selected_project_location: 0,
            selected_host: Some("local".to_owned()),
            selected_operation: 0,
            view_mode: NavigatorViewMode::Recent,
            workstream_scope: WorkstreamScope::Active,
            attached: None,
            observed_attachment: None,
            rendered_offset: 0,
            rendered_mouse_rows: Vec::new(),
            rendered_project_rows: Vec::new(),
            rendered_host_rows: Vec::new(),
            rendered_page_tabs: Vec::new(),
            mouse_click: None,
            message: None,
            help_visible: false,
            help_scroll: 0,
            modal: None,
            spinner_frame: 0,
        };
        view.normalize_page_selection();
        view
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
        self.normalize_page_selection();
        self.normalize_workstream_selection();
    }

    #[must_use]
    pub fn selected(&self) -> Option<&NavigatorWorkstream> {
        self.workstream_is_visible(self.selected)
            .then(|| self.snapshot.workstreams.get(self.selected))
            .flatten()
    }

    fn workstream_is_visible(&self, snapshot_index: usize) -> bool {
        self.snapshot
            .workstreams
            .get(snapshot_index)
            .is_some_and(|workstream| self.workstream_scope.includes(workstream))
    }

    fn visible_workstream_indexes(&self) -> Vec<usize> {
        self.snapshot
            .workstreams
            .iter()
            .enumerate()
            .filter_map(|(index, workstream)| {
                self.workstream_scope.includes(workstream).then_some(index)
            })
            .collect()
    }

    fn normalize_workstream_selection(&mut self) {
        if !self.workstream_is_visible(self.selected) {
            self.selected = self
                .visible_workstream_indexes()
                .into_iter()
                .next()
                .unwrap_or(0);
        }
    }

    fn selected_host_alias(&self) -> Option<&str> {
        if matches!(self.detail, Some(NavigatorDetail::Recovery)) {
            self.snapshot
                .unresolved_operations
                .get(self.selected_operation)
                .map(|operation| operation.host.alias())
        } else if self.page == NavigatorPage::Hosts {
            self.selected_host.as_deref()
        } else {
            self.selected().map(|row| row.host.alias())
        }
    }

    const fn page(&self) -> NavigatorPage {
        self.page
    }

    fn select_page(&mut self, page: NavigatorPage) {
        if self.page != page {
            self.page = page;
            self.detail = None;
            self.clear_message();
        }
    }

    fn open_selected_detail(&mut self) {
        self.detail = match self.page {
            NavigatorPage::Workstreams => {
                self.selected()
                    .map(|workstream| NavigatorDetail::Workstream {
                        host_alias: workstream.host.alias().to_owned(),
                        workstream_id: workstream.workstream_id,
                    })
            }
            NavigatorPage::Projects => {
                self.selected_project_location = 0;
                self.selected_project.map(NavigatorDetail::Project)
            }
            NavigatorPage::Hosts => self.selected_host.clone().map(NavigatorDetail::Host),
        };
    }

    fn open_recovery(&mut self) {
        if self.snapshot.unresolved_operations.is_empty() {
            self.set_message("no unresolved Workstream operations");
        } else {
            self.selected_operation = self
                .selected_operation
                .min(self.snapshot.unresolved_operations.len().saturating_sub(1));
            self.detail = Some(NavigatorDetail::Recovery);
        }
    }

    fn dismiss_detail(&mut self) -> bool {
        self.detail.take().is_some()
    }

    fn projects(&self) -> Vec<NavigatorProjectOverview> {
        let mut projects = Vec::<NavigatorProjectOverview>::new();
        for workstream in &self.snapshot.workstreams {
            if let Some(project) = projects
                .iter_mut()
                .find(|project| project.project_id == workstream.project_id)
            {
                project.workstream_count += 1;
                if workstream.archived {
                    project.archived_workstream_count += 1;
                } else {
                    project.active_workstream_count += 1;
                }
                project.latest_activity_at_millis = project
                    .latest_activity_at_millis
                    .max(workstream.last_activity_at_millis);
                if let Some(location) = project.locations.iter_mut().find(|location| {
                    location.host.alias() == workstream.host.alias()
                        && location.location_id == workstream.location_id
                }) {
                    if workstream.archived {
                        location.archived_workstream_count += 1;
                    } else {
                        location.active_workstream_count += 1;
                    }
                    location.latest_activity_at_millis = location
                        .latest_activity_at_millis
                        .max(workstream.last_activity_at_millis);
                } else {
                    project.locations.push(NavigatorProjectLocation {
                        host: workstream.host.clone(),
                        location_id: workstream.location_id,
                        label: workstream.location_label.clone(),
                        active_workstream_count: usize::from(!workstream.archived),
                        archived_workstream_count: usize::from(workstream.archived),
                        latest_activity_at_millis: workstream.last_activity_at_millis,
                    });
                }
            } else {
                projects.push(NavigatorProjectOverview {
                    project_id: workstream.project_id,
                    label: workstream.project_label.clone(),
                    workstream_count: 1,
                    active_workstream_count: usize::from(!workstream.archived),
                    archived_workstream_count: usize::from(workstream.archived),
                    locations: vec![NavigatorProjectLocation {
                        host: workstream.host.clone(),
                        location_id: workstream.location_id,
                        label: workstream.location_label.clone(),
                        active_workstream_count: usize::from(!workstream.archived),
                        archived_workstream_count: usize::from(workstream.archived),
                        latest_activity_at_millis: workstream.last_activity_at_millis,
                    }],
                    latest_activity_at_millis: workstream.last_activity_at_millis,
                });
            }
        }
        for project in &mut projects {
            project.locations.sort_by(|left, right| {
                left.host
                    .alias()
                    .cmp(right.host.alias())
                    .then_with(|| left.label.cmp(&right.label))
                    .then_with(|| left.location_id.cmp(&right.location_id))
            });
        }
        projects.sort_by(|left, right| {
            right
                .latest_activity_at_millis
                .cmp(&left.latest_activity_at_millis)
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.project_id.cmp(&right.project_id))
        });
        projects
    }

    fn selected_project_location_source(&self) -> Option<NavigatorWorkstream> {
        let NavigatorDetail::Project(project_id) = self.detail.as_ref()? else {
            return None;
        };
        let location = self
            .projects()
            .into_iter()
            .find(|project| project.project_id == *project_id)?
            .locations
            .get(self.selected_project_location)?
            .clone();
        self.snapshot
            .workstreams
            .iter()
            .find(|workstream| {
                workstream.project_id == *project_id
                    && workstream.location_id == location.location_id
                    && workstream.host.alias() == location.host.alias()
            })
            .cloned()
    }

    fn select_project_for_workstream(
        &mut self,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) -> bool {
        let Some((index, project_id)) = self
            .snapshot
            .workstreams
            .iter()
            .enumerate()
            .find(|(_, workstream)| {
                workstream.host.alias() == host_alias && workstream.workstream_id == workstream_id
            })
            .map(|(index, workstream)| (index, workstream.project_id))
        else {
            return false;
        };
        self.page = NavigatorPage::Projects;
        self.detail = None;
        self.selected = index;
        self.selected_project = Some(project_id);
        true
    }

    fn hosts(&self) -> Vec<NavigatorHostSummary> {
        let mut hosts = self
            .snapshot
            .hosts
            .iter()
            .map(|host| NavigatorHostSummary {
                alias: host.alias.clone(),
                reachability: host.reachability,
                workstream_count: 0,
                latest_activity_at_millis: None,
            })
            .collect::<Vec<_>>();
        for workstream in &self.snapshot.workstreams {
            if let Some(host) = hosts
                .iter_mut()
                .find(|host| host.alias == workstream.host.alias())
            {
                host.workstream_count += 1;
                host.latest_activity_at_millis = host
                    .latest_activity_at_millis
                    .max(workstream.last_activity_at_millis);
            } else {
                hosts.push(NavigatorHostSummary {
                    alias: workstream.host.alias().to_owned(),
                    reachability: if workstream.host.is_reachable() {
                        RemoteHostReachability::Reachable
                    } else {
                        RemoteHostReachability::Unreachable
                    },
                    workstream_count: 1,
                    latest_activity_at_millis: workstream.last_activity_at_millis,
                });
            }
        }
        hosts.sort_by(|left, right| {
            (left.alias != "local")
                .cmp(&(right.alias != "local"))
                .then_with(|| left.alias.cmp(&right.alias))
        });
        hosts
    }

    fn normalize_page_selection(&mut self) {
        self.selected_operation = self
            .selected_operation
            .min(self.snapshot.unresolved_operations.len().saturating_sub(1));
        let projects = self.projects();
        if !projects
            .iter()
            .any(|project| Some(project.project_id) == self.selected_project)
        {
            self.selected_project = projects.first().map(|project| project.project_id);
        }
        if let Some(NavigatorDetail::Project(project_id)) = self.detail.as_ref() {
            self.selected_project_location = projects
                .iter()
                .find(|project| project.project_id == *project_id)
                .map_or(0, |project| {
                    self.selected_project_location
                        .min(project.locations.len().saturating_sub(1))
                });
        }
        let hosts = self.hosts();
        if !hosts
            .iter()
            .any(|host| Some(host.alias.as_str()) == self.selected_host.as_deref())
        {
            self.selected_host = hosts.first().map(|host| host.alias.clone());
        }
        match &self.detail {
            Some(NavigatorDetail::Recovery) if self.snapshot.unresolved_operations.is_empty() => {
                self.detail = None;
            }
            Some(NavigatorDetail::Workstream {
                host_alias,
                workstream_id,
            }) if !self.snapshot.workstreams.iter().any(|workstream| {
                workstream.host.alias() == host_alias && workstream.workstream_id == *workstream_id
            }) =>
            {
                self.detail = None;
            }
            Some(NavigatorDetail::Project(project_id))
                if !projects
                    .iter()
                    .any(|project| project.project_id == *project_id) =>
            {
                self.detail = None;
            }
            Some(NavigatorDetail::Host(alias))
                if !hosts.iter().any(|host| host.alias == *alias) =>
            {
                self.detail = None;
            }
            Some(_) | None => {}
        }
    }

    pub fn select_next(&mut self) {
        if matches!(self.detail, Some(NavigatorDetail::Recovery)) {
            if !self.snapshot.unresolved_operations.is_empty() {
                self.selected_operation =
                    (self.selected_operation + 1) % self.snapshot.unresolved_operations.len();
            }
            return;
        }
        if let Some(NavigatorDetail::Project(project_id)) = self.detail.as_ref() {
            if let Some(project) = self
                .projects()
                .into_iter()
                .find(|project| project.project_id == *project_id)
                && !project.locations.is_empty()
            {
                self.selected_project_location =
                    (self.selected_project_location + 1) % project.locations.len();
            }
            return;
        }
        if self.detail.is_some() {
            return;
        }
        match self.page {
            NavigatorPage::Workstreams => {
                let visible = self.visible_workstream_indexes();
                if let Some(index) = visible.iter().position(|index| *index == self.selected) {
                    self.selected = visible[(index + 1) % visible.len()];
                } else if let Some(first) = visible.first() {
                    self.selected = *first;
                }
            }
            NavigatorPage::Projects => {
                let projects = self.projects();
                if let Some(index) = projects
                    .iter()
                    .position(|project| Some(project.project_id) == self.selected_project)
                {
                    self.selected_project = Some(projects[(index + 1) % projects.len()].project_id);
                } else {
                    self.selected_project = projects.first().map(|project| project.project_id);
                }
            }
            NavigatorPage::Hosts => {
                let hosts = self.hosts();
                if let Some(index) = hosts
                    .iter()
                    .position(|host| Some(host.alias.as_str()) == self.selected_host.as_deref())
                {
                    self.selected_host = Some(hosts[(index + 1) % hosts.len()].alias.clone());
                } else {
                    self.selected_host = hosts.first().map(|host| host.alias.clone());
                }
            }
        }
    }

    pub fn select_previous(&mut self) {
        if matches!(self.detail, Some(NavigatorDetail::Recovery)) {
            if !self.snapshot.unresolved_operations.is_empty() {
                self.selected_operation = self
                    .selected_operation
                    .checked_sub(1)
                    .unwrap_or(self.snapshot.unresolved_operations.len() - 1);
            }
            return;
        }
        if let Some(NavigatorDetail::Project(project_id)) = self.detail.as_ref() {
            if let Some(project) = self
                .projects()
                .into_iter()
                .find(|project| project.project_id == *project_id)
                && !project.locations.is_empty()
            {
                self.selected_project_location = self
                    .selected_project_location
                    .checked_sub(1)
                    .unwrap_or(project.locations.len() - 1);
            }
            return;
        }
        if self.detail.is_some() {
            return;
        }
        match self.page {
            NavigatorPage::Workstreams => {
                let visible = self.visible_workstream_indexes();
                if let Some(index) = visible.iter().position(|index| *index == self.selected) {
                    self.selected = visible[(index + visible.len() - 1) % visible.len()];
                } else if let Some(first) = visible.first() {
                    self.selected = *first;
                }
            }
            NavigatorPage::Projects => {
                let projects = self.projects();
                if let Some(index) = projects
                    .iter()
                    .position(|project| Some(project.project_id) == self.selected_project)
                {
                    self.selected_project =
                        Some(projects[(index + projects.len() - 1) % projects.len()].project_id);
                } else {
                    self.selected_project = projects.first().map(|project| project.project_id);
                }
            }
            NavigatorPage::Hosts => {
                let hosts = self.hosts();
                if let Some(index) = hosts
                    .iter()
                    .position(|host| Some(host.alias.as_str()) == self.selected_host.as_deref())
                {
                    self.selected_host =
                        Some(hosts[(index + hosts.len() - 1) % hosts.len()].alias.clone());
                } else {
                    self.selected_host = hosts.first().map(|host| host.alias.clone());
                }
            }
        }
    }

    pub fn select_row(&mut self, row: usize) {
        if self.workstream_is_visible(row) {
            self.selected = row;
        }
    }

    fn select_workstream(&mut self, host_alias: &str, workstream_id: WorkstreamId) -> bool {
        let Some(selected) = self
            .snapshot
            .workstreams
            .iter()
            .position(|row| row.host.alias() == host_alias && row.workstream_id == workstream_id)
            .filter(|index| self.workstream_is_visible(*index))
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
                && !workstream.archived
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
        self.help_scroll = 0;
    }

    fn dismiss_help(&mut self) {
        self.help_visible = false;
        self.help_scroll = 0;
    }

    fn begin_archive_confirmation(&mut self, workstream: NavigatorWorkstream) {
        self.modal = Some(NavigatorModal::ConfirmArchive(workstream));
    }

    fn begin_rename(&mut self, workstream: NavigatorWorkstream) {
        self.modal = Some(NavigatorModal::Rename {
            value: workstream.display_name.clone(),
            workstream,
        });
    }

    fn begin_checkout_registration(&mut self) {
        let mut hosts = self
            .snapshot
            .hosts
            .iter()
            .map(|host| {
                if host.alias == "local" {
                    NavigatorHost::Local
                } else {
                    NavigatorHost::Remote {
                        alias: host.alias.clone(),
                        reachability: host.reachability,
                    }
                }
            })
            .collect::<Vec<_>>();
        if !hosts
            .iter()
            .any(|host| matches!(host, NavigatorHost::Local))
        {
            hosts.push(NavigatorHost::Local);
        }
        hosts.sort_by(|left, right| {
            (left.alias() != "local")
                .cmp(&(right.alias() != "local"))
                .then_with(|| left.alias().cmp(right.alias()))
        });
        hosts.truncate(16);
        self.modal = Some(NavigatorModal::SelectRegistrationHost { hosts, selected: 0 });
    }

    fn select_registration_host_next(&mut self) {
        let Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) = self.modal.as_mut()
        else {
            return;
        };
        if !hosts.is_empty() {
            *selected = (*selected + 1) % hosts.len();
        }
    }

    fn select_registration_host_previous(&mut self) {
        let Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) = self.modal.as_mut()
        else {
            return;
        };
        if !hosts.is_empty() {
            *selected = selected.checked_sub(1).unwrap_or(hosts.len() - 1);
        }
    }

    fn dismiss_modal(&mut self) {
        self.modal = None;
    }

    fn confirm_modal(&mut self) -> Option<NavigatorModal> {
        self.modal.take()
    }

    const fn modal_visible(&self) -> bool {
        self.modal.is_some()
    }

    const fn help_visible(&self) -> bool {
        self.help_visible
    }

    fn scroll_help_next(&mut self) {
        let last = help_lines(
            self.page,
            self.detail.is_some(),
            matches!(self.detail, Some(NavigatorDetail::Recovery)),
            matches!(self.detail, Some(NavigatorDetail::Project(_))),
            self.workstream_scope,
        )
        .len()
        .saturating_sub(1);
        self.help_scroll = self
            .help_scroll
            .saturating_add(1)
            .min(u16::try_from(last).unwrap_or(u16::MAX));
    }

    fn scroll_help_previous(&mut self) {
        self.help_scroll = self.help_scroll.saturating_sub(1);
    }

    fn cycle_view_mode(&mut self) {
        self.view_mode = self.view_mode.next();
    }

    fn cycle_workstream_scope(&mut self) {
        self.workstream_scope = self.workstream_scope.next();
        self.normalize_workstream_selection();
    }

    const fn view_mode(&self) -> NavigatorViewMode {
        self.view_mode
    }

    fn advance_animation(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }

    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let help_height = if self.help_visible {
            frame.area().height.saturating_sub(4).clamp(3, 12)
        } else {
            2
        };
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(help_height),
            ])
            .split(frame.area());
        let content = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(2)])
            .split(areas[0]);
        self.render_page_tabs(frame, content[0]);
        match self.detail.clone() {
            Some(NavigatorDetail::Recovery) => self.render_recovery_detail(frame, content[1]),
            Some(NavigatorDetail::Workstream {
                host_alias,
                workstream_id,
            }) => self.render_workstream_detail(frame, content[1], &host_alias, workstream_id),
            Some(NavigatorDetail::Project(project_id)) => {
                self.render_project_detail(frame, content[1], project_id);
            }
            Some(NavigatorDetail::Host(alias)) => {
                self.render_host_detail(frame, content[1], &alias);
            }
            None => match self.page {
                NavigatorPage::Workstreams => self.render_workstreams(frame, content[1]),
                NavigatorPage::Projects => self.render_projects(frame, content[1]),
                NavigatorPage::Hosts => self.render_hosts(frame, content[1]),
            },
        }
        frame.render_widget(
            Paragraph::new(self.footer_status()).style(self.footer_style()),
            areas[1],
        );
        if self.help_visible {
            self.render_help_reference(frame, areas[2]);
        } else {
            frame.render_widget(
                Paragraph::new(self.compact_key_lines(areas[2].width))
                    .style(Style::default().fg(Color::Gray)),
                areas[2],
            );
        }
        if let Some(modal) = self.modal.clone() {
            Self::render_modal(frame, modal);
        }
    }

    fn render_page_tabs(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.rendered_page_tabs.clear();
        let pages = [
            NavigatorPage::Workstreams,
            NavigatorPage::Projects,
            NavigatorPage::Hosts,
        ];
        let mut spans = Vec::new();
        let mut x = area.x;
        for page in pages {
            let label = format!(" {} {} ", page.shortcut(), page.label());
            let width = u16::try_from(label.chars().count()).unwrap_or(u16::MAX);
            if x >= area.right() {
                break;
            }
            let visible_width = width.min(area.right().saturating_sub(x));
            self.rendered_page_tabs
                .push((Rect::new(x, area.y, visible_width, area.height), page));
            let style = if self.page == page {
                Style::default()
                    .fg(Color::White)
                    .bg(SELECTED_ROW_BACKGROUND)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(label, style));
            x = x.saturating_add(width);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_workstreams(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let entries = self.list_entries();
        let project_colors = visible_project_colors(&self.snapshot);
        let items = entries
            .iter()
            .map(|entry| {
                navigator_list_item(
                    entry,
                    &self.snapshot,
                    &project_colors,
                    self.spinner_frame,
                    area.width.saturating_sub(2),
                )
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
                .block(Block::default().borders(Borders::ALL).title(format!(
                    " Workstreams · {} · {} ",
                    self.workstream_scope.label(),
                    self.view_mode().label()
                )))
                .highlight_style(selected_row_style()),
            area,
            &mut state,
        );
        self.rendered_offset = state.offset();
        self.update_rendered_mouse_rows(&entries, area);
    }

    fn render_projects(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let projects = self.projects();
        let items = projects
            .iter()
            .map(project_overview_item)
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(
            projects
                .iter()
                .position(|project| Some(project.project_id) == self.selected_project),
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Projects "))
                .highlight_style(selected_row_style()),
            area,
            &mut state,
        );
        self.update_rendered_project_rows(&projects, state.offset(), area);
    }

    fn render_hosts(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let hosts = self.hosts();
        let items = hosts.iter().map(host_overview_item).collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(
            hosts
                .iter()
                .position(|host| Some(host.alias.as_str()) == self.selected_host.as_deref()),
        );
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Hosts "))
                .highlight_style(selected_row_style()),
            area,
            &mut state,
        );
        self.update_rendered_host_rows(&hosts, state.offset(), area);
    }

    fn render_workstream_detail(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        host_alias: &str,
        workstream_id: WorkstreamId,
    ) {
        let Some(workstream) = self.snapshot.workstreams.iter().find(|workstream| {
            workstream.host.alias() == host_alias && workstream.workstream_id == workstream_id
        }) else {
            return;
        };
        let attention = if workstream.recovery_required {
            "native recovery needed"
        } else if workstream.result_ready {
            "result ready"
        } else {
            "none"
        };
        let visibility = if workstream.archived {
            "archived; restore does not start Codex"
        } else {
            "active"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    workstream.display_name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::raw(format!(
                    "{} · {}",
                    workstream.host.alias(),
                    workstream.project_label
                )),
                Line::raw(format!("Runtime: {}", workstream.runtime_status.label())),
                Line::raw(format!("Attention: {attention}")),
                Line::raw(format!("Visibility: {visibility}")),
                Line::raw("Enter or Esc returns to Workstreams"),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Workstream status "),
            ),
            area,
        );
    }

    fn render_recovery_detail(&self, frame: &mut Frame<'_>, area: Rect) {
        let lines = self
            .snapshot
            .unresolved_operations
            .iter()
            .enumerate()
            .map(|(index, operation)| {
                let marker = if index == self.selected_operation {
                    "> "
                } else {
                    "  "
                };
                Line::from(Span::styled(
                    format!(
                        "{marker}{} · {} · {}",
                        operation.host.alias(),
                        operation_kind_label(operation.kind),
                        operation_phase_label(operation.phase)
                    ),
                    if index == self.selected_operation {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Recovery · Enter reconcile · Esc back "),
            ),
            area,
        );
    }

    fn render_project_detail(&self, frame: &mut Frame<'_>, area: Rect, project_id: ProjectId) {
        let Some(project) = self
            .projects()
            .into_iter()
            .find(|project| project.project_id == project_id)
        else {
            return;
        };
        let mut lines = vec![
            Line::from(Span::styled(
                project.label.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::raw(project_activity_summary(&project)),
            Line::raw("Locations"),
        ];
        lines.extend(
            project
                .locations
                .iter()
                .enumerate()
                .map(|(index, location)| {
                    let marker = if index == self.selected_project_location {
                        "> "
                    } else {
                        "  "
                    };
                    let activity = location_activity_summary(location);
                    Line::from(Span::styled(
                        format!(
                            "{marker}{} · {} · {activity}",
                            location.host.alias(),
                            location.label,
                        ),
                        if index == self.selected_project_location {
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Gray)
                        },
                    ))
                }),
        );
        lines.push(Line::raw(
            "↑/↓ selects · n starts at location · Enter/Esc returns",
        ));
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Project · n start selected location "),
            ),
            area,
        );
    }

    fn render_host_detail(&self, frame: &mut Frame<'_>, area: Rect, alias: &str) {
        let Some(host) = self.hosts().into_iter().find(|host| host.alias == alias) else {
            return;
        };
        let reachability = match host.reachability {
            RemoteHostReachability::Reachable => "reachable",
            RemoteHostReachability::Unreachable => "unreachable; showing cached state",
        };
        let workstream_label = if host.workstream_count == 1 {
            "1 Workstream".to_owned()
        } else {
            format!("{} Workstreams", host.workstream_count)
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    host.alias,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::raw(reachability),
                Line::raw(workstream_label),
                Line::raw("Enter or Esc returns to the Host list"),
            ])
            .block(Block::default().borders(Borders::ALL).title(" Host ")),
            area,
        );
    }

    fn list_entries(&self) -> Vec<NavigatorListEntry> {
        match self.view_mode {
            NavigatorViewMode::Recent => self
                .snapshot
                .workstreams
                .iter()
                .enumerate()
                .filter(|(_, row)| self.workstream_scope.includes(row))
                .map(|(snapshot_index, _)| NavigatorListEntry::Workstream {
                    snapshot_index,
                    context: WorkstreamRowContext::Recent,
                    tree_branch: None,
                })
                .collect(),
            NavigatorViewMode::Host => {
                let mut groups = Vec::<(String, Vec<usize>)>::new();
                for (snapshot_index, row) in self.snapshot.workstreams.iter().enumerate() {
                    if !self.workstream_scope.includes(row) {
                        continue;
                    }
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
                        let count = indexes.len();
                        std::iter::once(NavigatorListEntry::HostHeader { alias }).chain(
                            indexes
                                .into_iter()
                                .enumerate()
                                .map(move |(index, snapshot_index)| {
                                    NavigatorListEntry::Workstream {
                                        snapshot_index,
                                        context: WorkstreamRowContext::Host,
                                        tree_branch: Some(TreeBranch {
                                            is_last: index + 1 == count,
                                        }),
                                    }
                                }),
                        )
                    })
                    .collect()
            }
            NavigatorViewMode::Project => {
                let mut groups = Vec::<(ProjectId, String, Vec<usize>)>::new();
                for (snapshot_index, row) in self.snapshot.workstreams.iter().enumerate() {
                    if !self.workstream_scope.includes(row) {
                        continue;
                    }
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
                        let count = indexes.len();
                        std::iter::once(NavigatorListEntry::ProjectHeader { project_id, label })
                            .chain(indexes.into_iter().enumerate().map(
                                move |(index, snapshot_index)| NavigatorListEntry::Workstream {
                                    snapshot_index,
                                    context: WorkstreamRowContext::Project,
                                    tree_branch: Some(TreeBranch {
                                        is_last: index + 1 == count,
                                    }),
                                },
                            ))
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

    fn update_rendered_project_rows(
        &mut self,
        projects: &[NavigatorProjectOverview],
        offset: usize,
        area: Rect,
    ) {
        self.rendered_project_rows.clear();
        let content_top = area.y.saturating_add(1);
        let content_bottom = area.y.saturating_add(area.height.saturating_sub(1));
        let mut y = content_top;
        for project in projects.iter().skip(offset) {
            if y >= content_bottom {
                break;
            }
            let next_y = y.saturating_add(2).min(content_bottom);
            self.rendered_project_rows
                .extend((y..next_y).map(|row_y| (row_y, project.project_id)));
            y = next_y;
        }
    }

    fn update_rendered_host_rows(
        &mut self,
        hosts: &[NavigatorHostSummary],
        offset: usize,
        area: Rect,
    ) {
        self.rendered_host_rows.clear();
        let content_top = area.y.saturating_add(1);
        let content_bottom = area.y.saturating_add(area.height.saturating_sub(1));
        let mut y = content_top;
        for host in hosts.iter().skip(offset) {
            if y >= content_bottom {
                break;
            }
            let next_y = y.saturating_add(2).min(content_bottom);
            self.rendered_host_rows
                .extend((y..next_y).map(|row_y| (row_y, host.alias.clone())));
            y = next_y;
        }
    }

    fn page_from_position(&self, column: u16, row: u16) -> Option<NavigatorPage> {
        self.rendered_page_tabs.iter().find_map(|(area, page)| {
            (column >= area.x && column < area.right() && row >= area.y && row < area.bottom())
                .then_some(*page)
        })
    }

    fn project_from_y(&self, y: u16) -> Option<ProjectId> {
        self.rendered_project_rows
            .iter()
            .find_map(|(row_y, project_id)| (*row_y == y).then_some(*project_id))
    }

    fn host_from_y(&self, y: u16) -> Option<String> {
        self.rendered_host_rows
            .iter()
            .find_map(|(row_y, alias)| (*row_y == y).then_some(alias.clone()))
    }

    fn begin_page_click(&mut self, page: NavigatorPage) {
        self.mouse_click = Some(MouseClickIntent::Page(page));
    }

    fn begin_project_click(&mut self, project_id: Option<ProjectId>) {
        self.mouse_click = Some(if project_id.is_some() {
            MouseClickIntent::Project
        } else {
            MouseClickIntent::Blank
        });
        if let Some(project_id) = project_id {
            self.selected_project = Some(project_id);
        }
    }

    fn begin_host_click(&mut self, alias: Option<String>) {
        self.mouse_click = Some(if alias.is_some() {
            MouseClickIntent::Host
        } else {
            MouseClickIntent::Blank
        });
        if let Some(alias) = alias {
            self.selected_host = Some(alias);
        }
    }

    fn footer_status(&self) -> String {
        if let Some(message) = &self.message {
            return message.clone();
        }
        let operation_hint = (self.snapshot.unresolved_operation_count > 0).then(|| {
            format!(
                "! {} operation{} needs recovery",
                self.snapshot.unresolved_operation_count,
                if self.snapshot.unresolved_operation_count == 1 {
                    ""
                } else {
                    "s"
                }
            )
        });
        if self.visible_workstream_indexes().is_empty() {
            let empty_label = if self.snapshot.workstreams.is_empty() {
                "No Workstreams yet · n registers a checkout".to_owned()
            } else {
                format!(
                    "No {} Workstreams",
                    self.workstream_scope.label().to_ascii_lowercase()
                )
            };
            return format!(
                "{empty_label}{}",
                operation_hint.map_or_else(String::new, |hint| format!("  ·  {hint}"))
            );
        }
        let view_hint = if self.page == NavigatorPage::Workstreams {
            format!("view: {}", self.view_mode().label())
        } else {
            String::new()
        };
        if let Some(operation_hint) = operation_hint {
            return if view_hint.is_empty() {
                operation_hint
            } else {
                format!("{operation_hint}  ·  {view_hint}")
            };
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

    fn compact_key_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width < 16 {
            return vec![binding_line(&[("?", "keys")])];
        }
        let bindings = self.compact_bindings();
        let mut rows = vec![Vec::<(&str, &str)>::new()];
        let mut row_width = 0_usize;
        let maximum = usize::from(width);
        for binding in bindings {
            let binding_width = binding.0.len() + 1 + binding.1.len();
            let separator_width = usize::from(!rows.last().unwrap().is_empty()) * 2;
            if row_width + separator_width + binding_width > maximum {
                if rows.len() == 2 {
                    break;
                }
                rows.push(Vec::new());
                row_width = 0;
            }
            if !rows.last().unwrap().is_empty() {
                row_width += 2;
            }
            row_width += binding_width;
            rows.last_mut().unwrap().push(binding);
        }
        rows.into_iter()
            .filter(|row| !row.is_empty())
            .map(|row| binding_line(&row))
            .collect()
    }

    fn compact_bindings(&self) -> Vec<(&'static str, &'static str)> {
        if let Some(detail) = &self.detail {
            let enter = if matches!(detail, NavigatorDetail::Recovery) {
                "reconcile"
            } else {
                "back"
            };
            let mut bindings = vec![("Enter", enter), ("Esc", "back")];
            if matches!(detail, NavigatorDetail::Project(_)) {
                bindings.push(("n", "start location"));
                bindings.push(("a", "add checkout"));
            }
            bindings.push(("?", "keys"));
            return bindings;
        }
        match self.page {
            NavigatorPage::Workstreams => {
                let mut bindings = vec![("Enter", "open"), ("i", "status")];
                match self.workstream_scope {
                    WorkstreamScope::Active => bindings.extend([
                        (
                            "n",
                            if self.snapshot.workstreams.is_empty() {
                                "register"
                            } else {
                                "new"
                            },
                        ),
                        ("f", "fork"),
                        ("p", "park"),
                        ("r", "rename"),
                        ("x", "archive"),
                        ("a", "ack"),
                    ]),
                    WorkstreamScope::Archived => bindings.push(("u", "restore")),
                }
                bindings.extend([("s", "scope"), ("v", "view"), ("?", "keys")]);
                bindings.insert(2, ("o", "recovery"));
                bindings
            }
            NavigatorPage::Projects => vec![
                ("Enter", "details"),
                ("n", "register checkout"),
                ("1", "workstreams"),
                ("3", "hosts"),
                ("?", "keys"),
            ],
            NavigatorPage::Hosts => vec![
                ("Enter", "details"),
                ("1", "workstreams"),
                ("2", "projects"),
                ("?", "keys"),
            ],
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

    fn render_help_reference(&self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_widget(
            Paragraph::new(help_lines(
                self.page,
                self.detail.is_some(),
                matches!(self.detail, Some(NavigatorDetail::Recovery)),
                matches!(self.detail, Some(NavigatorDetail::Project(_))),
                self.workstream_scope,
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Keys · {} ", self.page.label()))
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .scroll((self.help_scroll, 0)),
            area,
        );
    }

    fn render_modal(frame: &mut Frame<'_>, modal: NavigatorModal) {
        let area = navigator_modal_area(frame.area(), &modal);
        let (title, lines) = navigator_modal_content(modal);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
            area,
        );
    }

    #[must_use]
    pub fn row_from_y(&self, y: u16) -> Option<usize> {
        self.rendered_mouse_rows
            .iter()
            .find_map(|(row_y, snapshot_index)| (*row_y == y).then_some(*snapshot_index))
    }
}

fn navigator_modal_area(outer: Rect, modal: &NavigatorModal) -> Rect {
    let width = outer.width.min(52);
    let desired_height = match modal {
        NavigatorModal::SelectRegistrationHost { hosts, .. } => hosts.len().saturating_add(4),
        NavigatorModal::ConfirmArchive(_)
        | NavigatorModal::Rename { .. }
        | NavigatorModal::RegisterCheckout { .. } => 7,
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

fn navigator_modal_content(modal: NavigatorModal) -> (String, Vec<Line<'static>>) {
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
                Line::raw("This parks the working Codex Runtime before archiving."),
                Line::from(vec![
                    Span::styled("Enter/y", key),
                    Span::raw(" confirm   "),
                    Span::styled("Esc/n", key),
                    Span::raw(" cancel"),
                ]),
            ],
        ),
        NavigatorModal::Rename { value, .. } => (
            " Rename Workstream ".to_owned(),
            vec![
                Line::raw("Set the canonical Codex thread title:"),
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
            let mut lines = vec![Line::raw("Choose the host that owns this checkout:")];
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
            (" Register checkout · choose host ".to_owned(), lines)
        }
        NavigatorModal::RegisterCheckout { host, value } => (
            if host.is_remote() {
                " Register remote checkout ".to_owned()
            } else {
                " Register local checkout ".to_owned()
            },
            vec![
                Line::raw(format!(
                    "Enter an existing Git checkout on {}:",
                    host.alias()
                )),
                Line::from(Span::styled(
                    truncate_display(&value, 44),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::raw("The path is sent only to the selected host."),
                Line::from(vec![
                    Span::styled("Enter", key),
                    Span::raw(" register   "),
                    Span::styled("Esc", key),
                    Span::raw(" cancel"),
                ]),
            ],
        ),
    }
}

fn binding_line(bindings: &[(&str, &str)]) -> Line<'static> {
    let key = Style::default().fg(Color::Yellow);
    let label = Style::default().fg(Color::Gray);
    let mut spans = Vec::new();
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

fn help_lines(
    page: NavigatorPage,
    showing_detail: bool,
    showing_recovery: bool,
    showing_project: bool,
    workstream_scope: WorkstreamScope,
) -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::Yellow);
    let mut lines = vec![
        Line::from(Span::styled("Navigation", heading)),
        Line::from(vec![Span::styled("↑/↓ or j/k", key), Span::raw("  select")]),
        Line::from(vec![
            Span::styled("1", key),
            Span::raw("          Workstreams page"),
        ]),
        Line::from(vec![
            Span::styled("2", key),
            Span::raw("          Projects page"),
        ]),
        Line::from(vec![
            Span::styled("3", key),
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
        if showing_project {
            lines.extend([
                Line::from(vec![
                    Span::styled("n", key),
                    Span::raw("          start a new Workstream at the selected location"),
                ]),
                Line::from(vec![
                    Span::styled("a", key),
                    Span::raw("          register another existing checkout"),
                ]),
            ]);
        }
    } else if page == NavigatorPage::Workstreams {
        lines.extend(workstream_help_lines(workstream_scope, heading, key));
    } else {
        let mut page_lines = vec![
            Line::raw(""),
            Line::from(Span::styled(page.label(), heading)),
            Line::from(vec![
                Span::styled("Enter", key),
                Span::raw("      show bounded details"),
            ]),
        ];
        if page == NavigatorPage::Projects {
            page_lines.push(Line::from(vec![
                Span::styled("n", key),
                Span::raw("          register an existing checkout"),
            ]));
        }
        lines.extend(page_lines);
    }
    lines.extend([
        Line::raw(""),
        Line::from(Span::styled("Mouse", heading)),
        Line::raw("click a tab to switch pages"),
        Line::raw("click a row to select; release to open or focus"),
        Line::raw("click empty navigator space to focus it"),
        Line::raw(""),
        Line::from(vec![
            Span::styled("?", key),
            Span::raw("          close keys"),
        ]),
        Line::from(vec![
            Span::styled("Esc", key),
            Span::raw("        close keys"),
        ]),
        Line::from(vec![
            Span::styled("q", key),
            Span::raw("          close keys"),
        ]),
    ]);
    lines
}

fn workstream_help_lines(scope: WorkstreamScope, heading: Style, key: Style) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled("Workstreams", heading)),
        Line::from(vec![
            Span::styled("Enter", key),
            Span::raw("      open, start, or recover"),
        ]),
        Line::from(vec![
            Span::styled("i", key),
            Span::raw("          show bounded status"),
        ]),
        Line::from(vec![
            Span::styled("Tab", key),
            Span::raw("        focus native agent"),
        ]),
        Line::from(vec![
            Span::styled("v", key),
            Span::raw("          cycle recent/project/host"),
        ]),
        Line::from(vec![
            Span::styled("s", key),
            Span::raw("          switch active/archived scope"),
        ]),
        Line::from(vec![
            Span::styled("o", key),
            Span::raw("          recover an unresolved Start or Fork"),
        ]),
    ];
    if scope == WorkstreamScope::Active {
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
                Span::raw("          rename canonical Codex thread"),
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
    } else {
        lines.push(Line::from(vec![
            Span::styled("u", key),
            Span::raw("          restore without starting Codex"),
        ]));
    }
    lines
}

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Keep selection distinct from every semantic row foreground. In particular,
/// `DarkGray` is reserved for secondary activity text and the parked marker,
/// so it must never become the selected-row background.
const SELECTED_ROW_BACKGROUND: Color = Color::Indexed(236);
const PARKED_INDICATOR_COLOR: Color = Color::Indexed(110);

/// Activity age is a neutral brightness ramp rather than another identity or
/// lifecycle color. Recent work should be easiest to spot; stale work remains
/// readable but deliberately recedes.
const AGE_UNKNOWN_COLOR: Color = Color::Indexed(244);
const AGE_RECENT_COLOR: Color = Color::Indexed(255);
const AGE_HOURLY_COLOR: Color = Color::Indexed(251);
const AGE_DAILY_COLOR: Color = Color::Indexed(247);
const AGE_WEEKLY_COLOR: Color = Color::Indexed(244);
const AGE_STALE_COLOR: Color = Color::Indexed(241);

fn selected_row_style() -> Style {
    Style::default()
        .bg(SELECTED_ROW_BACKGROUND)
        .add_modifier(Modifier::BOLD)
}

fn navigator_list_item(
    entry: &NavigatorListEntry,
    snapshot: &LocalNavigatorSnapshot,
    project_colors: &BTreeMap<ProjectId, Color>,
    spinner_frame: usize,
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
            spinner_frame,
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

fn project_overview_item(project: &NavigatorProjectOverview) -> ListItem<'static> {
    ListItem::new(vec![
        Line::from(Span::styled(
            project.label.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                project_activity_summary(project),
                Style::default().fg(Color::Gray),
            ),
            Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "{} location{}",
                    project.locations.len(),
                    if project.locations.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
                Style::default().fg(Color::LightBlue),
            ),
        ]),
    ])
}

fn project_activity_summary(project: &NavigatorProjectOverview) -> String {
    format!(
        "{} active · {} archived",
        project.active_workstream_count, project.archived_workstream_count
    )
}

fn location_activity_summary(location: &NavigatorProjectLocation) -> String {
    format!(
        "{} active · {} archived",
        location.active_workstream_count, location.archived_workstream_count
    )
}

fn host_overview_item(host: &NavigatorHostSummary) -> ListItem<'static> {
    let reachability = match host.reachability {
        RemoteHostReachability::Reachable => "available",
        RemoteHostReachability::Unreachable => "unavailable; cached",
    };
    let workstream_label = if host.workstream_count == 1 {
        "1 Workstream".to_owned()
    } else {
        format!("{} Workstreams", host.workstream_count)
    };
    ListItem::new(vec![
        Line::from(Span::styled(
            host.alias.clone(),
            Style::default()
                .fg(host_color(&host.alias))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(workstream_label, Style::default().fg(Color::Gray)),
            Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                reachability,
                Style::default().fg(match host.reachability {
                    RemoteHostReachability::Reachable => Color::Gray,
                    RemoteHostReachability::Unreachable => Color::Yellow,
                }),
            ),
        ]),
    ])
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
    spinner_frame: usize,
    available_width: u16,
) -> ListItem<'static> {
    let (indicator, indicator_style) = status_indicator(row, spinner_frame);
    let thread_style = Style::default().fg(Color::White);
    let (context_prefix, thread_prefix) = tree_prefix(tree_branch);
    ListItem::new(vec![
        workstream_context_line(row, context, context_prefix, project_colors),
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

fn workstream_context_line(
    row: &NavigatorWorkstream,
    context: WorkstreamRowContext,
    prefix: &str,
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
            Span::raw(prefix.to_owned()),
            host(),
            Span::styled(" · ", Style::default().fg(Color::Gray)),
            project(),
        ]),
        WorkstreamRowContext::Host => Line::from(vec![
            Span::raw(prefix.to_owned()),
            project_marker(row.project_id, project_colors),
            Span::raw(" "),
            project(),
        ]),
        WorkstreamRowContext::Project => Line::from(vec![Span::raw(prefix.to_owned()), host()]),
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

/// Uses a readable neutral ramp, brightest for the newest activity and
/// progressively muted for older work. This deliberately avoids the
/// green/yellow/red lifecycle colors and the host/project identity palettes.
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
    let mut observer_needs_review = initialize_observer_activation_message(root, &mut view);
    let mut terminal = TerminalSession::enter()?;
    let mut last_refresh = Instant::now();
    let mut last_animation = Instant::now();
    refresh_attachment_status(&presentation, &mut view);
    let outcome: Result<(), NavigatorError> = loop {
        terminal.terminal.draw(|frame| view.render(frame))?;
        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            let exit = match event::read()? {
                Event::Key(key) => {
                    handle_navigator_key(key, root, &presentation, &mut remote, &mut view)
                }
                Event::Mouse(mouse) if !view.help_visible() => {
                    handle_navigator_mouse(mouse, root, &presentation, &mut remote, &mut view);
                    false
                }
                _ => false,
            };
            if exit {
                break Ok(());
            }
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            refresh_navigator(
                root,
                &presentation,
                &mut remote,
                &mut view,
                &mut observer_needs_review,
            );
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

fn handle_navigator_key(
    key: crossterm::event::KeyEvent,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    if view.modal_visible() {
        return handle_navigator_modal_key(key, root, remote, view);
    }
    if view.help_visible() {
        match key.code {
            KeyCode::Char('?' | 'q') | KeyCode::Esc => view.dismiss_help(),
            KeyCode::Down | KeyCode::Char('j') => view.scroll_help_next(),
            KeyCode::Up | KeyCode::Char('k') => view.scroll_help_previous(),
            _ => {}
        }
        return false;
    }
    if matches!(view.detail, Some(NavigatorDetail::Recovery)) && matches!(key.code, KeyCode::Enter)
    {
        recover_selected_operation(root, remote, view);
        return false;
    }
    if matches!(view.detail, Some(NavigatorDetail::Project(_)))
        && matches!(key.code, KeyCode::Char('n'))
    {
        create_workstream_from_selected_project_location(root, presentation, remote, view);
        return false;
    }
    if matches!(view.detail, Some(NavigatorDetail::Project(_)))
        && matches!(key.code, KeyCode::Char('a'))
    {
        view.begin_checkout_registration();
        return false;
    }
    let workstreams = view.page() == NavigatorPage::Workstreams && view.detail.is_none();
    if workstreams && handle_workstream_action_key(key.code, root, presentation, remote, view) {
        return false;
    }
    match key.code {
        KeyCode::Char('q') => true,
        KeyCode::Esc => !view.dismiss_detail(),
        KeyCode::Char('?') => {
            view.toggle_help();
            false
        }
        KeyCode::Char('1') => {
            view.select_page(NavigatorPage::Workstreams);
            false
        }
        KeyCode::Char('2') => {
            view.select_page(NavigatorPage::Projects);
            false
        }
        KeyCode::Char('3') => {
            view.select_page(NavigatorPage::Hosts);
            false
        }
        KeyCode::Char('v') if workstreams => {
            view.cycle_view_mode();
            false
        }
        KeyCode::Char('s') if workstreams => {
            view.cycle_workstream_scope();
            false
        }
        KeyCode::Char('o') if workstreams => {
            view.open_recovery();
            false
        }
        KeyCode::Char('n') if view.page() == NavigatorPage::Projects && view.detail.is_none() => {
            view.begin_checkout_registration();
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            view.select_next();
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            view.select_previous();
            false
        }
        KeyCode::Tab if workstreams && view.workstream_scope == WorkstreamScope::Active => {
            if let Err(error) = presentation.focus_provider() {
                view.set_message(action_message(&error));
            }
            false
        }
        KeyCode::Enter if view.dismiss_detail() => false,
        KeyCode::Enter if workstreams => {
            activate_selected(root, presentation, remote, view);
            false
        }
        KeyCode::Char('i') if workstreams => {
            view.open_selected_detail();
            false
        }
        KeyCode::Enter => {
            view.open_selected_detail();
            false
        }
        _ => false,
    }
}

fn handle_workstream_action_key(
    key: KeyCode,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    match (key, view.workstream_scope) {
        (KeyCode::Char('a'), WorkstreamScope::Active) => acknowledge_selected(root, remote, view),
        (KeyCode::Char('p'), WorkstreamScope::Active) => park_selected(root, remote, view),
        (KeyCode::Char('x'), WorkstreamScope::Active) => archive_selected(root, remote, view),
        (KeyCode::Char('r'), WorkstreamScope::Active) => rename_selected(view),
        (KeyCode::Char('u'), WorkstreamScope::Archived) => restore_selected(root, remote, view),
        (KeyCode::Char('n'), WorkstreamScope::Active) => {
            create_workstream_selected(
                root,
                presentation,
                remote,
                view,
                CreationAction::Independent,
            );
        }
        (KeyCode::Char('f'), WorkstreamScope::Active) => {
            create_workstream_selected(root, presentation, remote, view, CreationAction::Fork);
        }
        _ => return false,
    }
    true
}

fn handle_navigator_modal_key(
    key: crossterm::event::KeyEvent,
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) -> bool {
    if matches!(key.code, KeyCode::Esc) {
        view.dismiss_modal();
        return false;
    }
    if matches!(key.code, KeyCode::Char('n'))
        && matches!(view.modal, Some(NavigatorModal::ConfirmArchive(_)))
    {
        view.dismiss_modal();
        return false;
    }
    if matches!(
        view.modal,
        Some(NavigatorModal::SelectRegistrationHost { .. })
    ) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => view.select_registration_host_next(),
            KeyCode::Up | KeyCode::Char('k') => view.select_registration_host_previous(),
            _ => {}
        }
        if !matches!(key.code, KeyCode::Enter) {
            return false;
        }
    }
    if matches!(key.code, KeyCode::Enter) {
        match view.confirm_modal() {
            Some(NavigatorModal::ConfirmArchive(workstream)) => {
                archive_workstream(root, remote, view, &workstream);
            }
            Some(NavigatorModal::Rename { workstream, value }) => {
                rename_workstream(root, remote, view, &workstream, &value);
            }
            Some(NavigatorModal::SelectRegistrationHost { hosts, selected }) => {
                if let Some(host) = hosts.get(selected).cloned() {
                    view.modal = Some(NavigatorModal::RegisterCheckout {
                        host,
                        value: String::new(),
                    });
                } else {
                    view.set_message("no registered host is available for checkout registration");
                }
            }
            Some(NavigatorModal::RegisterCheckout { host, value }) if value.trim().is_empty() => {
                view.modal = Some(NavigatorModal::RegisterCheckout { host, value });
                view.set_message("enter an existing Git checkout path");
            }
            Some(NavigatorModal::RegisterCheckout { host, value }) => {
                register_checkout(root, remote, view, &host, &value);
            }
            None => {}
        }
        return false;
    }
    if matches!(key.code, KeyCode::Char('y'))
        && matches!(view.modal, Some(NavigatorModal::ConfirmArchive(_)))
    {
        if let Some(NavigatorModal::ConfirmArchive(workstream)) = view.confirm_modal() {
            archive_workstream(root, remote, view, &workstream);
        }
        return false;
    }
    match view.modal.as_mut() {
        Some(NavigatorModal::Rename { value, .. }) => match key.code {
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character) if !character.is_control() && value.chars().count() < 512 => {
                value.push(character);
            }
            _ => {}
        },
        Some(NavigatorModal::RegisterCheckout { value, .. }) => match key.code {
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(character)
                if !character.is_control() && value.len() < MAX_NAVIGATOR_CHECKOUT_PATH_BYTES =>
            {
                value.push(character);
            }
            _ => {}
        },
        Some(NavigatorModal::ConfirmArchive(_) | NavigatorModal::SelectRegistrationHost { .. })
        | None => {}
    }
    false
}

fn handle_navigator_mouse(
    mouse: crossterm::event::MouseEvent,
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    if view.modal_visible() {
        return;
    }
    match mouse.kind {
        MouseEventKind::ScrollDown => view.select_next(),
        MouseEventKind::ScrollUp => view.select_previous(),
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(page) = view.page_from_position(mouse.column, mouse.row) {
                view.begin_page_click(page);
            } else if view.detail.is_some() {
                view.begin_mouse_click(None);
            } else {
                match view.page() {
                    NavigatorPage::Workstreams => {
                        view.begin_mouse_click(view.row_from_y(mouse.row));
                    }
                    NavigatorPage::Projects => {
                        view.begin_project_click(view.project_from_y(mouse.row));
                    }
                    NavigatorPage::Hosts => view.begin_host_click(view.host_from_y(mouse.row)),
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => match view.take_mouse_click() {
            Some(MouseClickIntent::Row) => activate_selected(root, presentation, remote, view),
            Some(MouseClickIntent::Project | MouseClickIntent::Host) => view.open_selected_detail(),
            Some(MouseClickIntent::Page(page)) => view.select_page(page),
            Some(MouseClickIntent::Blank) => {
                if let Err(error) = presentation.focus_navigator() {
                    view.set_message(action_message(&error));
                }
            }
            None => {}
        },
        _ => {}
    }
}

fn initialize_observer_activation_message(root: &StateRoot, view: &mut NavigatorView) -> bool {
    let pending = observer_review_pending(root);
    if pending {
        view.set_message(
            "approve the observer hooks in the native Codex pane with /hooks, then exit Codex",
        );
    }
    pending
}

fn refresh_navigator(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    observer_needs_review: &mut bool,
) {
    let selected_host = view.selected_host_alias().map(str::to_owned);
    match combined_snapshot(root, remote, selected_host.as_deref()) {
        Ok(snapshot) => view.replace_snapshot(snapshot),
        Err(error) => view.set_message(action_message(&error)),
    }
    refresh_attachment_status(presentation, view);
    let now_pending = observer_review_pending(root);
    if *observer_needs_review && !now_pending {
        view.set_message("observer ready; native Workstreams can now start");
    }
    *observer_needs_review = now_pending;
}

fn observer_review_pending(root: &StateRoot) -> bool {
    HostRegistry::open(root)
        .ok()
        .and_then(|registry| registry.codex_integration().ok().flatten())
        .is_none_or(|integration| integration.lifecycle != IntegrationLifecycle::Ready)
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
    if selected.archived {
        view.set_message("restore this Workstream before opening Codex");
        return;
    }
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

fn archive_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no active Workstream is selected");
        return;
    };
    if selected.archived {
        view.set_message("this Workstream is already archived");
        return;
    }
    if selected.host.is_remote() && !selected.host.is_reachable() {
        view.set_message("remote host is unavailable; cached state is not actionable");
        return;
    }
    if selected.runtime_status == NavigatorRuntimeStatus::Working {
        view.begin_archive_confirmation(selected);
        return;
    }
    archive_workstream(root, remote, view, &selected);
}

fn rename_selected(view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no active Workstream is selected");
        return;
    };
    if selected.host.is_remote() && !selected.host.is_reachable() {
        view.set_message("remote host is unavailable; cached state is not actionable");
        return;
    }
    view.begin_rename(selected);
}

fn rename_workstream(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    selected: &NavigatorWorkstream,
    name: &str,
) {
    match run_rename_action(root, selected, name) {
        Ok(()) => {
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("canonical Codex thread title updated");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn archive_workstream(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    selected: &NavigatorWorkstream,
) {
    match run_action(
        root,
        "archive",
        selected,
        Some(selected.workstream_revision.value()),
    ) {
        Ok(()) => {
            view.clear_attached(selected);
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("Workstream archived; provider history and checkout are retained");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn restore_selected(root: &StateRoot, remote: &mut RemoteMonitor, view: &mut NavigatorView) {
    let Some(selected) = view.selected().cloned() else {
        view.set_message("no archived Workstream is selected");
        return;
    };
    if !selected.archived {
        view.set_message("switch to the archived scope to restore this Workstream");
        return;
    }
    match run_action(
        root,
        "restore",
        &selected,
        Some(selected.workstream_revision.value()),
    ) {
        Ok(()) => {
            remote.request_soon(selected.host.alias());
            refresh_view(root, remote, view);
            view.set_message("Workstream restored; select it to start or resume Codex");
        }
        Err(error) => view.set_message(action_message(&error)),
    }
}

fn recover_selected_operation(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    let Some(operation) = view
        .snapshot
        .unresolved_operations
        .get(view.selected_operation)
        .cloned()
    else {
        view.set_message("no unresolved Workstream operation is selected");
        return;
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            view.set_message(action_message(&error));
            return;
        }
    };
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if operation.host.is_remote() {
        command
            .arg("host")
            .arg("recover-operation")
            .arg(operation.host.alias())
            .arg(operation.operation_id.to_string());
    } else {
        command
            .arg("recover-operation")
            .arg(operation.operation_id.to_string());
    }
    match output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process) {
        Ok(output) if output.status.success() => {
            remote.request_soon(operation.host.alias());
            refresh_view(root, remote, view);
            view.dismiss_detail();
            view.set_message("recovery reconciled the exact recorded operation");
        }
        Ok(_) => view.set_message("the exact operation remains unavailable for recovery"),
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
        if view.snapshot.workstreams.is_empty() {
            view.begin_checkout_registration();
        } else {
            view.set_message("select a ProjectLocation to start a new Workstream");
        }
        return;
    };
    create_workstream_from_source(root, presentation, remote, view, &source, action);
}

fn create_workstream_from_selected_project_location(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
) {
    let Some(source) = view.selected_project_location_source() else {
        view.set_message("the selected ProjectLocation is unavailable; refresh this host");
        return;
    };
    create_workstream_from_source(
        root,
        presentation,
        remote,
        view,
        &source,
        CreationAction::Independent,
    );
}

fn create_workstream_from_source(
    root: &StateRoot,
    presentation: &Presentation,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    source: &NavigatorWorkstream,
    action: CreationAction,
) {
    let destination = match run_creation_action(root, action, source) {
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

fn register_checkout(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    view: &mut NavigatorView,
    host: &NavigatorHost,
    checkout: &str,
) {
    if host.is_remote() && !host.is_reachable() {
        view.set_message("remote host is unavailable; checkout registration was not sent");
        return;
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            view.set_message(action_message(&error));
            return;
        }
    };
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if host.is_remote() {
        command
            .arg("host")
            .arg("register-checkout")
            .arg(host.alias())
            .arg(checkout);
    } else {
        command.arg("register").arg(checkout);
    }
    match output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process) {
        Ok(output) if output.status.success() => match parse_created_workstream(&output.stdout) {
            Ok(workstream_id) => {
                remote.request_soon(host.alias());
                refresh_view(root, remote, view);
                if view.select_project_for_workstream(host.alias(), workstream_id) {
                    view.set_message("checkout registered; select this Project to start Codex");
                } else if host.is_remote() {
                    view.set_message(
                        "remote checkout registered; waiting for its bounded snapshot",
                    );
                } else {
                    view.set_message("checkout registration completed; refresh the Project view");
                }
            }
            Err(error) => view.set_message(action_message(&error)),
        },
        Ok(_) => view.set_message("checkout registration is unavailable"),
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

fn run_rename_action(
    root: &StateRoot,
    workstream: &NavigatorWorkstream,
    name: &str,
) -> Result<(), NavigatorError> {
    if workstream.host.is_remote() && !workstream.host.is_reachable() {
        return Err(NavigatorError::RemoteHostUnavailable);
    }
    let executable = std::env::current_exe().map_err(NavigatorError::ActionLaunch)?;
    let mut command = Command::new(executable);
    command.arg("--state-root").arg(root.base());
    if workstream.host.is_remote() {
        command
            .arg("host")
            .arg("rename")
            .arg(workstream.host.alias())
            .arg(workstream.workstream_id.to_string())
            .arg(workstream.workstream_revision.value().to_string())
            .arg(name);
    } else {
        command
            .arg("rename")
            .arg(workstream.workstream_id.to_string())
            .arg(workstream.workstream_revision.value().to_string())
            .arg(name);
    }
    let output =
        output_bounded(&mut command, 1024, 1024).map_err(NavigatorError::from_action_process)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(NavigatorError::ActionFailed)
    }
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
    fn workstream_scope_filters_rows_and_keeps_selection_in_scope() {
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

        view.cycle_workstream_scope();
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

        view.cycle_workstream_scope();
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
        assert!(rendered.contains("Runtime: working"));
        assert!(rendered.contains("Visibility: active"));
        assert!(!rendered.contains(&workstream_id.to_string()));
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
    fn checkout_registration_uses_a_navigator_local_host_picker_and_path_entry() {
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: Vec::new(),
            hosts: vec![
                NavigatorHostOverview {
                    alias: "local".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                NavigatorHostOverview {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
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
        view.modal = Some(NavigatorModal::RegisterCheckout {
            host: hosts[selected].clone(),
            value: "/private/checkout".to_owned(),
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
        assert!(rendered.contains("Register remote checkout"));
        assert!(rendered.contains("snap"));
        assert!(rendered.contains("/private/checkout"));
        assert!(!rendered.contains("provider pane"));
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
    fn row_mapping_covers_both_lines_of_each_workstream() {
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
        assert_eq!(view.row_from_y(1), None);
        assert_eq!(view.row_from_y(2), Some(0));
        assert_eq!(view.row_from_y(3), Some(0));
        assert_eq!(view.row_from_y(4), Some(1));
        assert_eq!(view.row_from_y(5), Some(1));
        assert_eq!(view.row_from_y(6), None);
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
        let (indicator, style) = status_indicator(&row, 0);

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
    fn recent_context_uses_one_neutral_separator_between_color_axes() {
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

        let line =
            workstream_context_line(row, WorkstreamRowContext::Recent, "   ", &project_colors);

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

        assert!(rendered.contains("operation needs recovery"));
        assert!(rendered.contains("No Workstreams yet"));
        assert!(rendered.contains("? keys"));
    }

    #[test]
    fn recovery_page_shows_only_bounded_operation_identity_and_reconciles_selection() {
        let local_operation_id = OperationId::new();
        let remote_operation_id = OperationId::new();
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: Vec::new(),
            hosts: vec![NavigatorHostOverview {
                alias: "snap".to_owned(),
                reachability: RemoteHostReachability::Reachable,
            }],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 2,
            unresolved_operations: vec![
                NavigatorOperation {
                    host: NavigatorHost::Local,
                    operation_id: local_operation_id,
                    kind: OperationKind::Start,
                    phase: OperationPhase::AwaitingReconciliation,
                    revision: Revision::INITIAL,
                },
                NavigatorOperation {
                    host: NavigatorHost::Remote {
                        alias: "snap".to_owned(),
                        reachability: RemoteHostReachability::Reachable,
                    },
                    operation_id: remote_operation_id,
                    kind: OperationKind::Fork,
                    phase: OperationPhase::RecoveryRequired,
                    revision: Revision::INITIAL.next(),
                },
            ],
        });
        view.open_recovery();
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

        terminal.draw(|frame| view.render(frame)).unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Recovery"));
        assert!(rendered.contains("local · Start · awaiting reconciliation"));
        assert!(rendered.contains("snap · Fork · recovery required"));
        assert!(!rendered.contains(&local_operation_id.to_string()));
        assert!(!rendered.contains(&remote_operation_id.to_string()));
        assert_eq!(view.selected_host_alias(), Some("local"));

        view.select_next();
        assert_eq!(view.selected_operation, 1);
        assert_eq!(view.selected_host_alias(), Some("snap"));

        view.replace_snapshot(LocalNavigatorSnapshot::default());
        assert_eq!(view.detail, None);
    }

    #[test]
    fn empty_navigator_requires_an_explicit_checkout_registration() {
        let view = NavigatorView::new(LocalNavigatorSnapshot::default());

        assert_eq!(
            view.footer_status(),
            "No Workstreams yet · n registers a checkout"
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
        view.cycle_view_mode();
        assert_eq!(view.view_mode(), NavigatorViewMode::Project);
        assert!(view.footer_status().contains("view: By project"));
        view.cycle_view_mode();
        assert_eq!(view.view_mode(), NavigatorViewMode::Host);
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
            hosts: Vec::new(),
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
            unresolved_operations: Vec::new(),
        });

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
            false,
            WorkstreamScope::Active,
        )
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
        assert!(full_help.contains("click a row to select"));
        assert!(full_help.contains("cycle recent/project/host"));
        assert!(full_help.contains("recover an unresolved Start or Fork"));
        assert!(full_help.contains("close keys"));
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
                },
                NavigatorHostOverview {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                NavigatorHostOverview {
                    alias: "spare".to_owned(),
                    reachability: RemoteHostReachability::Unreachable,
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

        view.open_selected_detail();
        assert_eq!(view.detail, Some(NavigatorDetail::Project(project_id)));
        assert!(view.dismiss_detail());

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
        assert_eq!(view.detail, Some(NavigatorDetail::Host("spare".to_owned())));
        assert!(view.is_attached_to(&view.snapshot.workstreams[0]));
    }

    #[test]
    fn project_detail_selects_host_owned_locations_without_rendering_opaque_ids() {
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
            }],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
            unresolved_operations: Vec::new(),
        });
        view.select_page(NavigatorPage::Projects);
        view.open_selected_detail();

        let project = view.projects().pop().unwrap();
        assert_eq!(project.active_workstream_count, 1);
        assert_eq!(project.archived_workstream_count, 2);
        assert_eq!(project.locations.len(), 2);
        assert_eq!(
            view.selected_project_location_source()
                .map(|source| source.workstream_id),
            Some(local_active)
        );
        view.select_next();
        assert_eq!(
            view.selected_project_location_source()
                .map(|source| source.workstream_id),
            Some(remote_archived)
        );

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
        assert!(rendered.contains("local · main checkout"));
        assert!(rendered.contains("snap · remote checkout"));
        assert!(rendered.contains("n start selected location"));
        assert!(!rendered.contains(&local_location.to_string()));
        assert!(!rendered.contains(&remote_location.to_string()));
    }

    #[test]
    fn management_page_mouse_targets_cover_tabs_and_two_line_rows() {
        let mut view = NavigatorView::new(LocalNavigatorSnapshot {
            workstreams: vec![row(WorkstreamId::new(), NavigatorRuntimeStatus::Idle)],
            hosts: vec![
                NavigatorHostOverview {
                    alias: "local".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
                NavigatorHostOverview {
                    alias: "snap".to_owned(),
                    reachability: RemoteHostReachability::Reachable,
                },
            ],
            unreachable_hosts: Vec::new(),
            unresolved_operation_count: 0,
            unresolved_operations: Vec::new(),
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

        terminal.draw(|frame| view.render(frame)).unwrap();
        let projects_tab = view
            .rendered_page_tabs
            .iter()
            .find(|(_, page)| *page == NavigatorPage::Projects)
            .map(|(area, _)| *area)
            .unwrap();
        assert_eq!(
            view.page_from_position(projects_tab.x, projects_tab.y),
            Some(NavigatorPage::Projects)
        );

        view.select_page(NavigatorPage::Projects);
        terminal.draw(|frame| view.render(frame)).unwrap();
        assert_eq!(view.project_from_y(2), view.selected_project);
        assert_eq!(view.project_from_y(3), view.selected_project);
        view.begin_project_click(view.project_from_y(3));
        assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Project));

        view.select_page(NavigatorPage::Hosts);
        terminal.draw(|frame| view.render(frame)).unwrap();
        assert_eq!(view.host_from_y(2).as_deref(), Some("local"));
        assert_eq!(view.host_from_y(3).as_deref(), Some("local"));
        view.begin_host_click(view.host_from_y(3));
        assert_eq!(view.take_mouse_click(), Some(MouseClickIntent::Host));
    }

    #[test]
    fn compact_and_expanded_controls_preserve_terminal_key_memory() {
        let mut view = NavigatorView::new(LocalNavigatorSnapshot::default());
        let compact = view
            .compact_key_lines(80)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
            .collect::<String>();
        assert!(compact.contains("Enter open"));
        assert!(compact.contains("n register"));
        assert!(compact.contains("? keys"));
        assert!(!compact.contains("No Workstreams"));
        assert_eq!(
            view.footer_status(),
            "No Workstreams yet · n registers a checkout"
        );

        view.select_page(NavigatorPage::Projects);
        let project_compact = view
            .compact_key_lines(80)
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
            .collect::<String>();
        assert!(project_compact.contains("Enter details"));
        assert!(project_compact.contains("1 workstreams"));
        assert!(!project_compact.contains("n new"));

        let expanded = help_lines(
            NavigatorPage::Workstreams,
            false,
            false,
            false,
            WorkstreamScope::Active,
        );
        assert!(expanded.iter().all(|line| {
            line.spans
                .iter()
                .filter(|span| span.style.fg == Some(Color::Yellow))
                .count()
                <= 1
        }));
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
                unresolved_operations: Vec::new(),
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
                unresolved_operations: Vec::new(),
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
            location_id: LocationId::new(),
            workstream_id,
            project_label: "project".to_owned(),
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
}
