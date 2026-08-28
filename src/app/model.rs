use super::{Error, FromStr, OperationId, PathBuf, Revision, StateError, WorkstreamId, env};

pub(super) fn parse_workstream(value: &str) -> Result<WorkstreamId, AppError> {
    WorkstreamId::from_str(value).map_err(AppError::InvalidWorkstreamId)
}

pub(super) fn parse_provider(value: &str) -> Result<crate::domain::ProviderKind, AppError> {
    value
        .parse()
        .map_err(|error| AppError::State(StateError::Domain(error)))
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
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("native provider exec failed")]
    RuntimeExec(std::io::Error),
    #[cfg(not(unix))]
    #[error("native provider exited during the internal launch handoff")]
    RuntimeExited,
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("observer profile is not installed; open wsnav to activate it")]
    ObserverNotInstalled,
    #[error("observer profile removal is refused while a managed runtime is live")]
    LiveRuntimePreventsRemoval,
    #[error(transparent)]
    Repository(#[from] crate::repository::RepositoryError),
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
    D17Navigator(#[from] crate::navigator::D17NavigatorError),
    #[error(transparent)]
    Presentation(#[from] crate::presentation::PresentationError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    Action(#[from] crate::actions::ActionError),
    #[error(transparent)]
    State(#[from] crate::state::StateError),
    #[error(transparent)]
    Startup(#[from] crate::startup::StartupError),
    #[error(transparent)]
    Cutover(#[from] crate::cutover::CutoverError),
    #[error("the D17 account-shell command is explicitly unmanaged")]
    D17ShellGateUnmanaged,
    #[error("the D17 Codex observer requires interactive setup")]
    D17ObserverReadinessRequired,
    #[error("the D17 account-shell command is unavailable")]
    D17ShellControlUnavailable,
    #[error("the D17 runtime attachment is unavailable")]
    D17AttachmentUnavailable,
    #[error("close the existing Navigator presentation before D17 cutover")]
    D17CutoverNeedsPresentationClosed,
}
