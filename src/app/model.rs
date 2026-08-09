use super::{
    Error, FromStr, OperationId, PathBuf, Revision, RuntimeProbe, StateError, WorkstreamId, env,
};

pub(super) const fn runtime_probe_label(probe: &RuntimeProbe) -> &'static str {
    match probe {
        RuntimeProbe::Live { .. } => "live",
        RuntimeProbe::Missing => "missing",
        RuntimeProbe::Unknown { .. } => "unknown",
    }
}

pub(super) fn parse_workstream(value: &str) -> Result<WorkstreamId, AppError> {
    WorkstreamId::from_str(value).map_err(AppError::InvalidWorkstreamId)
}

pub(super) fn parse_optional_provider(
    value: Option<&str>,
) -> Result<Option<crate::domain::ProviderKind>, AppError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|error| AppError::State(StateError::Domain(error)))
        })
        .transpose()
}

pub(super) fn parse_operation(value: &str) -> Result<OperationId, AppError> {
    OperationId::from_str(value).map_err(AppError::InvalidOperationId)
}

pub(super) fn parse_revision(value: i64) -> Result<Revision, AppError> {
    Revision::try_from(value).map_err(|_| AppError::InvalidWorkstreamRevision)
}

pub(super) fn default_state_root() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".wsnav-state"))
        .join("wsnav")
}

/// User-facing local-command failures.
#[derive(Debug, Error)]
pub(crate) enum AppError {
    #[error("native tmux attach failed")]
    AttachFailed,
    #[error("attention revision is invalid")]
    InvalidAttentionRevision,
    #[error("invalid workstream ID")]
    InvalidWorkstreamId(uuid::Error),
    #[error("invalid operation ID")]
    InvalidOperationId(uuid::Error),
    #[error("workstream revision is invalid")]
    InvalidWorkstreamRevision,
    #[error("invalid runtime ID")]
    InvalidRuntimeId(uuid::Error),
    #[error("invalid provider attachment attempt")]
    InvalidAttachmentAttempt(uuid::Error),
    #[error("host alias is not registered")]
    UnknownHostAlias,
    #[error("host alias is not an SSH host")]
    HostIsNotSsh,
    #[error("remote executable path is not valid UTF-8")]
    RemoteExecutableNotUtf8,
    #[error("remote Workstream has no live Runtime to attach")]
    RemoteRuntimeUnavailable,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("native provider exec failed")]
    RuntimeExec(std::io::Error),
    #[cfg(not(unix))]
    #[error("native provider exited during the internal launch handoff")]
    RuntimeExited,
    #[error("workstream {0} has no runtime")]
    NoRuntime(WorkstreamId),
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("observer profile is not installed; open wsnav to activate it")]
    ObserverNotInstalled,
    #[error(
        "native hook trust remains pending; open wsnav and approve the exact observer hooks in Codex"
    )]
    NativeTrustReviewIncomplete,
    #[error("observer profile removal is refused while a managed runtime is live")]
    LiveRuntimePreventsRemoval,
    #[error("observer profile update is refused while a managed runtime is live")]
    LiveRuntimePreventsUpdate,
    #[error("observer activation is refused while a managed runtime is live")]
    LiveRuntimePreventsObserverActivation,
    #[error(transparent)]
    Repository(#[from] crate::repository::RepositoryError),
    #[error(transparent)]
    BuildInfo(#[from] crate::build_info::BuildInfoError),
    #[error(transparent)]
    Profile(#[from] crate::provider::codex::profile::ProfileError),
    #[error(transparent)]
    Provider(#[from] crate::provider::ProviderReadinessError),
    #[error(transparent)]
    ProviderSelection(#[from] crate::provider::ProviderSelectionError),
    #[error(transparent)]
    Domain(#[from] crate::domain::DomainError),
    #[error(transparent)]
    OpenCode(#[from] crate::provider::opencode::OpenCodeError),
    #[error(transparent)]
    OpenCodeObserver(#[from] crate::provider::opencode::OpenCodeObserverError),
    #[error(transparent)]
    AppServer(#[from] crate::provider::codex::app_server::AppServerError),
    #[error(transparent)]
    Navigator(#[from] crate::navigator::NavigatorError),
    #[error(transparent)]
    Presentation(#[from] crate::presentation::PresentationError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    Remote(#[from] crate::remote::RemoteError),
    #[error(transparent)]
    Action(#[from] crate::actions::ActionError),
    #[error(transparent)]
    Transport(#[from] crate::transport::TransportError),
    #[error(transparent)]
    State(#[from] crate::state::StateError),
}
