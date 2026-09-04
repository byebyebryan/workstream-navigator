use super::model::{AppError, parse_revision, parse_workstream};
use super::{
    AttachmentPhase, Command, FromStr, LinuxProcessProbe, PathBuf, Presentation, PrivateRuntime,
    ProviderSessionId, Revision, RuntimeId, RuntimePaths, StateRoot, Stdio, await_launch_release,
};
use crate::presentation::{
    AttachmentPurpose, PresentationAction, PresentationError, PresentationPaneRole,
};
use crate::{
    domain::{RuntimeStatus, WorkstreamLifecycle},
    navigator::view::workstreams_in_visual_order,
    snapshot::read_snapshot,
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

/// Runs one fixed presentation action. The helper receives only values
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
    presentation.validate_presentation_client(client_name)?;
    match action {
        PresentationAction::LiteralCtrlB => {
            send_presentation_literal_ctrl_b(root, &presentation, source_pane)
        }
        PresentationAction::SwitchPrevious | PresentationAction::SwitchNext => {
            switch_provider_workstream(root, &presentation, action, source_pane, client_name)
        }
        other => presentation
            .control_with_client(other, source_pane, Some(client_name))
            .map_err(AppError::from),
    }
}

/// Read-only synchronous predicate used by the presentation's primary-button
/// tmux binding. A non-zero exit status deliberately suppresses both pane
/// selection and native mouse delivery in tmux.
pub(super) fn presentation_mouse_validate(
    root: &StateRoot,
    presentation_socket: PathBuf,
    presentation_session: String,
    target_pane: &str,
    client_name: &str,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    presentation.context()?;
    presentation.validate_mouse_press(target_pane, client_name)?;
    Ok(())
}

/// Selects one adjacent already-live Workstream from the provider pane. The
/// complete operation is serialized by the presentation attachment claim;
/// the helper receives only exact bounded IDs/revisions and the purpose-tagged
/// status needed for one Navigator selection synchronization.
fn switch_provider_workstream(
    root: &StateRoot,
    presentation: &Presentation,
    action: PresentationAction,
    source_pane: &str,
    client_name: &str,
) -> Result<(), AppError> {
    // A client that is not attached to this exact presentation is never a
    // safe guidance target. This read-only proof also ensures that a later
    // refusal cannot retarget a message at the Navigator pane or another
    // tmux client.
    presentation.validate_presentation_client(client_name)?;
    let result = presentation.with_attachment_claim(|| {
        // Keep every volatile read and the outer-pane replacement inside the
        // same claim. The client is revalidated after the claim is acquired so
        // a detached/reattached client cannot authorize a stale action.
        presentation.validate_presentation_client(client_name)?;
        presentation.validate_focused_provider(source_pane)?;
        let status = presentation.attachment_status_read_only()?.ok_or(
            PresentationError::ControlRefused("provider switching requires a live attachment"),
        )?;
        if status.phase != AttachmentPhase::Running {
            return Err(PresentationError::ControlRefused(
                "provider switching requires a live attachment",
            ));
        }
        presentation.validate_provider_context(status.workstream_id)?;

        let snapshot = read_snapshot(root).map_err(|_| {
            PresentationError::ControlRefused("provider switching state is unavailable")
        })?;
        let source = exact_cycle_source(&snapshot, status.workstream_id)?;
        let source_runtime = source.runtime.ok_or(PresentationError::ControlRefused(
            "provider switching source Runtime is unavailable",
        ))?;
        strict_cycle_runtime(
            root,
            source.workstream_id,
            source.revision,
            source_runtime.runtime_id,
            source_runtime.revision,
        )
        .map_err(|_| PresentationError::ControlRefused("provider switching Runtime changed"))?;

        let ordered = workstreams_in_visual_order(&snapshot, false);
        let source_index = ordered
            .iter()
            .position(|workstream| workstream.workstream_id == source.workstream_id)
            .ok_or(PresentationError::ControlRefused(
                "provider switching source is unavailable",
            ))?;
        let destination = adjacent_cycle_destination(root, &ordered, source_index, action).ok_or(
            PresentationError::ControlRefused("no eligible Workstream in that direction"),
        )?;
        let destination_runtime = destination
            .runtime
            .ok_or(PresentationError::ControlRefused(
                "destination Runtime is unavailable",
            ))?;

        presentation.attach_workstream_claimed(
            destination.workstream_id,
            destination.revision,
            destination_runtime.runtime_id,
            destination_runtime.revision,
            AttachmentPurpose::ProviderCycle,
        )?;
        Ok(())
    });

    if let Err(error) = &result {
        let message = if matches!(
            error,
            PresentationError::ControlRefused("no eligible Workstream in that direction")
        ) {
            "No eligible Workstream in that direction"
        } else {
            "Workstream switch unavailable"
        };
        let _ = presentation.show_client_guidance(client_name, message);
    }
    result.map_err(AppError::from)
}

fn exact_cycle_source(
    snapshot: &crate::snapshot::Snapshot,
    workstream_id: crate::domain::WorkstreamId,
) -> Result<&crate::snapshot::WorkstreamSnapshot, PresentationError> {
    let source = workstreams_in_visual_order(snapshot, false)
        .into_iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .ok_or(PresentationError::ControlRefused(
            "provider switching source is unavailable",
        ))?;
    if !cycle_workstream_eligible(source) {
        return Err(PresentationError::ControlRefused(
            "provider switching source is unavailable",
        ));
    }
    Ok(source)
}

fn adjacent_cycle_destination<'a>(
    root: &StateRoot,
    ordered: &[&'a crate::snapshot::WorkstreamSnapshot],
    source_index: usize,
    action: PresentationAction,
) -> Option<&'a crate::snapshot::WorkstreamSnapshot> {
    adjacent_cycle_candidate(ordered, source_index, action, |candidate| {
        cycle_workstream_eligible(candidate)
            && candidate.runtime.is_some_and(|runtime| {
                strict_cycle_runtime(
                    root,
                    candidate.workstream_id,
                    candidate.revision,
                    runtime.runtime_id,
                    runtime.revision,
                )
                .is_ok()
            })
    })
}

fn adjacent_cycle_candidate<'a, F>(
    ordered: &[&'a crate::snapshot::WorkstreamSnapshot],
    source_index: usize,
    action: PresentationAction,
    mut eligible: F,
) -> Option<&'a crate::snapshot::WorkstreamSnapshot>
where
    F: FnMut(&crate::snapshot::WorkstreamSnapshot) -> bool,
{
    match action {
        PresentationAction::SwitchPrevious => ordered[..source_index]
            .iter()
            .rev()
            .copied()
            .find(|candidate| eligible(candidate)),
        PresentationAction::SwitchNext => {
            ordered
                .get(source_index.saturating_add(1)..)
                .and_then(|candidates| {
                    candidates
                        .iter()
                        .copied()
                        .find(|candidate| eligible(candidate))
                })
        }
        _ => None,
    }
}

fn cycle_workstream_eligible(workstream: &crate::snapshot::WorkstreamSnapshot) -> bool {
    !workstream.archived
        && workstream.lifecycle == WorkstreamLifecycle::Open
        && workstream.onboarding.is_none()
        && workstream.runtime.is_some_and(|runtime| {
            matches!(
                runtime.status,
                RuntimeStatus::Idle | RuntimeStatus::Working | RuntimeStatus::Attention
            )
        })
}

fn strict_cycle_runtime(
    root: &StateRoot,
    workstream_id: crate::domain::WorkstreamId,
    workstream_revision: Revision,
    runtime_id: RuntimeId,
    runtime_revision: Revision,
) -> Result<(), AppError> {
    let state = crate::state::open_current(&StateRoot::select(root.base()))?;
    if state
        .onboarding_workstream_projections()?
        .iter()
        .any(|onboarding| onboarding.workstream_id == workstream_id)
    {
        return Err(AppError::AttachmentUnavailable);
    }
    let registry = state.into_host_registry()?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(AppError::AttachmentUnavailable)?;
    let runtime = overview.runtime.ok_or(AppError::AttachmentUnavailable)?;
    if overview.revision != workstream_revision
        || runtime.runtime_id != runtime_id
        || runtime.revision != runtime_revision
    {
        return Err(AppError::AttachmentUnavailable);
    }
    let record = crate::actions::preflight_attachment_read_only(
        &StateRoot::select(root.base()),
        &registry,
        workstream_id,
    )?;
    if record.runtime_id != runtime_id || record.revision != runtime_revision {
        return Err(AppError::AttachmentUnavailable);
    }
    Ok(())
}

/// Routes literal Ctrl-b to the exact nested Runtime only after proving the
/// provider attachment. A provider pane with missing, stale, or failed
/// attachment evidence is left untouched, so the nested provider prefix can
/// never be consumed accidentally by the outer presentation server.
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
    presentation.validate_provider_context(status.workstream_id)?;
    let state = crate::state::open_current(&StateRoot::select(root.base()))?;
    let mut registry = state.into_host_registry()?;
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

pub(super) struct OpenCodeObserverArguments {
    pub(super) runtime_id: String,
    pub(super) generation: String,
    pub(super) port: u16,
    pub(super) session_id: String,
    pub(super) pane_pid: u32,
    pub(super) cwd: PathBuf,
    pub(super) provider_birth: String,
    pub(super) mode: crate::provider::opencode::OpenCodeObserverMode,
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
        mode: arguments.mode,
    };
    crate::provider::opencode::run_observer(root, &context).map_err(AppError::OpenCodeObserver)
}

/// Runs a proven schema-15 attachment only inside the presentation pane.
/// It keeps the retired application facade out of the schema-15 route and
/// repeats the workstream/runtime revisions immediately before private tmux
/// attachment.
#[allow(
    clippy::too_many_arguments,
    reason = "the pane helper receives only exact presentation and revision claims"
)]
pub(super) fn provider_attach(
    root: &StateRoot,
    workstream_id: &str,
    expected_workstream_revision: i64,
    expected_runtime_id: &str,
    expected_runtime_revision: i64,
    presentation_socket: PathBuf,
    presentation_session: String,
    attempt_id: &str,
    provider_cycle: bool,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let attempt_id =
        uuid::Uuid::parse_str(attempt_id).map_err(AppError::InvalidAttachmentAttempt)?;
    let parsed = (|| -> Result<_, AppError> {
        let workstream_id = parse_workstream(workstream_id)?;
        let expected_workstream_revision = parse_revision(expected_workstream_revision)?;
        let expected_runtime_id =
            RuntimeId::from_str(expected_runtime_id).map_err(AppError::InvalidRuntimeId)?;
        let expected_runtime_revision = parse_revision(expected_runtime_revision)?;
        Ok((
            workstream_id,
            expected_workstream_revision,
            expected_runtime_id,
            expected_runtime_revision,
        ))
    })();
    let outcome = if provider_cycle {
        let prepared = parsed.and_then(
            |(
                workstream_id,
                expected_workstream_revision,
                expected_runtime_id,
                expected_runtime_revision,
            )| {
                prepare_runtime_attach_read_only(
                    root,
                    workstream_id,
                    expected_workstream_revision,
                    expected_runtime_id,
                    expected_runtime_revision,
                )
            },
        );
        let (mut command, record) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = presentation.report_attachment_phase(attempt_id, AttachmentPhase::Failed);
                return Err(error);
            }
        };
        // This is intentionally the last operation before native attach:
        // Running means every ID/revision/provider check and Runtime tmux
        // preparation has already succeeded.
        if let Err(error) =
            presentation.report_attachment_phase(attempt_id, AttachmentPhase::Running)
        {
            let _ = presentation.report_attachment_phase(attempt_id, AttachmentPhase::Failed);
            return Err(error.into());
        }
        command
            .status()
            .map_err(AppError::Io)
            .and_then(|status| finish_runtime_attachment(root, &record, status))
    } else {
        presentation.report_attachment_phase(attempt_id, AttachmentPhase::Running)?;
        parsed.and_then(
            |(
                workstream_id,
                expected_workstream_revision,
                expected_runtime_id,
                expected_runtime_revision,
            )| {
                attach_runtime_with_outcome(
                    root,
                    workstream_id,
                    expected_workstream_revision,
                    expected_runtime_id,
                    expected_runtime_revision,
                )
            },
        )
    };
    let phase = if outcome.is_ok() {
        AttachmentPhase::Completed
    } else {
        AttachmentPhase::Failed
    };
    presentation.report_attachment_phase(attempt_id, phase)?;
    if matches!(outcome, Ok(RuntimeAttachmentEnd::Stopped)) {
        let _ = clear_stopped_provider_surface();
    }
    provider_wait()
}

/// Attaches the initial provisional shell's exact private Runtime. The outer
/// pane retains only its immutable provisional identity; once native tmux
/// returns, cleanup is possible only after that identity proves it became the
/// exact retired `provider_exec_proven` Runtime generation. The shared
/// attachment-end reconciler remains the sole mutation authority.
#[allow(
    clippy::too_many_arguments,
    reason = "the pane helper receives only immutable provisional identity claims"
)]
pub(super) fn provisional_provider_attach(
    root: &StateRoot,
    expected_presentation_id: &str,
    expected_presentation_revision: i64,
    expected_slot_generation: &str,
    expected_runtime_id: &str,
    presentation_socket: PathBuf,
    presentation_session: String,
) -> Result<(), AppError> {
    let presentation =
        Presentation::from_control(root.base(), presentation_socket, presentation_session)?;
    let identity = (|| -> Result<crate::shell_control::ProvisionalAttachmentIdentity, AppError> {
        Ok(crate::shell_control::ProvisionalAttachmentIdentity {
            presentation_id: uuid::Uuid::parse_str(expected_presentation_id)
                .map_err(|_| AppError::AttachmentUnavailable)?,
            presentation_revision: Revision::try_from(expected_presentation_revision)
                .map_err(|_| AppError::AttachmentUnavailable)?,
            slot_generation: uuid::Uuid::parse_str(expected_slot_generation)
                .map_err(|_| AppError::AttachmentUnavailable)?,
            candidate_runtime_id: RuntimeId::from_str(expected_runtime_id)
                .map_err(AppError::InvalidRuntimeId)?,
        })
    })();
    let outcome = identity.and_then(|identity| {
        let tmux = super::SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let runtime = PrivateRuntime::new(
            &tmux,
            &process_probe,
            RuntimePaths::for_runtime(root.base(), identity.candidate_runtime_id),
        );
        let mut command = runtime.attach_command();
        command.stderr(Stdio::null());
        let status = command.status().map_err(AppError::Io)?;
        let Some(record) = crate::shell_control::retired_provisional_attachment_record(
            root,
            &presentation,
            identity,
        )
        .map_err(|_| AppError::AttachmentUnavailable)?
        else {
            return Ok(RuntimeAttachmentEnd::Detached);
        };
        finish_runtime_attachment(root, &record, status)
    });
    if matches!(outcome, Ok(RuntimeAttachmentEnd::Stopped)) {
        let _ = clear_stopped_provider_surface();
    }
    // Like managed attachment, retain an inert provider-pane helper after a
    // return. Failures stay silent in the provider pane and cannot become
    // native-provider traffic.
    provider_wait()
}

/// Attaches only a Runtime that is neither owned nor fenced by an
/// unfinished onboarding operation. The Navigator passes the same snapshot
/// revisions through the outer helper, so stale cards can never authorize an
/// attachment after a different state transition.
pub(super) fn attach_runtime(
    root: &StateRoot,
    workstream_id: crate::domain::WorkstreamId,
    expected_workstream_revision: Revision,
    expected_runtime_id: RuntimeId,
    expected_runtime_revision: Revision,
) -> Result<(), AppError> {
    attach_runtime_with_outcome(
        root,
        workstream_id,
        expected_workstream_revision,
        expected_runtime_id,
        expected_runtime_revision,
    )
    .map(|_| ())
}

fn attach_runtime_with_outcome(
    root: &StateRoot,
    workstream_id: crate::domain::WorkstreamId,
    expected_workstream_revision: Revision,
    expected_runtime_id: RuntimeId,
    expected_runtime_revision: Revision,
) -> Result<RuntimeAttachmentEnd, AppError> {
    let state = crate::state::open_current(&StateRoot::select(root.base()))?;
    if state
        .onboarding_workstream_projections()?
        .iter()
        .any(|onboarding| onboarding.workstream_id == workstream_id)
    {
        return Err(AppError::AttachmentUnavailable);
    }
    let mut registry = state.into_host_registry()?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(AppError::AttachmentUnavailable)?;
    let Some(runtime) = overview.runtime else {
        return Err(AppError::AttachmentUnavailable);
    };
    if overview.revision != expected_workstream_revision
        || runtime.runtime_id != expected_runtime_id
        || runtime.revision != expected_runtime_revision
    {
        return Err(AppError::AttachmentUnavailable);
    }
    let record = match crate::actions::preflight_attachment(root, &mut registry, workstream_id) {
        Ok(record) => record,
        Err(crate::actions::ActionError::ProviderExited) => {
            return Ok(RuntimeAttachmentEnd::Stopped);
        }
        Err(error) => return Err(error.into()),
    };
    if record.runtime_id != expected_runtime_id || record.revision != expected_runtime_revision {
        return Err(AppError::AttachmentUnavailable);
    }
    let tmux = super::SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    runtime.prepare_attach()?;
    let mut command = runtime.attach_command();
    command.stderr(Stdio::null());
    let status = command.status().map_err(AppError::Io)?;
    finish_runtime_attachment(root, &record, status)
}

/// Classifies the end of one native tmux attachment without confusing a
/// client detach with provider exit. A deliberate concurrent stop is accepted
/// only after its durable Runtime/Workstream outcome is visible; otherwise the
/// exact retained pane must prove either a still-live provider or a clean
/// native exit.
fn finish_runtime_attachment(
    root: &StateRoot,
    record: &crate::state::RuntimeRecord,
    status: std::process::ExitStatus,
) -> Result<RuntimeAttachmentEnd, AppError> {
    let reconciliation = (|| {
        let state = crate::state::open_current(&StateRoot::select(root.base()))?;
        let mut registry = state.into_host_registry()?;
        crate::actions::reconcile_provider_attachment_end(
            root,
            &mut registry,
            record.workstream_id,
            record.runtime_id,
            &record.tmux_generation,
        )
        .map_err(AppError::from)
    })();

    match reconciliation {
        Ok(true) => Ok(RuntimeAttachmentEnd::Stopped),
        Ok(false) if status.success() => Ok(RuntimeAttachmentEnd::Detached),
        Ok(false) => {
            if crate::actions::await_deliberate_park(root, record.runtime_id, record.workstream_id)?
            {
                Ok(RuntimeAttachmentEnd::Stopped)
            } else {
                Err(AppError::AttachFailed)
            }
        }
        Err(error) => {
            if crate::actions::await_deliberate_park(root, record.runtime_id, record.workstream_id)?
            {
                Ok(RuntimeAttachmentEnd::Stopped)
            } else {
                Err(error)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeAttachmentEnd {
    Detached,
    Stopped,
}

fn clear_stopped_provider_surface() -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    clear_stopped_provider_surface_to(&mut stdout)
}

fn clear_stopped_provider_surface_to(writer: &mut impl std::io::Write) -> std::io::Result<()> {
    crossterm::queue!(
        writer,
        crossterm::style::ResetColor,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0),
        crossterm::cursor::Show,
    )?;
    writer.flush()
}

/// Prepares the read-only provider-cycle attach command. No native process is
/// started until the caller has recorded `Running` for the exact pending
/// attempt, so that status phase is a truthful handoff boundary.
fn prepare_runtime_attach_read_only(
    root: &StateRoot,
    workstream_id: crate::domain::WorkstreamId,
    expected_workstream_revision: Revision,
    expected_runtime_id: RuntimeId,
    expected_runtime_revision: Revision,
) -> Result<(Command, crate::state::RuntimeRecord), AppError> {
    let state = crate::state::open_current(&StateRoot::select(root.base()))?;
    if state
        .onboarding_workstream_projections()?
        .iter()
        .any(|onboarding| onboarding.workstream_id == workstream_id)
    {
        return Err(AppError::AttachmentUnavailable);
    }
    let registry = state.into_host_registry()?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(AppError::AttachmentUnavailable)?;
    let Some(runtime_record) = overview.runtime else {
        return Err(AppError::AttachmentUnavailable);
    };
    if overview.revision != expected_workstream_revision
        || runtime_record.runtime_id != expected_runtime_id
        || runtime_record.revision != expected_runtime_revision
    {
        return Err(AppError::AttachmentUnavailable);
    }
    let record = crate::actions::preflight_attachment_read_only(root, &registry, workstream_id)?;
    if record.runtime_id != expected_runtime_id || record.revision != expected_runtime_revision {
        return Err(AppError::AttachmentUnavailable);
    }
    let tmux = super::SystemTmux::default();
    let process_probe = super::LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    runtime.prepare_attach()?;
    let mut command = runtime.attach_command();
    command.stderr(Stdio::null());
    Ok((command, record))
}

pub(super) fn provider_wait() -> Result<(), AppError> {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{adjacent_cycle_candidate, clear_stopped_provider_surface_to};
    use crate::{
        domain::{
            LocationId, ProjectId, ProviderKind, Revision, WorkstreamId, WorkstreamLifecycle,
        },
        presentation::PresentationAction,
        snapshot::WorkstreamSnapshot,
    };

    fn workstream(id: u128) -> WorkstreamSnapshot {
        WorkstreamSnapshot {
            project_id: ProjectId::from(Uuid::from_u128(1)),
            location_id: LocationId::from(Uuid::from_u128(2)),
            workstream_id: WorkstreamId::from(Uuid::from_u128(id)),
            provider: ProviderKind::Codex,
            lifecycle: WorkstreamLifecycle::Open,
            archived: false,
            last_activity_sequence: 1,
            last_activity_at_millis: None,
            revision: Revision::INITIAL,
            runtime: None,
            onboarding: None,
            native_name: None,
        }
    }

    #[test]
    fn stopped_provider_surface_emits_only_terminal_reset_and_clear_controls() {
        let mut output = Vec::new();

        clear_stopped_provider_surface_to(&mut output).unwrap();

        assert_eq!(output, b"\x1b[0m\x1b[2J\x1b[1;1H\x1b[?25h");
    }

    #[test]
    fn cycle_candidate_skips_ineligible_rows_without_wrap() {
        let rows = [
            workstream(10),
            workstream(11),
            workstream(12),
            workstream(13),
        ];
        let references = rows.iter().collect::<Vec<_>>();
        let destination = adjacent_cycle_candidate(
            &references,
            0,
            PresentationAction::SwitchNext,
            |candidate| candidate.workstream_id != WorkstreamId::from(Uuid::from_u128(11)),
        )
        .expect("eligible row after skipped candidate");
        assert_eq!(
            destination.workstream_id,
            WorkstreamId::from(Uuid::from_u128(12))
        );
        assert!(
            adjacent_cycle_candidate(&references, 0, PresentationAction::SwitchPrevious, |_| true,)
                .is_none()
        );
        assert!(
            adjacent_cycle_candidate(
                &references,
                references.len() - 1,
                PresentationAction::SwitchNext,
                |_| true,
            )
            .is_none()
        );
    }
}
