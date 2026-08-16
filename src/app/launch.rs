use super::{
    AttachmentPhase, ClientCatalog, Command, FromStr, HostRegistry, PathBuf, Presentation,
    ProviderSessionId, RemoteExecutable, RuntimeId, RuntimePaths, SshDestination, SshEndpoint,
    StateRoot, await_launch_release, env,
};
use super::{
    local::attach,
    model::{AppError, parse_workstream},
    remote::{attach_remote_workstream, checked_ssh_endpoint},
};
use crate::presentation::{PresentationAction, PresentationError, PresentationPaneRole};
use std::path::Path;

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

/// Runs one fixed local presentation action. The helper receives only values
/// emitted by the private presentation key table; it never evaluates an
/// arbitrary tmux command or shell fragment.
pub(super) fn presentation_control(
    root: &StateRoot,
    presentation_socket: PathBuf,
    presentation_session: String,
    action: &str,
    source_pane: &str,
    client_name: &str,
) -> Result<(), AppError> {
    let action = PresentationAction::from_str(action)?;
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let result = match action {
        PresentationAction::CreateOrFocusShell => {
            create_presentation_shell(root, &presentation, source_pane)
        }
        PresentationAction::LiteralCtrlB => {
            send_presentation_literal_ctrl_b(root, &presentation, source_pane)
        }
        other => presentation
            .control_with_client(other, source_pane, Some(client_name))
            .map_err(AppError::from),
    };
    if result.is_err() {
        let _ = presentation
            .show_guidance("Presentation action unavailable; exact owned state required");
    }
    result
}

/// Runs the fixed utility-shell launch barrier inside the newly-created pane.
/// The barrier disables retention through the exact private presentation
/// socket before replacing itself with the ordinary interactive shell.
pub(super) fn presentation_shell(
    root: &StateRoot,
    presentation_socket: PathBuf,
    presentation_session: String,
    shell: PathBuf,
    cwd: PathBuf,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let pane = env::var("TMUX_PANE")
        .map_err(|_| PresentationError::ControlRefused("utility pane identity is unavailable"))?;
    presentation.prepare_utility_pane(&pane)?;
    if !cwd.is_dir() {
        return Err(
            PresentationError::ControlRefused("registered project root is unavailable").into(),
        );
    }
    let mut command = Command::new(shell);
    command.current_dir(cwd).arg("-i");
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

/// Runs the local remote-SSH launch barrier inside the newly-created utility
/// pane. It arms the exact pane before starting SSH, so a failed connection
/// exits and is not retained by the presentation tmux server.
pub(super) fn presentation_remote_shell(
    root: &StateRoot,
    presentation_socket: PathBuf,
    presentation_session: String,
    destination: &str,
    executable: &Path,
    workstream_id: &str,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let destination = SshDestination::parse(destination)?;
    let executable = executable
        .to_str()
        .ok_or(AppError::RemoteExecutableNotUtf8)
        .and_then(|value| RemoteExecutable::parse(value).map_err(AppError::Transport))?;
    let workstream_id = parse_workstream(workstream_id)?;
    let pane = env::var("TMUX_PANE")
        .map_err(|_| PresentationError::ControlRefused("utility pane identity is unavailable"))?;
    presentation.prepare_utility_pane(&pane)?;
    crate::transport::attach_presentation_shell_ssh(
        &SshEndpoint::new(destination, executable),
        workstream_id,
    )?;
    Ok(())
}

fn create_presentation_shell(
    root: &StateRoot,
    presentation: &Presentation,
    source_pane: &str,
) -> Result<(), AppError> {
    if presentation.focus_existing_utility_if_present(source_pane)? {
        return Ok(());
    }
    let status = presentation
        .attachment_status()?
        .ok_or(PresentationError::ControlRefused(
            "a Running attachment is required",
        ))?;
    if status.phase != AttachmentPhase::Running {
        return Err(PresentationError::ControlRefused("a Running attachment is required").into());
    }
    presentation.validate_provider_context(status.workstream_id, &status.host_alias)?;
    if status.host_alias != "local" {
        let catalog = ClientCatalog::open(root)?;
        let endpoint = checked_ssh_endpoint(&catalog, &status.host_alias)?;
        presentation.create_or_focus_remote_shell(
            source_pane,
            &status.host_alias,
            status.workstream_id,
            endpoint.destination.as_str(),
            endpoint.executable.as_str(),
        )?;
        return Ok(());
    }
    let mut registry = HostRegistry::open(root)?;
    let runtime = crate::actions::preflight_attachment(root, &mut registry, status.workstream_id)?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == status.workstream_id)
        .ok_or(PresentationError::ControlRefused(
            "the registered Workstream is unavailable",
        ))?;
    if overview.archived_at_millis.is_some()
        || overview.project_repository_path != runtime.cwd
        || !runtime.cwd.is_dir()
    {
        return Err(PresentationError::ControlRefused(
            "the registered project root is unavailable",
        )
        .into());
    }
    let shell = ordinary_interactive_shell()?;
    presentation.create_or_focus_shell(
        source_pane,
        &status.host_alias,
        status.workstream_id,
        &runtime.cwd,
        &shell,
    )?;
    Ok(())
}

fn send_presentation_literal_ctrl_b(
    root: &StateRoot,
    presentation: &Presentation,
    source_pane: &str,
) -> Result<(), AppError> {
    if presentation.focused_pane_role(source_pane)? != PresentationPaneRole::Provider {
        presentation.send_outer_literal_c_b(source_pane)?;
        return Ok(());
    }
    let status = presentation
        .attachment_status()?
        .ok_or(PresentationError::ControlRefused(
            "provider literal input requires a Running attachment",
        ))?;
    if status.phase != AttachmentPhase::Running {
        return Err(PresentationError::ControlRefused(
            "provider literal input requires a Running attachment",
        )
        .into());
    }
    presentation.validate_provider_context(status.workstream_id, &status.host_alias)?;
    if status.host_alias != "local" {
        let catalog = ClientCatalog::open(root)?;
        let endpoint = checked_ssh_endpoint(&catalog, &status.host_alias)?;
        crate::transport::send_remote_literal_ctrl_b(&endpoint, status.workstream_id)?;
        return Ok(());
    }
    let mut registry = HostRegistry::open(root)?;
    let runtime_record =
        crate::actions::preflight_attachment(root, &mut registry, status.workstream_id)?;
    let tmux = super::SystemTmux::default();
    let process_probe = super::LinuxProcessProbe;
    let runtime = super::PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(
            root.base(),
            runtime_record.runtime_id,
            &runtime_record.tmux_session,
        )?,
    );
    runtime.send_literal_ctrl_b()?;
    Ok(())
}

fn ordinary_interactive_shell() -> Result<PathBuf, AppError> {
    let shell = env::var_os("SHELL").unwrap_or_else(|| std::ffi::OsString::from("/bin/sh"));
    validate_interactive_shell(&PathBuf::from(shell))
}

fn validate_interactive_shell(path: &Path) -> Result<PathBuf, AppError> {
    let valid_path = path.is_absolute()
        && path.to_str().is_some_and(|value| {
            !value.is_empty() && !value.chars().any(|c| c.is_control() || c.is_whitespace())
        });
    #[cfg(unix)]
    let executable = valid_path
        && path.is_file()
        && std::fs::metadata(path).is_ok_and(|metadata| {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        });
    #[cfg(not(unix))]
    let executable = valid_path && path.is_file();
    if !executable {
        return Err(PresentationError::ControlRefused("ordinary shell path is invalid").into());
    }
    Ok(path.to_path_buf())
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

#[cfg(test)]
mod tests {
    use super::validate_interactive_shell;
    use std::path::Path;

    #[test]
    fn ordinary_shell_requires_an_executable_file() {
        let current = std::env::current_exe().unwrap();
        assert!(validate_interactive_shell(&current).is_ok());
        assert!(validate_interactive_shell(Path::new("/tmp")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let temporary = tempfile::tempdir().unwrap();
            let file = temporary.path().join("not-executable");
            std::fs::write(&file, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(validate_interactive_shell(&file).is_err());
        }
    }
}
