//! Active D16 host-local navigator run loop.
//!
//! The model in [`super::d16`] is intentionally pure.  This module is the
//! small effect owner that connects it to the typed local application facade
//! and the already-owned private presentation.  It never opens a remote
//! monitor or a client catalog and it keeps provider-pane failures out of the
//! rendered native surface.

use std::{
    io::{self, Stdout},
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, MouseButton, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use thiserror::Error;

use crate::{
    app::ObserverActivation,
    application::{
        ApplicationAction, ApplicationError, ApplicationOutcome, AttachEvidence,
        HostRegistryApplicationBackend, LocalApplication, LocalApplicationBackend,
        ObserverReadiness, WorkstreamSnapshot,
    },
    domain::{RuntimeStatus, WorkstreamId},
    presentation::{Presentation, PresentationError},
    state::{self, StateRoot},
};

use super::{D16Command, D16Navigator};

/// Errors that prevent the private Navigator pane from entering its TUI.
#[derive(Debug, Error)]
pub enum D16NavigatorError {
    #[error("navigator terminal setup failed: {0}")]
    Terminal(#[source] io::Error),
    #[error("navigator application setup failed: {0}")]
    Application(#[source] ApplicationError),
    #[error("navigator presentation setup failed: {0}")]
    Presentation(#[source] PresentationError),
}

/// Runs the active D16 navigator inside one exact presentation pane.
///
/// # Errors
///
/// Returns an error only when terminal setup, current-only application
/// opening, or the exact private presentation path cannot be established.
/// Ordinary action and readiness failures are rendered as bounded Navigator
/// status text and do not write diagnostics to the provider pane.
#[allow(
    clippy::too_many_lines,
    reason = "The run loop keeps terminal, mouse, focus, and refresh ordering in one auditable boundary."
)]
pub fn run_local_navigator(
    root: &StateRoot,
    socket: PathBuf,
    session_name: String,
) -> Result<(), D16NavigatorError> {
    let presentation = Presentation::from_control(root.base(), socket, session_name)
        .map_err(D16NavigatorError::Presentation)?;
    let application_root = StateRoot::select(root.base());
    let backend = HostRegistryApplicationBackend::open(application_root)
        .map_err(D16NavigatorError::Application)?;
    let host_id = backend.host_id();
    let hostname = crate::application::operating_system_hostname();
    let mut application = LocalApplication::new(backend, host_id, hostname);
    let snapshot = application
        .snapshot()
        .map_err(D16NavigatorError::Application)?;
    let mut navigator = D16Navigator::new(snapshot);
    let mut terminal = TerminalSession::enter().map_err(D16NavigatorError::Terminal)?;
    let mut redraw = true;
    let mut last_refresh = Instant::now();
    let mut mouse_down = None;
    let mut pending_attach = None;
    let mut observer_replay = None;
    if let Ok(Some(status)) = presentation.attachment_status() {
        navigator.model_mut().observe_attachment(&status);
        if matches!(
            status.phase,
            crate::presentation::AttachmentPhase::Pending
                | crate::presentation::AttachmentPhase::Running
        ) {
            pending_attach = Some(PendingAttachment::Running {
                attempt_id: status.attempt_id,
                workstream_id: status.workstream_id,
            });
        } else {
            navigator.model_mut().clear_attachment(status.attempt_id);
        }
    }

    let quit = loop {
        if redraw {
            terminal
                .terminal
                .draw(|frame| navigator.render(frame, frame.area()))
                .map_err(D16NavigatorError::Terminal)?;
            redraw = false;
        }

        if event::poll(Duration::from_millis(100)).map_err(D16NavigatorError::Terminal)? {
            match event::read().map_err(D16NavigatorError::Terminal)? {
                Event::Key(key) => {
                    let command = navigator.handle_key(key.code);
                    if execute_command(
                        command,
                        &mut application,
                        &mut navigator,
                        &presentation,
                        root,
                        &mut pending_attach,
                        &mut observer_replay,
                        FocusAfter::Provider,
                    ) {
                        break true;
                    }
                    redraw = true;
                }
                Event::Mouse(mouse) if !navigator.model().help_visible() => {
                    let command = match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            navigator.model_mut().select_previous();
                            None
                        }
                        MouseEventKind::ScrollDown => {
                            navigator.model_mut().select_next();
                            None
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let size = terminal
                                .terminal
                                .size()
                                .map_err(D16NavigatorError::Terminal)?;
                            mouse_down = navigator.row_at(
                                Rect::new(0, 0, size.width, size.height),
                                mouse.column,
                                mouse.row,
                            );
                            None
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            let size = terminal
                                .terminal
                                .size()
                                .map_err(D16NavigatorError::Terminal)?;
                            let target = navigator.row_at(
                                Rect::new(0, 0, size.width, size.height),
                                mouse.column,
                                mouse.row,
                            );
                            let pressed = mouse_down.take();
                            if pressed.is_some() && pressed == target {
                                pressed.map(|row| navigator.model_mut().activate_row(row))
                            } else if target.is_none() {
                                presentation.focus_navigator().ok();
                                None
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(command) = command
                        && execute_command(
                            command,
                            &mut application,
                            &mut navigator,
                            &presentation,
                            root,
                            &mut pending_attach,
                            &mut observer_replay,
                            FocusAfter::Navigator,
                        )
                    {
                        break true;
                    }
                    redraw = true;
                }
                Event::Resize(_, _) => {
                    if presentation.set_default_navigator_width().is_err() {
                        navigator.model_mut().set_message(
                            "Navigator resize is unavailable; private presentation evidence changed",
                        );
                    }
                    redraw = true;
                }
                _ => {}
            }
        }

        if last_refresh.elapsed() >= Duration::from_millis(500) {
            refresh_application(
                &mut application,
                &mut navigator,
                &presentation,
                root,
                &mut pending_attach,
                &mut observer_replay,
            );
            redraw = true;
            last_refresh = Instant::now();
        }
    };

    drop(terminal);
    if quit {
        presentation
            .stop_session()
            .map_err(D16NavigatorError::Presentation)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusAfter {
    Provider,
    Navigator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingAttachment {
    AwaitRuntime {
        workstream_id: WorkstreamId,
        focus_after: FocusAfter,
    },
    Running {
        attempt_id: uuid::Uuid,
        workstream_id: WorkstreamId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObserverReplay {
    action: ApplicationAction,
    ownership: crate::provider::codex::profile::ProfileOwnership,
    prepared_revision: crate::domain::Revision,
    expected_ready_revision: crate::domain::Revision,
    focus_after: FocusAfter,
}

#[allow(
    clippy::too_many_arguments,
    reason = "These explicit channels keep local action, presentation, focus, and replay state separate."
)]
fn execute_command(
    command: D16Command,
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    navigator: &mut D16Navigator,
    presentation: &Presentation,
    root: &StateRoot,
    pending_attach: &mut Option<PendingAttachment>,
    observer_replay: &mut Option<ObserverReplay>,
    focus_after_attach: FocusAfter,
) -> bool {
    match command {
        D16Command::None => false,
        D16Command::Quit => true,
        D16Command::Attach(evidence) => {
            if attachment_replacement_blocked(pending_attach.as_ref()) {
                navigator
                    .model_mut()
                    .set_message("provider start is already in progress");
            } else {
                attach_existing(
                    application,
                    navigator,
                    presentation,
                    evidence,
                    focus_after_attach,
                    pending_attach,
                );
            }
            false
        }
        D16Command::Apply(action) => {
            let auto_target = auto_attach_target(&action);
            match application.apply(action) {
                Ok(outcome) => {
                    let created_id = match &outcome {
                        ApplicationOutcome::Created { workstream_id, .. } => Some(*workstream_id),
                        _ => None,
                    };
                    navigator.accept_outcome(outcome);
                    if let Some(workstream_id) = created_id.or(auto_target) {
                        *pending_attach = Some(PendingAttachment::AwaitRuntime {
                            workstream_id,
                            focus_after: focus_after_attach,
                        });
                    }
                }
                Err(error) => show_application_error(navigator, &error),
            }
            let _ = root;
            false
        }
        D16Command::AcceptObserverGuide(guide) => {
            accept_observer_guide(
                application,
                navigator,
                presentation,
                root,
                guide,
                pending_attach,
                focus_after_attach,
                observer_replay,
            );
            false
        }
    }
}

fn attachment_replacement_blocked(pending: Option<&PendingAttachment>) -> bool {
    matches!(pending, Some(PendingAttachment::AwaitRuntime { .. }))
}

fn auto_attach_target(action: &ApplicationAction) -> Option<WorkstreamId> {
    match action {
        ApplicationAction::Start { workstream_id, .. }
        | ApplicationAction::Recover { workstream_id, .. } => Some(*workstream_id),
        _ => None,
    }
}

fn attach_existing(
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    navigator: &mut D16Navigator,
    presentation: &Presentation,
    evidence: AttachEvidence,
    focus_after: FocusAfter,
    pending_attach: &mut Option<PendingAttachment>,
) {
    let Some(evidence) = authorize_attachment(application, navigator, evidence) else {
        return;
    };
    let Ok(status) = presentation.attach_workstream(evidence.workstream_id) else {
        *pending_attach = None;
        navigator
            .model_mut()
            .set_message("native attachment is unavailable; private presentation evidence changed");
        return;
    };
    navigator.model_mut().observe_attachment(&status);
    *pending_attach = Some(PendingAttachment::Running {
        attempt_id: status.attempt_id,
        workstream_id: status.workstream_id,
    });
    let focus = match focus_after {
        FocusAfter::Provider => presentation.focus_provider(),
        FocusAfter::Navigator => presentation.focus_navigator(),
    };
    if focus.is_err() {
        navigator
            .model_mut()
            .set_message("attachment succeeded but pane focus is unavailable");
    }
}

fn authorize_attachment(
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    navigator: &mut D16Navigator,
    evidence: AttachEvidence,
) -> Option<AttachEvidence> {
    match application.attach(evidence) {
        Ok(_) => Some(evidence),
        Err(ApplicationError::StaleRevision { .. }) => {
            let Ok(snapshot) = application.snapshot() else {
                navigator.model_mut().set_message(
                    "attachment revision changed and the current snapshot is unavailable",
                );
                return None;
            };
            let refreshed = snapshot
                .active_workstreams()
                .find(|workstream| workstream.workstream_id == evidence.workstream_id)
                .and_then(|workstream| refreshed_attachment_evidence(workstream, evidence));
            navigator.replace_snapshot(snapshot);
            let Some(refreshed) = refreshed else {
                navigator
                    .model_mut()
                    .set_message("attachment refused; selected Runtime identity changed");
                return None;
            };
            if application.attach(refreshed).is_ok() {
                Some(refreshed)
            } else {
                navigator
                    .model_mut()
                    .set_message("attachment refused; Workstream or Runtime kept changing");
                None
            }
        }
        Err(_) => {
            navigator
                .model_mut()
                .set_message("attachment refused; exact Runtime is unavailable");
            None
        }
    }
}

fn refreshed_attachment_evidence(
    workstream: &WorkstreamSnapshot,
    previous: AttachEvidence,
) -> Option<AttachEvidence> {
    let runtime = workstream.runtime?;
    (workstream.workstream_id == previous.workstream_id
        && runtime.runtime_id == previous.runtime_id
        && runtime_status_allows_attachment(runtime.status))
    .then_some(AttachEvidence {
        workstream_id: workstream.workstream_id,
        runtime_id: runtime.runtime_id,
        expected_workstream_revision: workstream.revision,
        expected_runtime_revision: runtime.revision,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "Guide acceptance carries the explicit action, presentation, replay, and attachment authorities separately."
)]
fn accept_observer_guide(
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    navigator: &mut D16Navigator,
    presentation: &Presentation,
    root: &StateRoot,
    guide: crate::application::ObserverReadinessGuide,
    pending_attach: &mut Option<PendingAttachment>,
    focus_after: FocusAfter,
    observer_replay: &mut Option<ObserverReplay>,
) {
    let Ok(current) = application.backend().observer_readiness() else {
        navigator
            .model_mut()
            .set_message("observer readiness could not be revalidated");
        return;
    };
    if current != guide.evidence {
        navigator.model_mut().dismiss_observer_guide();
        navigator
            .model_mut()
            .set_message("observer intent became stale; retry the action");
        return;
    }
    if !guide.evidence.needs_guide() {
        navigator.model_mut().dismiss_observer_guide();
        navigator
            .model_mut()
            .set_message("observer readiness does not authorize a setup guide");
        return;
    }
    let Some(action) = navigator.model().pending_action().cloned() else {
        navigator.model_mut().dismiss_observer_guide();
        navigator
            .model_mut()
            .set_message("observer action intent is no longer available; retry the action");
        return;
    };
    let state_root = StateRoot::select(root.base());
    let Ok(state) = state::open_current_only(&state_root) else {
        navigator
            .model_mut()
            .set_message("observer setup requires current schema-13 host state");
        return;
    };
    let Ok(mut registry) = state.into_host_registry() else {
        navigator
            .model_mut()
            .set_message("observer setup requires current schema-13 host state");
        return;
    };
    let activation = match crate::app::prepare_observer_activation(root, &mut registry) {
        Ok(activation) => activation,
        Err(error) => {
            show_observer_activation_error(navigator, &error);
            return;
        }
    };
    let integration = registry.codex_integration().ok().flatten();
    drop(registry);
    let Some(integration) = integration else {
        navigator
            .model_mut()
            .set_message("observer preparation did not produce an owned integration");
        return;
    };
    let prepared_revision = integration.revision;
    let expected_ready_revision = match activation {
        ObserverActivation::Ready => prepared_revision,
        ObserverActivation::ReviewRequired => prepared_revision.next(),
    };
    let replay = ObserverReplay {
        action,
        ownership: integration.ownership,
        prepared_revision,
        expected_ready_revision,
        focus_after,
    };
    *observer_replay = Some(replay);
    if matches!(activation, ObserverActivation::Ready) {
        replay_observer_action(
            application,
            navigator,
            root,
            pending_attach,
            observer_replay,
        );
    } else {
        match presentation.start_observer_review() {
            Ok(()) => {
                if presentation.focus_provider().is_err() {
                    navigator
                        .model_mut()
                        .set_message("native observer review could not be focused");
                }
            }
            Err(_) => navigator
                .model_mut()
                .set_message("native observer review could not be opened"),
        }
    }
}

fn show_observer_activation_error(navigator: &mut D16Navigator, error: &crate::app::AppError) {
    let message = match error {
        crate::app::AppError::LiveRuntimePreventsObserverActivation => {
            "observer activation refused while a managed Runtime is live"
        }
        _ => "observer preparation refused; existing Runtime state was preserved",
    };
    navigator.model_mut().set_message(message);
}

fn replay_observer_action(
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    navigator: &mut D16Navigator,
    root: &StateRoot,
    pending_attach: &mut Option<PendingAttachment>,
    observer_replay: &mut Option<ObserverReplay>,
) {
    let Some(replay) = observer_replay.as_ref() else {
        return;
    };
    if navigator.model().pending_action() != Some(&replay.action) {
        *observer_replay = None;
        navigator.model_mut().dismiss_observer_guide();
        navigator
            .model_mut()
            .set_message("observer action intent changed; retry the action");
        return;
    }
    let Ok(current) = application.backend().observer_readiness() else {
        navigator
            .model_mut()
            .set_message("observer readiness could not be revalidated");
        return;
    };
    let ready = current.readiness == ObserverReadiness::Ready
        && current.integration_revision == Some(replay.expected_ready_revision);
    let review_pending = current.readiness == ObserverReadiness::TrustReviewRequired
        && current.integration_revision == Some(replay.prepared_revision);
    if !ready {
        if !review_pending {
            *observer_replay = None;
            navigator.model_mut().dismiss_observer_guide();
            navigator
                .model_mut()
                .set_message("observer ownership or readiness changed; retry the action");
        }
        return;
    }
    let state_root = StateRoot::select(root.base());
    let integration_matches = state::open_current_only(&state_root)
        .ok()
        .and_then(|state| state.into_host_registry().ok())
        .and_then(|registry| registry.codex_integration().ok().flatten())
        .is_some_and(|integration| {
            integration.ownership == replay.ownership
                && integration.revision == replay.expected_ready_revision
                && (replay.expected_ready_revision == replay.prepared_revision
                    || replay.expected_ready_revision == replay.prepared_revision.next())
        });
    if !integration_matches {
        *observer_replay = None;
        navigator.model_mut().dismiss_observer_guide();
        navigator
            .model_mut()
            .set_message("observer ownership changed; retry the action");
        return;
    }
    let action = replay.action.clone();
    let auto_target = auto_attach_target(&action);
    let focus_after = replay.focus_after;
    match application.apply(action) {
        Ok(outcome) => {
            let created_id = match &outcome {
                ApplicationOutcome::Created { workstream_id, .. } => Some(*workstream_id),
                _ => None,
            };
            *observer_replay = None;
            navigator.accept_outcome(outcome);
            if let Some(workstream_id) = created_id.or(auto_target) {
                *pending_attach = Some(PendingAttachment::AwaitRuntime {
                    workstream_id,
                    focus_after,
                });
            }
        }
        Err(error) => show_application_error(navigator, &error),
    }
}

fn refresh_application(
    application: &mut LocalApplication<HostRegistryApplicationBackend>,
    navigator: &mut D16Navigator,
    presentation: &Presentation,
    root: &StateRoot,
    pending_attach: &mut Option<PendingAttachment>,
    observer_replay: &mut Option<ObserverReplay>,
) {
    if let Ok(snapshot) = application.snapshot() {
        navigator.replace_snapshot(snapshot);
    } else {
        navigator
            .model_mut()
            .set_message("host snapshot unavailable; no local action was performed");
    }
    if observer_replay.is_some() {
        replay_observer_action(
            application,
            navigator,
            root,
            pending_attach,
            observer_replay,
        );
    }
    match *pending_attach {
        Some(PendingAttachment::Running {
            attempt_id,
            workstream_id,
            ..
        }) => poll_attachment(
            navigator,
            presentation,
            pending_attach,
            attempt_id,
            workstream_id,
        ),
        Some(PendingAttachment::AwaitRuntime {
            workstream_id,
            focus_after,
        }) => {
            let Some(workstream) = navigator
                .model()
                .snapshot()
                .active_workstreams()
                .find(|workstream| workstream.workstream_id == workstream_id)
            else {
                *pending_attach = None;
                navigator
                    .model_mut()
                    .set_message("created Workstream disappeared; no attachment was attempted");
                return;
            };
            let Some(runtime) = workstream.runtime else {
                return;
            };
            if !runtime_status_allows_attachment(runtime.status) {
                *pending_attach = None;
                navigator
                    .model_mut()
                    .set_message("provider start did not produce an attachable Runtime");
                return;
            }
            let evidence = AttachEvidence {
                workstream_id,
                runtime_id: runtime.runtime_id,
                expected_workstream_revision: workstream.revision,
                expected_runtime_revision: runtime.revision,
            };
            *pending_attach = None;
            attach_existing(
                application,
                navigator,
                presentation,
                evidence,
                focus_after,
                pending_attach,
            );
        }
        None => {}
    }
}

fn runtime_status_allows_attachment(status: RuntimeStatus) -> bool {
    // `Starting` already has the exact recorded private Runtime identity. Its
    // native SessionStart observation may follow terminal attachment, so it
    // must not be used as an attachment prerequisite.
    !matches!(status, RuntimeStatus::Stopped | RuntimeStatus::Unknown)
}

fn poll_attachment(
    navigator: &mut D16Navigator,
    presentation: &Presentation,
    pending_attach: &mut Option<PendingAttachment>,
    attempt_id: uuid::Uuid,
    workstream_id: WorkstreamId,
) {
    let Ok(status) = presentation.attachment_status() else {
        navigator.model_mut().set_message(
            "attachment status is unavailable; native helper was not assumed successful",
        );
        return;
    };
    let Some(status) = status else {
        *pending_attach = None;
        navigator.model_mut().set_message(
            "attachment attempt disappeared; press Enter or click the same row to retry",
        );
        return;
    };
    if status.attempt_id != attempt_id || status.workstream_id != workstream_id {
        *pending_attach = None;
        navigator
            .model_mut()
            .set_message("attachment attempt changed; press Enter or click the same row to retry");
        return;
    }
    navigator.model_mut().observe_attachment(&status);
    if matches!(
        status.phase,
        crate::presentation::AttachmentPhase::Completed
            | crate::presentation::AttachmentPhase::Failed
    ) {
        *pending_attach = None;
        navigator.model_mut().clear_attachment(status.attempt_id);
    }
}

fn show_application_error(navigator: &mut D16Navigator, error: &ApplicationError) {
    let message = match error {
        ApplicationError::StaleRevision { .. } => "action became stale; refresh and retry",
        ApplicationError::ObserverUnavailable {
            readiness: ObserverReadiness::Foreign,
        } => "observer ownership is foreign; action refused",
        ApplicationError::ObserverUnavailable {
            readiness: ObserverReadiness::Modified,
        } => "owned observer declaration changed; action refused",
        ApplicationError::ObserverUnavailable {
            readiness: ObserverReadiness::Disabled,
        } => "observer integration is disabled; action refused",
        ApplicationError::ObserverUnavailable {
            readiness: ObserverReadiness::Ambiguous | ObserverReadiness::Unknown,
        } => "observer readiness is ambiguous; refresh and retry",
        ApplicationError::ObserverUnavailable { .. } => "observer readiness requires native review",
        _ => "local action refused; no provider input was sent",
    };
    navigator.model_mut().set_message(message);
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        if let Err(error) = execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture
        ) {
            disable_raw_mode()?;
            return Err(error);
        }
        terminal.clear()?;
        Ok(Self { terminal })
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workstream_with_runtime(
        workstream_id: WorkstreamId,
        runtime_id: crate::domain::RuntimeId,
        status: RuntimeStatus,
    ) -> WorkstreamSnapshot {
        WorkstreamSnapshot {
            project_id: crate::domain::ProjectId::new(),
            location_id: crate::domain::LocationId::new(),
            workstream_id,
            provider: crate::domain::ProviderKind::Codex,
            lifecycle: crate::domain::WorkstreamLifecycle::Open,
            archived: false,
            last_activity_sequence: 1,
            last_activity_at_millis: None,
            revision: crate::domain::Revision::INITIAL.next(),
            runtime: Some(crate::application::RuntimeSnapshot {
                runtime_id,
                status,
                revision: crate::domain::Revision::INITIAL.next().next(),
                observer_degraded: false,
            }),
            attention: crate::application::AttentionSnapshot {
                result_unseen: false,
                recovery_unseen: false,
                revision: crate::domain::Revision::INITIAL,
            },
            native_name: None,
        }
    }

    #[test]
    fn running_attachment_can_be_replaced_but_runtime_start_remains_serialized() {
        let workstream_id = WorkstreamId::new();
        let running = PendingAttachment::Running {
            attempt_id: uuid::Uuid::new_v4(),
            workstream_id,
        };
        assert!(!attachment_replacement_blocked(Some(&running)));

        let starting = PendingAttachment::AwaitRuntime {
            workstream_id,
            focus_after: FocusAfter::Provider,
        };
        assert!(attachment_replacement_blocked(Some(&starting)));
        assert!(!attachment_replacement_blocked(None));
    }

    #[test]
    fn starting_runtime_is_attachable_before_session_start_observation() {
        assert!(runtime_status_allows_attachment(RuntimeStatus::Starting));
        assert!(runtime_status_allows_attachment(RuntimeStatus::Idle));
        assert!(runtime_status_allows_attachment(RuntimeStatus::Working));
        assert!(runtime_status_allows_attachment(RuntimeStatus::Attention));
        assert!(!runtime_status_allows_attachment(RuntimeStatus::Stopped));
        assert!(!runtime_status_allows_attachment(RuntimeStatus::Unknown));
    }

    #[test]
    fn stale_attachment_refresh_requires_the_same_attachable_runtime_identity() {
        let workstream_id = WorkstreamId::new();
        let runtime_id = crate::domain::RuntimeId::new();
        let previous = AttachEvidence {
            workstream_id,
            runtime_id,
            expected_workstream_revision: crate::domain::Revision::INITIAL,
            expected_runtime_revision: crate::domain::Revision::INITIAL,
        };
        let current = workstream_with_runtime(workstream_id, runtime_id, RuntimeStatus::Idle);
        let refreshed = refreshed_attachment_evidence(&current, previous).unwrap();
        assert_eq!(refreshed.workstream_id, previous.workstream_id);
        assert_eq!(refreshed.runtime_id, previous.runtime_id);
        assert_eq!(refreshed.expected_workstream_revision, current.revision);
        assert_eq!(
            refreshed.expected_runtime_revision,
            current.runtime.unwrap().revision
        );

        let rotated = workstream_with_runtime(
            workstream_id,
            crate::domain::RuntimeId::new(),
            RuntimeStatus::Idle,
        );
        assert!(refreshed_attachment_evidence(&rotated, previous).is_none());
        let stopped = workstream_with_runtime(workstream_id, runtime_id, RuntimeStatus::Stopped);
        assert!(refreshed_attachment_evidence(&stopped, previous).is_none());
    }
}
