use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use crate::{
    build_info::BuildInfoError,
    domain::{AttentionState, HostId, ProjectId, Revision, RuntimeStatus, WorkstreamLifecycle},
    protocol::{ObserverStatus, ProviderCapability, SnapshotResponse},
    provider::InstallationProbeCache,
    provider::names::{NameContext, resolve_name},
    state::{
        ClientCatalog, ClientHost, ClientHostTransport, ClientProjectLocation, HostIdentity,
        HostRegistry, IntegrationLifecycle, StateError, StateRoot, WorkstreamOverview,
    },
    transport::{
        HostClient, RemoteExecutable, SshDestination, SshEndpoint, SystemCommandRunner,
        TransportError,
    },
};

use super::model::{
    LocalNavigatorSnapshot, NavigatorError, NavigatorHost, NavigatorHostOverview,
    NavigatorOperation, NavigatorRuntimeStatus, NavigatorWorkstream, RemoteHostIssue,
    RemoteHostReachability,
};

/// Reads a fresh local-only navigator projection from durable host state.
///
/// The caller controls polling. This projection does not contact a private
/// provider tmux server: exact liveness checks are reserved for recovery and
/// stateful action boundaries so passive rendering cannot disturb the native
/// provider pane.
///
/// # Errors
///
/// Returns an error when the local registry cannot be opened or contains
/// invalid persisted state.
pub fn local_snapshot(root: &StateRoot) -> Result<LocalNavigatorSnapshot, NavigatorError> {
    let installation_cache = InstallationProbeCache::probe();
    local_snapshot_with_installation_cache(root, installation_cache)
}

pub(in crate::navigator) fn local_snapshot_with_installation_cache(
    root: &StateRoot,
    installation_cache: InstallationProbeCache,
) -> Result<LocalNavigatorSnapshot, NavigatorError> {
    let mut registry = HostRegistry::open(root)?;
    crate::repository::refresh_pending_metadata(&mut registry)?;
    let host = registry.identity()?;
    let observer_status = observer_status(&registry)?;
    let provider_capabilities = crate::provider::discover_capabilities_with_installation_cache(
        &registry,
        installation_cache,
    )?;
    let mut catalog = ClientCatalog::open(root)?;
    let executable = std::env::current_exe().map_err(NavigatorError::CurrentExecutable)?;
    let workstreams = registry
        .workstream_overviews()?
        .into_iter()
        .map(|overview| project_workstream(&mut catalog, &host, &executable, &overview))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let unresolved_operations = registry
        .unresolved_operation_overviews()?
        .into_iter()
        .map(|operation| NavigatorOperation {
            host: NavigatorHost::Local,
            operation_id: operation.operation_id,
            kind: operation.kind,
            source_workstream_id: operation.source_workstream_id,
            phase: operation.phase,
            revision: operation.revision,
        })
        .collect::<Vec<_>>();
    Ok(LocalNavigatorSnapshot {
        workstreams,
        hosts: vec![NavigatorHostOverview {
            alias: "local".to_owned(),
            reachability: RemoteHostReachability::Reachable,
            observer_status,
            provider_capabilities,
        }],
        unreachable_hosts: Vec::new(),
        unresolved_operation_count: unresolved_operations.len(),
        unresolved_operations,
    })
}

fn observer_status(registry: &HostRegistry) -> Result<ObserverStatus, NavigatorError> {
    Ok(
        match registry
            .codex_integration()?
            .map(|integration| integration.lifecycle)
        {
            None => ObserverStatus::NotInstalled,
            Some(IntegrationLifecycle::TrustPending) => ObserverStatus::TrustPending,
            Some(IntegrationLifecycle::Ready) => ObserverStatus::Ready,
            Some(IntegrationLifecycle::Modified) => ObserverStatus::Modified,
            Some(IntegrationLifecycle::Disabled) => ObserverStatus::Disabled,
        },
    )
}

fn project_workstream(
    catalog: &mut ClientCatalog,
    host: &HostIdentity,
    executable: &Path,
    overview: &WorkstreamOverview,
) -> Result<Option<NavigatorWorkstream>, NavigatorError> {
    if catalog.project_location_is_ignored(host.host_id, overview.location_id)? {
        return Ok(None);
    }
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
    let attention_revision = acknowledgement_revision(overview.attention.as_ref());
    let runtime_status = if recovery_required {
        NavigatorRuntimeStatus::RecoveryRequired
    } else {
        observed_runtime_status(overview)
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
    Ok(Some(NavigatorWorkstream {
        host: NavigatorHost::Local,
        project_id: project.project_id,
        location_id: overview.location_id,
        workstream_id: overview.workstream_id,
        provider: overview.provider,
        project_label: bounded_display(&project.display_name),
        remote_identity_display: overview
            .remote_identity_display
            .as_deref()
            .map(bounded_display),
        location_label: bounded_display(&overview.project_display_name),
        display_name,
        runtime_status,
        archived: overview.archived_at_millis.is_some(),
        result_ready,
        recovery_required,
        attention_revision,
        last_activity_at_millis: overview.last_activity_at_millis,
        workstream_revision: overview.revision,
    }))
}

/// Returns the current optimistic-lock revision only while an acknowledgement
/// is meaningful. The first unseen-result revision is display metadata; it is
/// deliberately not the mutation authority after newer lifecycle evidence.
pub(in crate::navigator) fn acknowledgement_revision(
    attention: Option<&AttentionState>,
) -> Option<Revision> {
    attention
        .filter(|attention| {
            attention.result_unseen_since_revision.is_some()
                || attention.recovery_unseen_since_revision.is_some()
        })
        .map(|attention| attention.revision)
}

pub(in crate::navigator) fn project_remote_workstream(
    catalog: &mut ClientCatalog,
    host_id: HostId,
    host_alias: &str,
    workstream: &crate::protocol::SnapshotWorkstream,
    host_reachable: bool,
) -> Result<Option<NavigatorWorkstream>, NavigatorError> {
    if catalog.project_location_is_ignored(host_id, workstream.location_id)? {
        return Ok(None);
    }
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
    Ok(Some(NavigatorWorkstream {
        host: NavigatorHost::Remote {
            alias: host_alias.to_owned(),
            reachability: if host_reachable {
                RemoteHostReachability::Reachable
            } else {
                RemoteHostReachability::Unreachable(RemoteHostIssue::ControlCommunicationFailed)
            },
        },
        project_id: project.project_id,
        location_id: workstream.location_id,
        workstream_id: workstream.workstream_id,
        provider: workstream.provider,
        project_label: bounded_display(&project.display_name),
        remote_identity_display: workstream
            .remote_identity_display
            .as_deref()
            .map(bounded_display),
        location_label: bounded_display(&workstream.project_display_name),
        display_name: bounded_display(&workstream.display_name),
        runtime_status,
        archived: workstream.archived,
        result_ready: workstream.result_ready,
        recovery_required: workstream.recovery_required,
        attention_revision,
        last_activity_at_millis: workstream.last_activity_at_millis,
        workstream_revision,
    }))
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

fn observed_runtime_status(overview: &WorkstreamOverview) -> NavigatorRuntimeStatus {
    if overview.lifecycle == WorkstreamLifecycle::Parked {
        return NavigatorRuntimeStatus::Parked;
    }
    let Some(record) = &overview.runtime else {
        return NavigatorRuntimeStatus::Parked;
    };
    if record.status == RuntimeStatus::Stopped {
        return NavigatorRuntimeStatus::Unknown;
    }
    navigator_runtime_status(record.status)
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

pub(in crate::navigator) fn bounded_display(value: &str) -> String {
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
pub(in crate::navigator) const REMOTE_INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const REMOTE_MAX_BACKOFF: Duration = Duration::from_secs(30);
pub(in crate::navigator) const MAX_NAVIGATOR_TEXT_INPUT_BYTES: usize = 4096;
pub(in crate::navigator) const PROJECT_BROWSER_VIEWPORT_ROWS: usize = 10;

/// Non-durable client presentation state for bounded asynchronous SSH refresh.
/// It retains the last accepted snapshot if a host becomes unavailable; an SSH
/// disconnect is never projected as a provider stop or attention clear.
pub(in crate::navigator) struct RemoteMonitor {
    pub(in crate::navigator) sender: Sender<RemotePollResult>,
    pub(in crate::navigator) receiver: Receiver<RemotePollResult>,
    pub(in crate::navigator) hosts: BTreeMap<String, CachedRemoteHost>,
    installation_cache: Option<InstallationProbeCache>,
}

pub(in crate::navigator) struct CachedRemoteHost {
    pub(in crate::navigator) workstreams: Vec<NavigatorWorkstream>,
    pub(in crate::navigator) unresolved_operation_count: usize,
    pub(in crate::navigator) unresolved_operations: Vec<NavigatorOperation>,
    pub(in crate::navigator) observer_status: ObserverStatus,
    pub(in crate::navigator) provider_capabilities: Vec<ProviderCapability>,
    pub(in crate::navigator) reachability: RemoteHostReachability,
    pub(in crate::navigator) pending: bool,
    pub(in crate::navigator) next_poll: Instant,
    pub(in crate::navigator) backoff: Duration,
}

pub(in crate::navigator) struct RemotePollResult {
    pub(in crate::navigator) alias: String,
    pub(in crate::navigator) host_id: HostId,
    pub(in crate::navigator) outcome: Result<
        (
            crate::protocol::SnapshotResponse,
            crate::protocol::OperationsResponse,
        ),
        RemoteHostIssue,
    >,
}

impl RemoteMonitor {
    pub(in crate::navigator) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            hosts: BTreeMap::new(),
            installation_cache: None,
        }
    }

    pub(in crate::navigator) fn set_installation_cache(
        &mut self,
        installation_cache: InstallationProbeCache,
    ) {
        self.installation_cache = Some(installation_cache);
    }

    pub(in crate::navigator) fn refresh(
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
                    observer_status: ObserverStatus::NotInstalled,
                    provider_capabilities: SnapshotResponse::default().provider_capabilities,
                    reachability: RemoteHostReachability::Unreachable(RemoteHostIssue::Checking),
                    pending: false,
                    next_poll: now,
                    backoff: REMOTE_INITIAL_BACKOFF,
                });
            if entry.reachability.is_reachable()
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

    pub(in crate::navigator) fn collect(
        &mut self,
        now: Instant,
        catalog: &mut ClientCatalog,
    ) -> Result<(), NavigatorError> {
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
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                host.unresolved_operation_count = usize::from(snapshot.unresolved_operation_count);
                host.observer_status = snapshot.observer_status;
                host.provider_capabilities = snapshot.provider_capabilities;
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
                                source_workstream_id: operation.source_workstream_id,
                                phase: operation.phase,
                                revision,
                            }
                        })
                    })
                    .collect();
                host.reachability = RemoteHostReachability::Reachable;
                host.backoff = REMOTE_INITIAL_BACKOFF;
                host.next_poll = now + REMOTE_POLL_INTERVAL;
            } else if let Err(issue) = result.outcome {
                host.reachability = RemoteHostReachability::Unreachable(issue);
                host.next_poll = now + host.backoff;
                host.backoff = host.backoff.saturating_mul(2).min(REMOTE_MAX_BACKOFF);
            }
        }
        Ok(())
    }

    pub(in crate::navigator) fn combine(
        &self,
        mut local: LocalNavigatorSnapshot,
    ) -> LocalNavigatorSnapshot {
        for (alias, host) in &self.hosts {
            local.hosts.push(NavigatorHostOverview {
                alias: alias.clone(),
                reachability: host.reachability,
                observer_status: host.observer_status,
                provider_capabilities: host.provider_capabilities.clone(),
            });
            local
                .workstreams
                .extend(host.workstreams.iter().cloned().map(|mut workstream| {
                    workstream.host = NavigatorHost::Remote {
                        alias: alias.clone(),
                        reachability: host.reachability,
                    };
                    workstream
                }));
            if !host.reachability.is_reachable() {
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

    pub(in crate::navigator) fn remove_project(&mut self, project_id: ProjectId) {
        for host in self.hosts.values_mut() {
            host.workstreams
                .retain(|workstream| workstream.project_id != project_id);
        }
    }

    pub(in crate::navigator) fn request_soon(&mut self, host_alias: &str) {
        if let Some(host) = self.hosts.get_mut(host_alias) {
            host.next_poll = Instant::now();
        }
    }
}

/// Orders the combined client view by the same cross-host activity age it
/// displays. Per-host activity sequences remain authoritative only inside
/// their own durable host registry, so they cannot order rows from two hosts.
pub(in crate::navigator) fn compare_workstream_activity(
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
    RemoteHostIssue,
> {
    let ClientHostTransport::Ssh { destination } = &host.transport else {
        return Err(RemoteHostIssue::HostRegistrationStale);
    };
    let destination =
        SshDestination::parse(destination).map_err(|_| RemoteHostIssue::HostRegistrationStale)?;
    let executable = host
        .executable_path
        .to_str()
        .ok_or(RemoteHostIssue::HostRegistrationStale)
        .and_then(|value| {
            RemoteExecutable::parse(value).map_err(|_| RemoteHostIssue::HostRegistrationStale)
        })?;
    let endpoint = SshEndpoint::new(destination, executable);
    let client = HostClient::new(SystemCommandRunner);
    client
        .probe_ssh(&endpoint)
        .map_err(|error| remote_probe_issue(&error))?
        .ensure_compatible_with_local()
        .map_err(|error| remote_build_issue(&error))?;
    let hello = client
        .hello_ssh(&endpoint, "wsnav")
        .map_err(|error| remote_control_issue(&error))?;
    host.verify_hello(&hello)
        .map_err(|error| remote_registration_issue(&error))?;
    let snapshot = client
        .snapshot_ssh(&endpoint)
        .map_err(|error| remote_control_issue(&error))?;
    let operations = client
        .operations_ssh(&endpoint)
        .map_err(|error| remote_control_issue(&error))?;
    Ok((snapshot, operations))
}

fn remote_probe_issue(error: &TransportError) -> RemoteHostIssue {
    match error {
        TransportError::ReleaseProbeUnavailable | TransportError::Launch(_) => {
            RemoteHostIssue::SshOrRemoteExecutableUnavailable
        }
        TransportError::TimedOut => RemoteHostIssue::TimedOut,
        TransportError::ReleaseProbeMalformed => RemoteHostIssue::BuildProbeMalformed,
        _ => RemoteHostIssue::ControlCommunicationFailed,
    }
}

pub(in crate::navigator) fn remote_build_issue(error: &BuildInfoError) -> RemoteHostIssue {
    match error {
        BuildInfoError::ControlAbiMismatch { local, remote } => {
            RemoteHostIssue::ControlAbiMismatch {
                local: *local,
                remote: *remote,
            }
        }
        BuildInfoError::ProtocolVersionMismatch { local, remote } => {
            RemoteHostIssue::ProtocolMismatch {
                local: *local,
                remote: *remote,
            }
        }
        BuildInfoError::HostSchemaVersionMismatch { local, remote } => {
            RemoteHostIssue::HostSchemaMismatch {
                local: *local,
                remote: *remote,
            }
        }
        _ => RemoteHostIssue::BuildProbeMalformed,
    }
}

fn remote_registration_issue(error: &StateError) -> RemoteHostIssue {
    match error {
        StateError::ClientHostIdentityMismatch => RemoteHostIssue::HostIdentityChanged,
        StateError::ClientHostGenerationMismatch | StateError::ClientHostCapabilitiesMismatch => {
            RemoteHostIssue::HostRegistrationStale
        }
        _ => RemoteHostIssue::ControlCommunicationFailed,
    }
}

fn remote_control_issue(error: &TransportError) -> RemoteHostIssue {
    match error {
        TransportError::TimedOut => RemoteHostIssue::TimedOut,
        TransportError::Rejected(_) => RemoteHostIssue::RemoteRequestRejected,
        _ => RemoteHostIssue::ControlCommunicationFailed,
    }
}

pub(in crate::navigator) fn combined_snapshot(
    root: &StateRoot,
    remote: &mut RemoteMonitor,
    selected_host: Option<&str>,
) -> Result<LocalNavigatorSnapshot, NavigatorError> {
    let local = match remote.installation_cache.as_ref() {
        Some(installation_cache) => {
            local_snapshot_with_installation_cache(root, *installation_cache)?
        }
        None => local_snapshot(root)?,
    };
    let mut catalog = ClientCatalog::open(root)?;
    remote.refresh(&mut catalog, selected_host)?;
    Ok(remote.combine(local))
}
