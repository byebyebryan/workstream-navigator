use std::io;

use ratatui::style::Color;
use thiserror::Error;

use crate::{
    build_info::BuildInfoError,
    domain::{
        LocationId, OperationId, OperationKind, OperationPhase, ProjectId, ProviderKind, Revision,
        WorkstreamId,
    },
    presentation::PresentationError,
    process::BoundedProcessError,
    protocol::{ObserverStatus, ProviderCapability},
    runtime::RuntimeError,
    state::StateError,
    transport::TransportError,
};

/// One bounded row rendered by the local navigator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorWorkstream {
    pub host: NavigatorHost,
    pub project_id: ProjectId,
    /// Opaque host-owned location identity used only for Project actions.
    pub location_id: LocationId,
    pub workstream_id: WorkstreamId,
    pub provider: ProviderKind,
    pub project_label: String,
    /// Credential-free normalized fetch-remote label for display only.
    pub remote_identity_display: Option<String>,
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
    Unreachable(RemoteHostIssue),
}

/// Bounded and credential-free reason why a registered host cannot currently
/// participate in the navigator. This is transport evidence, never a claim
/// about an agent Runtime or the remote machine's broader health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteHostIssue {
    Checking,
    SshOrRemoteExecutableUnavailable,
    TimedOut,
    BuildProbeMalformed,
    ControlAbiMismatch { local: u16, remote: u16 },
    ProtocolMismatch { local: u16, remote: u16 },
    HostSchemaMismatch { local: i64, remote: i64 },
    HostIdentityChanged,
    HostRegistrationStale,
    RemoteRequestRejected,
    ControlCommunicationFailed,
}

impl RemoteHostIssue {
    pub(in crate::navigator) const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::SshOrRemoteExecutableUnavailable
                | Self::TimedOut
                | Self::ControlCommunicationFailed
        )
    }

    pub(in crate::navigator) fn label(self) -> String {
        match self {
            Self::Checking => "checking remote".to_owned(),
            Self::SshOrRemoteExecutableUnavailable => "SSH/wsnav unavailable".to_owned(),
            Self::TimedOut => "remote timed out".to_owned(),
            Self::BuildProbeMalformed => "build probe malformed".to_owned(),
            Self::ControlAbiMismatch { local, remote } => {
                format!("control ABI {remote} ≠ {local}")
            }
            Self::ProtocolMismatch { local, remote } => format!("protocol {remote} ≠ {local}"),
            Self::HostSchemaMismatch { local, remote } => format!("schema {remote} ≠ {local}"),
            Self::HostIdentityChanged => "host identity changed".to_owned(),
            Self::HostRegistrationStale => "host registration stale".to_owned(),
            Self::RemoteRequestRejected => "remote request rejected".to_owned(),
            Self::ControlCommunicationFailed => "control communication failed".to_owned(),
        }
    }

    pub(in crate::navigator) const fn color(self) -> Color {
        if self.is_transient() {
            Color::Yellow
        } else {
            Color::Red
        }
    }
}

impl RemoteHostReachability {
    pub(in crate::navigator) const fn is_reachable(self) -> bool {
        matches!(self, Self::Reachable)
    }
}

impl NavigatorHost {
    pub(in crate::navigator) fn alias(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::Remote { alias, .. } => alias,
        }
    }

    pub(in crate::navigator) const fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    pub(in crate::navigator) const fn is_reachable(&self) -> bool {
        match self {
            Self::Local => true,
            Self::Remote { reachability, .. } => reachability.is_reachable(),
        }
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

pub(in crate::navigator) const fn operation_kind_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Start => "Start",
        OperationKind::Fork => "Fork",
    }
}

pub(in crate::navigator) const fn operation_phase_label(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::Prepared => "prepared",
        OperationPhase::ExternalEffectStarted => "external effect started",
        OperationPhase::AwaitingReconciliation => "awaiting reconciliation",
        OperationPhase::Committed => "committed",
        OperationPhase::RecoveryRequired => "recovery required",
        OperationPhase::Failed => "failed",
    }
}

pub(in crate::navigator) const fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Codex => "Codex",
        ProviderKind::OpenCode => "OpenCode",
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
/// effect evidence remain on the host. A Fork retains only its already-visible
/// source Workstream ID, allowing a repeated Fork to reach its exact recovery
/// path without exposing raw operation details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigatorOperation {
    pub host: NavigatorHost,
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub source_workstream_id: Option<WorkstreamId>,
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
    pub observer_status: ObserverStatus,
    pub provider_capabilities: Vec<ProviderCapability>,
}

impl NavigatorHostOverview {
    /// Cached provider evidence is authoritative only while the host is
    /// reachable. An unreachable host retains records for display but cannot
    /// authorize a new Workstream action.
    #[must_use]
    pub fn provider_is_new_eligible(&self, kind: ProviderKind) -> bool {
        self.reachability.is_reachable()
            && crate::provider::eligible_new_providers(&self.provider_capabilities).contains(&kind)
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
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    BuildInfo(#[from] BuildInfoError),
    #[error(transparent)]
    ProviderSelection(#[from] crate::provider::ProviderSelectionError),
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
    pub(in crate::navigator) fn from_action_process(source: BoundedProcessError) -> Self {
        match source {
            BoundedProcessError::OutputTooLarge => Self::ActionOutputTooLarge,
            other => Self::ActionProcess(other),
        }
    }
}
