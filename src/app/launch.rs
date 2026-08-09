use super::{
    AttachmentPhase, ClientCatalog, Command, FromStr, HostRegistry, PathBuf, Presentation,
    ProviderSessionId, RuntimeId, RuntimePaths, StateRoot, await_launch_release,
};
use super::{
    local::attach,
    model::{AppError, parse_workstream},
    remote::{attach_remote_workstream, checked_ssh_endpoint},
};

pub(super) fn runtime_launch(
    root: &StateRoot,
    runtime_id: &str,
    mut program: Vec<std::ffi::OsString>,
) -> Result<(), AppError> {
    let runtime_id = RuntimeId::from_str(runtime_id).map_err(AppError::InvalidRuntimeId)?;
    let paths = RuntimePaths::for_runtime(root.base(), runtime_id);
    await_launch_release(&paths)?;
    let executable = program.remove(0);
    let mut command = Command::new(executable);
    command.args(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(AppError::RuntimeExec(command.exec()))
    }
    #[cfg(not(unix))]
    {
        let status = command.status().map_err(AppError::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(AppError::RuntimeExited)
        }
    }
}

pub(super) struct OpenCodeObserverArguments {
    pub(super) runtime_id: String,
    pub(super) generation: String,
    pub(super) port: u16,
    pub(super) session_id: String,
    pub(super) pane_pid: u32,
    pub(super) cwd: PathBuf,
    pub(super) provider_birth: String,
}

pub(super) fn opencode_observer(
    root: &StateRoot,
    arguments: OpenCodeObserverArguments,
) -> Result<(), AppError> {
    let context = crate::provider::opencode::OpenCodeObserverContext {
        runtime_id: RuntimeId::from_str(&arguments.runtime_id)
            .map_err(AppError::InvalidRuntimeId)?,
        generation: arguments.generation,
        endpoint: crate::provider::opencode::OpenCodeEndpoint::loopback(arguments.port)
            .map_err(AppError::OpenCode)?,
        session: ProviderSessionId::new(
            crate::domain::ProviderKind::OpenCode,
            &arguments.session_id,
        )
        .map_err(AppError::Domain)?,
        pane_pid: arguments.pane_pid,
        cwd: arguments.cwd,
        provider_birth: arguments.provider_birth,
    };
    crate::provider::opencode::run_observer(root, &context).map_err(AppError::OpenCodeObserver)
}

/// Runs an attachment only inside the presentation provider pane.
///
/// A provider pane is reserved for native provider bytes. The navigator refreshes
/// lifecycle state independently, so an unavailable or unexpectedly stopped
/// Runtime must leave this pane blank rather than render a CLI diagnostic.
pub(super) fn provider_attach(
    root: &StateRoot,
    workstream_id: &str,
    presentation_socket: PathBuf,
    presentation_session: String,
    attempt_id: &str,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let attempt_id =
        uuid::Uuid::parse_str(attempt_id).map_err(AppError::InvalidAttachmentAttempt)?;
    presentation.report_attachment_phase(attempt_id, AttachmentPhase::Running)?;
    let outcome = (|| -> Result<(), AppError> {
        let workstream_id = parse_workstream(workstream_id)?;
        let mut registry = HostRegistry::open(root)?;
        attach(root, &mut registry, workstream_id)
    })();
    let phase = if outcome.is_ok() {
        AttachmentPhase::Completed
    } else {
        AttachmentPhase::Failed
    };
    presentation.report_attachment_phase(attempt_id, phase)?;
    provider_wait()
}

/// Runs an SSH attachment only inside the presentation provider pane.
///
/// The remote `_attach` endpoint follows the same no-diagnostics rule, while
/// the navigator's normal polling displays the resulting bounded state.
pub(super) fn provider_remote_attach(
    root: &StateRoot,
    host_alias: &str,
    workstream_id: &str,
    presentation_socket: PathBuf,
    presentation_session: String,
    attempt_id: &str,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let attempt_id =
        uuid::Uuid::parse_str(attempt_id).map_err(AppError::InvalidAttachmentAttempt)?;
    presentation.report_attachment_phase(attempt_id, AttachmentPhase::Running)?;
    let outcome = (|| -> Result<(), AppError> {
        let catalog = ClientCatalog::open(root)?;
        attach_remote_workstream(&catalog, host_alias, workstream_id)
    })();
    let phase = if outcome.is_ok() {
        AttachmentPhase::Completed
    } else {
        AttachmentPhase::Failed
    };
    presentation.report_attachment_phase(attempt_id, phase)?;
    provider_wait()
}

/// Runs the remote observer review only in the presentation provider pane.
/// Codex owns every visible byte. This helper intentionally discards transport
/// diagnostics and returns to the blank pane after the native review exits.
pub(super) fn provider_remote_observer_review(
    root: &StateRoot,
    host_alias: &str,
) -> Result<(), AppError> {
    let _ = (|| -> Result<(), AppError> {
        let catalog = ClientCatalog::open(root)?;
        let endpoint = checked_ssh_endpoint(&catalog, host_alias)?;
        crate::transport::review_observer_ssh(&endpoint)?;
        Ok(())
    })();
    provider_wait()
}

pub(super) fn provider_wait() -> Result<(), AppError> {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
