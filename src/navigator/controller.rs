//! schema-15 Navigator pane.
//!
//! This controller owns terminal setup, passive snapshots, shell-card
//! materialization, and exact presentation attachment. It is reached solely
//! from the hidden presentation-pane command.

use std::{
    env,
    ffi::OsStr,
    io::{self, Stdout},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
        Event, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use thiserror::Error;

use crate::{
    account_shell::{AccountShellContext, AccountShellLaunch},
    app::observer::{
        ObserverActivation, ObserverReadiness, ObserverReadinessEvidence,
        finalize_observer_trust_under_lease, observer_readiness, prepare_observer_activation,
    },
    domain::{ProviderKind, Revision, RuntimeId, WorkstreamId},
    presentation::{AttachmentPhase, AttachmentPurpose, Presentation, PresentationError},
    provider_reconcile::ExpectedProviderExecutable,
    provisional::{
        PreHandoffRecovery, ProvisionalPhase, ProvisionalSlot, SlotError, SlotGeneration,
        read_marker, reconcile_pre_handoff_under_lease,
    },
    review::ReviewDirectory,
    runtime::{LinuxProcessProbe, PrivateRuntime, SystemTmux},
    shell_control::reconcile_provider_exec_from_presentation,
    snapshot::{Snapshot, SnapshotError, read_snapshot},
    state::{StateRoot, open_current},
};

use super::view::{Command, Navigator, ObserverSetupKind, ShellLocation};

const MANAGED_SESSION_RECONCILIATION_GUIDANCE: &str =
    "Managed session reconciliation is unavailable; exact recovery required";

/// Errors that prevent the pane from rendering its schema-15-only
/// Workstreams view.
#[derive(Debug, Error)]
pub(crate) enum NavigatorError {
    #[error("navigator terminal setup failed: {0}")]
    Terminal(#[source] io::Error),
    #[error("navigator presentation setup failed: {0}")]
    Presentation(#[from] PresentationError),
    #[error("navigator state is unavailable: {0}")]
    Snapshot(#[from] SnapshotError),
    #[error("provisional shell is unavailable")]
    ProvisionalShellUnavailable,
    #[error("same-location session creation is unavailable")]
    SameLocationSessionUnavailable,
    #[error("managed action is unavailable")]
    ManagedActionUnavailable,
}

/// Runs the hidden schema-15 Navigator pane. It validates the exact
/// presentation context before reading state. The provisional-shell command is
/// a lease-held marker-first materialization followed by an outer-pane attach.
#[allow(
    clippy::too_many_lines,
    reason = "The loop keeps shell, promotion, exact attachment, and focus ordering in one auditable owner."
)]
pub(crate) fn run_navigator(
    root: &StateRoot,
    socket: PathBuf,
    session_name: String,
) -> Result<(), NavigatorError> {
    let presentation = Presentation::from_control(root.base(), socket, session_name)?;
    let context =
        Presentation::context_from_directory(root.base(), &presentation.paths().directory)
            .map_err(|_| PresentationError::ContextUnavailable)?;
    let seed_cwd = context.seed_cwd().to_path_buf();
    let home = env::var_os("HOME").map(PathBuf::from);
    // Reconcile any interrupted pre-effect marker before taking the passive
    // snapshot.  A failure stays visible as bounded guidance; it must not
    // become permission to allocate a replacement candidate.
    let startup_recovery = reconcile_pre_handoff_presentation(root, &presentation);
    let snapshot = read_snapshot(root)?;
    let mut navigator = Navigator::new(snapshot);
    if startup_recovery.is_err() {
        navigator.set_guidance("Provisional shell recovery is unavailable; exact state required");
    }
    let mut observed_shell_cwd = None;
    refresh_shell_location(
        root,
        &presentation,
        &seed_cwd,
        home.as_deref(),
        &mut observed_shell_cwd,
        &mut navigator,
    );
    let mut terminal = TerminalSession::enter().map_err(NavigatorError::Terminal)?;
    let mut redraw = true;
    let mut last_refresh = Instant::now();
    let mut mouse_down = None;
    let mut promoted_runtime = None;
    let mut pending_observer = None;
    let mut seen_cycle_attempt = None;

    let quit = loop {
        if redraw {
            terminal
                .terminal
                .draw(|frame| navigator.render(frame, frame.area()))
                .map_err(NavigatorError::Terminal)?;
            redraw = false;
        }
        if event::poll(Duration::from_millis(100)).map_err(NavigatorError::Terminal)? {
            match event::read().map_err(NavigatorError::Terminal)? {
                Event::Key(key) => {
                    let command = navigator.handle_key(key.code);
                    if execute_command(
                        command,
                        root,
                        &mut navigator,
                        &presentation,
                        &mut pending_observer,
                    ) {
                        break true;
                    }
                    redraw = true;
                }
                Event::Mouse(mouse) => {
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
                            let size =
                                terminal.terminal.size().map_err(NavigatorError::Terminal)?;
                            mouse_down = navigator.row_at(
                                Rect::new(0, 0, size.width, size.height),
                                mouse.column,
                                mouse.row,
                            );
                            None
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            let size =
                                terminal.terminal.size().map_err(NavigatorError::Terminal)?;
                            let target = navigator.row_at(
                                Rect::new(0, 0, size.width, size.height),
                                mouse.column,
                                mouse.row,
                            );
                            let pressed = mouse_down.take();
                            if pressed.is_some() && pressed == target {
                                pressed.map(|row| navigator.model_mut().activate_row(row))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(command) = command
                        && execute_command(
                            command,
                            root,
                            &mut navigator,
                            &presentation,
                            &mut pending_observer,
                        )
                    {
                        break true;
                    }
                    redraw = true;
                }
                Event::Resize(_, _) => {
                    if restore_default_navigator_width(&presentation).is_err() {
                        navigator.set_guidance(
                            "Navigator resize is unavailable; exact presentation evidence changed",
                        );
                    }
                    redraw = true;
                }
                Event::FocusGained => {
                    navigator.set_terminal_focused(true);
                    redraw = true;
                }
                Event::FocusLost => {
                    navigator.set_terminal_focused(false);
                    mouse_down = None;
                    redraw = true;
                }
                Event::Paste(_) => {}
            }
        }
        if let Some(command) = finish_pending_observer_review(
            root,
            &mut navigator,
            &presentation,
            &mut pending_observer,
        ) && execute_command(
            command,
            root,
            &mut navigator,
            &presentation,
            &mut pending_observer,
        ) {
            break true;
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            let refresh = refresh_provider_exec(root, &presentation);
            if let ProviderExecRefresh::RuntimeOwned { runtime_id, .. } = refresh {
                promoted_runtime = Some(runtime_id);
            }
            apply_provider_exec_refresh(&mut navigator, refresh);
            if let Ok(snapshot) = read_snapshot(root) {
                navigator.replace_snapshot(snapshot);
                sync_cycle_selection(&presentation, &mut navigator, &mut seen_cycle_attempt);
                if let Some(runtime_id) = promoted_runtime
                    && navigator.select_runtime(runtime_id)
                {
                    promoted_runtime = None;
                }
            }
            refresh_shell_location(
                root,
                &presentation,
                &seed_cwd,
                home.as_deref(),
                &mut observed_shell_cwd,
                &mut navigator,
            );
            redraw = true;
            last_refresh = Instant::now();
        }
    };
    drop(terminal);
    if quit {
        presentation.stop_session()?;
    }
    Ok(())
}

/// A tmux attach resize and its installed width hook can briefly overlap. The
/// topology must still become exact within one small bounded interval; a
/// persistent ownership or shape failure remains a visible refusal.
fn restore_default_navigator_width(presentation: &Presentation) -> Result<(), PresentationError> {
    retry_default_navigator_width(|| presentation.set_default_navigator_width())
}

/// Consumes one purpose-tagged provider-cycle status on the existing bounded
/// refresh path. The attempt fence prevents repeated selection forcing while
/// leaving ordinary Navigator attachments entirely unaffected.
fn sync_cycle_selection(
    presentation: &Presentation,
    navigator: &mut Navigator,
    seen_attempt: &mut Option<uuid::Uuid>,
) {
    let Ok(Some(status)) = presentation.attachment_status_read_only() else {
        return;
    };
    let Some(attempt_id) = cycle_selection_attempt(&status, *seen_attempt) else {
        return;
    };
    if navigator.select_workstream(status.workstream_id) {
        *seen_attempt = Some(attempt_id);
    }
}

fn cycle_selection_attempt(
    status: &crate::presentation::AttachmentStatus,
    seen_attempt: Option<uuid::Uuid>,
) -> Option<uuid::Uuid> {
    (status.purpose == AttachmentPurpose::ProviderCycle
        && status.phase == AttachmentPhase::Running
        && seen_attempt != Some(status.attempt_id))
    .then_some(status.attempt_id)
}

fn retry_default_navigator_width(
    resize: impl FnMut() -> Result<(), PresentationError>,
) -> Result<(), PresentationError> {
    crate::presentation::retry_default_navigator_width(resize)
}

/// Refreshes only presentation-local shell context. The cwd is exact live
/// process evidence while a provisional account shell exists and never feeds
/// registration or provider launch.
fn refresh_shell_location(
    root: &StateRoot,
    presentation: &Presentation,
    seed_cwd: &Path,
    home: Option<&Path>,
    observed: &mut Option<PathBuf>,
    navigator: &mut Navigator,
) {
    let cwd = observe_shell_cwd(root, presentation, seed_cwd).ok();
    if cwd.as_ref() == observed.as_ref() {
        return;
    }
    let location = cwd.as_ref().map_or_else(
        || ShellLocation::cwd("unavailable"),
        |cwd| describe_shell_location(cwd, home),
    );
    navigator.set_shell_location(location);
    *observed = cwd;
}

fn observe_shell_cwd(
    root: &StateRoot,
    presentation: &Presentation,
    seed_cwd: &Path,
) -> Result<PathBuf, NavigatorError> {
    let slot = match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return Ok(seed_cwd.to_path_buf()),
        Err(_) => return Err(NavigatorError::ProvisionalShellUnavailable),
    };
    if !matches!(
        slot.phase(),
        ProvisionalPhase::Materialized | ProvisionalPhase::HandoffIssued
    ) {
        return Ok(seed_cwd.to_path_buf());
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    slot.revalidate_live_shell(&runtime, &process_probe)
        .map(|shell| shell.cwd)
        .map_err(|_| NavigatorError::ProvisionalShellUnavailable)
}

fn describe_shell_location(cwd: &Path, home: Option<&Path>) -> ShellLocation {
    let cwd_label = home
        .and_then(|home| cwd.strip_prefix(home).ok())
        .and_then(|relative| {
            if relative.as_os_str().is_empty() {
                Some("~".to_owned())
            } else {
                abbreviated_path(relative).map(|relative| format!("~/{relative}"))
            }
        })
        .or_else(|| abbreviated_path(cwd))
        .unwrap_or_else(|| "unavailable".to_owned());
    ShellLocation::cwd(&cwd_label)
}

fn abbreviated_path(path: &Path) -> Option<String> {
    let absolute = path.is_absolute();
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(component) => Some(safe_path_component(component)),
            Component::RootDir => None,
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => Some(None),
        })
        .collect::<Option<Vec<_>>>()?;
    if components.is_empty() {
        return absolute.then(|| "/".to_owned());
    }
    let last = components.len().saturating_sub(1);
    let display = components
        .iter()
        .enumerate()
        .map(|(index, component)| {
            if index == last {
                (*component).to_owned()
            } else {
                abbreviated_path_component(component)
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    Some(if absolute {
        format!("/{display}")
    } else {
        display
    })
}

fn safe_path_component(component: &OsStr) -> Option<&str> {
    component
        .to_str()
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
}

fn abbreviated_path_component(component: &str) -> String {
    let mut characters = component.chars();
    let first = characters
        .next()
        .expect("validated path component is non-empty");
    if first == '.' {
        characters
            .next()
            .map_or_else(|| ".".to_owned(), |next| format!(".{next}"))
    } else {
        first.to_string()
    }
}

/// Process-local intent retained while Codex native observer review owns the
/// right-hand presentation pane. It deliberately carries only typed IDs and
/// revisions; no provider argv, prompt, output, or capture is retained.
enum PendingObserverIntent {
    Managed(ManagedAction),
    NewAtSameLocation {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
}

impl PendingObserverIntent {
    fn into_command(self) -> Command {
        match self {
            Self::Managed(action) => match action {
                ManagedAction::Start {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                } => Command::Start {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                },
                ManagedAction::Recover {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                } => Command::Recover {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                },
                ManagedAction::Archive {
                    workstream_id,
                    expected_workstream_revision,
                } => Command::Archive {
                    workstream_id,
                    expected_workstream_revision,
                },
                ManagedAction::Restore {
                    workstream_id,
                    expected_workstream_revision,
                } => Command::Restore {
                    workstream_id,
                    expected_workstream_revision,
                },
            },
            Self::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            } => Command::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            },
        }
    }
}

struct PendingObserverSetup {
    intent: PendingObserverIntent,
    kind: ObserverSetupKind,
    evidence: ObserverReadinessEvidence,
    presentation_context: crate::presentation::PresentationContext,
    marker: Option<ProvisionalSlot>,
    expected_integration: Option<crate::state::CodexIntegration>,
    review_directory: Option<ReviewDirectory>,
}

/// Executes one model command. Presentation tmux alone owns pane focus;
/// replacing or attaching the right-hand surface never selects a pane.
#[allow(
    clippy::too_many_lines,
    reason = "The small command set keeps exact attachment and focus outcomes in one controller seam."
)]
fn execute_command(
    command: Command,
    root: &StateRoot,
    navigator: &mut Navigator,
    presentation: &Presentation,
    pending_observer: &mut Option<PendingObserverSetup>,
) -> bool {
    match command {
        Command::Quit => true,
        Command::MaterializeProvisionalShell => {
            if materialize_provisional_shell(root, presentation).is_ok() {
            } else {
                navigator.set_guidance("New session shell unavailable; exact state required");
            }
            false
        }
        Command::Attach {
            workstream_id,
            expected_workstream_revision,
            runtime_id,
            expected_runtime_revision,
        } => {
            if presentation
                .attach_workstream(
                    workstream_id,
                    expected_workstream_revision,
                    runtime_id,
                    expected_runtime_revision,
                )
                .is_ok()
            {
            } else {
                navigator.set_guidance(
                    "Managed session is unavailable; exact Runtime evidence required",
                );
            }
            false
        }
        Command::NewAtSameLocation {
            source_workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            let Some(PendingObserverIntent::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            }) = prepare_observer_or_request(
                root,
                navigator,
                presentation,
                PendingObserverIntent::NewAtSameLocation {
                    source_workstream_id,
                    expected_workstream_revision,
                    provider,
                },
                pending_observer,
            )
            else {
                return false;
            };
            match start_same_location(
                root,
                source_workstream_id,
                expected_workstream_revision,
                provider,
            ) {
                Ok((snapshot, attachment)) => {
                    navigator.replace_snapshot(snapshot);
                    navigator.select_runtime(attachment.runtime_id);
                    if presentation
                        .attach_workstream(
                            attachment.workstream_id,
                            attachment.workstream_revision,
                            attachment.runtime_id,
                            attachment.runtime_revision,
                        )
                        .is_ok()
                    {
                    } else {
                        navigator.set_guidance(
                            "New session started; exact Runtime attachment is unavailable",
                        );
                    }
                }
                Err(_) => navigator.set_guidance(
                    "New session is unavailable; selected provider and Location are required",
                ),
            }
            false
        }
        Command::Start {
            workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            let action = ManagedAction::Start {
                workstream_id,
                expected_workstream_revision,
                provider,
            };
            execute_managed_action_or_request(
                root,
                navigator,
                presentation,
                provider,
                action,
                pending_observer,
            );
            false
        }
        Command::Recover {
            workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            let action = ManagedAction::Recover {
                workstream_id,
                expected_workstream_revision,
                provider,
            };
            execute_managed_action_or_request(
                root,
                navigator,
                presentation,
                provider,
                action,
                pending_observer,
            );
            false
        }
        Command::Archive {
            workstream_id,
            expected_workstream_revision,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                ManagedAction::Archive {
                    workstream_id,
                    expected_workstream_revision,
                },
            );
            false
        }
        Command::Restore {
            workstream_id,
            expected_workstream_revision,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                ManagedAction::Restore {
                    workstream_id,
                    expected_workstream_revision,
                },
            );
            false
        }
        Command::AcceptObserverSetup { kind } => {
            accept_observer_setup(root, navigator, presentation, kind, pending_observer);
            false
        }
        Command::CancelObserverSetup => {
            pending_observer.take();
            navigator.set_guidance(
                "Codex observer setup was declined; no profile or trust state was changed",
            );
            false
        }
        Command::ShowGuidance(guidance) => {
            navigator.set_guidance(guidance);
            false
        }
        Command::None => false,
    }
}

/// Schema-15-native managed lifecycle intent. This deliberately bypasses the
/// retired application facade: each variant carries only the exact
/// durable IDs/revisions supplied by the passive snapshot.
#[derive(Clone, Copy)]
pub(crate) enum ManagedAction {
    Start {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    Recover {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    Archive {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Restore {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
}

/// Performs the Codex readiness preflight before any managed action can
/// reserve a Runtime or launch a provider. Setup/update/trust review is
/// offered only through the contextual Navigator guide, which retains the
/// typed action until exact native review has completed.
#[allow(
    clippy::too_many_lines,
    clippy::manual_let_else,
    clippy::single_match_else,
    reason = "The readiness boundary keeps every exact evidence branch together for audit."
)]
fn execute_managed_action_or_request(
    root: &StateRoot,
    navigator: &mut Navigator,
    presentation: &Presentation,
    _provider: ProviderKind,
    action: ManagedAction,
    pending_observer: &mut Option<PendingObserverSetup>,
) {
    let Some(PendingObserverIntent::Managed(action)) = prepare_observer_or_request(
        root,
        navigator,
        presentation,
        PendingObserverIntent::Managed(action),
        pending_observer,
    ) else {
        return;
    };
    execute_managed_action(root, navigator, presentation, action);
}

/// Reads the exact schema-15 observer state without reserving onboarding
/// capability or mutating profile/state. This preflight is deliberately
/// repeated at the action boundary because the Navigator snapshot is passive.
#[allow(
    clippy::too_many_lines,
    clippy::manual_let_else,
    clippy::single_match_else,
    reason = "The readiness classifier keeps its fail-closed evidence branches together."
)]
fn prepare_observer_or_request(
    root: &StateRoot,
    navigator: &mut Navigator,
    presentation: &Presentation,
    intent: PendingObserverIntent,
    pending_observer: &mut Option<PendingObserverSetup>,
) -> Option<PendingObserverIntent> {
    let provider = match &intent {
        PendingObserverIntent::Managed(action) => match action {
            ManagedAction::Start { provider, .. } | ManagedAction::Recover { provider, .. } => {
                *provider
            }
            ManagedAction::Archive { .. } | ManagedAction::Restore { .. } => ProviderKind::Codex,
        },
        PendingObserverIntent::NewAtSameLocation { provider, .. } => *provider,
    };
    if provider != ProviderKind::Codex {
        return Some(intent);
    }
    let state = match open_current(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer readiness is unavailable; exact schema-15 state is required",
            );
            return None;
        }
    };
    let evidence = match observer_readiness(root, &state) {
        Ok(evidence) => evidence,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer readiness is unavailable; exact ownership evidence is required",
            );
            return None;
        }
    };
    match evidence.readiness {
        ObserverReadiness::Ready => Some(intent),
        ObserverReadiness::SetupRequired
        | ObserverReadiness::UpdateRequired
        | ObserverReadiness::TrustReviewRequired
        | ObserverReadiness::TrustFinalizationRequired => {
            if pending_observer.is_some() {
                navigator.set_guidance(
                    "Codex observer review is already active; finish that exact review first",
                );
                return None;
            }
            let kind = match evidence.readiness {
                ObserverReadiness::SetupRequired => ObserverSetupKind::Create,
                ObserverReadiness::UpdateRequired => ObserverSetupKind::Update,
                ObserverReadiness::TrustReviewRequired
                | ObserverReadiness::TrustFinalizationRequired => ObserverSetupKind::TrustReview,
                _ => unreachable!("observer setup arm is exhaustive"),
            };
            let presentation_context = match presentation.context() {
                Ok(context) => context,
                Err(_) => {
                    navigator.set_guidance(
                        "Codex observer setup is unavailable; exact presentation evidence changed",
                    );
                    return None;
                }
            };
            let marker = match read_optional_marker(root, presentation) {
                Ok(marker) => marker,
                Err(()) => {
                    navigator.set_guidance(
                        "Codex observer setup is unavailable; exact provisional evidence changed",
                    );
                    return None;
                }
            };
            *pending_observer = Some(PendingObserverSetup {
                intent,
                kind,
                evidence,
                presentation_context,
                marker,
                expected_integration: None,
                review_directory: None,
            });
            navigator.request_observer_setup(kind);
            None
        }
        ObserverReadiness::Modified => {
            navigator.set_guidance(
                "Codex observer profile is modified; it was left untouched and needs exact review",
            );
            None
        }
        ObserverReadiness::Foreign => {
            navigator.set_guidance(
                "Codex observer profile is foreign; it was left untouched and needs exact ownership",
            );
            None
        }
        ObserverReadiness::Disabled => {
            navigator.set_guidance("Codex observer integration is disabled; it was left untouched");
            None
        }
        ObserverReadiness::Ambiguous | ObserverReadiness::Unknown => {
            navigator.set_guidance(
                "Codex observer evidence is ambiguous; no setup or provider launch was attempted",
            );
            None
        }
    }
}

fn read_optional_marker(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<Option<ProvisionalSlot>, ()> {
    match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => Ok(Some(slot)),
        Err(SlotError::MarkerUnavailable) => Ok(None),
        Err(_) => Err(()),
    }
}

/// Applies the Navigator's explicit observer consent to one retained typed
/// action. Profile setup occurs under the exact provisional lease; native
/// review then runs in the right-hand provider pane, and the action remains in
/// process memory until its evidence is revalidated.
#[allow(
    clippy::too_many_lines,
    clippy::manual_let_else,
    clippy::single_match_else,
    reason = "The consent boundary keeps mutation, review launch, and exact revalidation together."
)]
fn accept_observer_setup(
    root: &StateRoot,
    navigator: &mut Navigator,
    presentation: &Presentation,
    kind: ObserverSetupKind,
    pending_observer: &mut Option<PendingObserverSetup>,
) {
    let Some(mut pending) = pending_observer.take() else {
        navigator.set_guidance("Codex observer setup is unavailable; refresh and retry");
        return;
    };
    if pending.kind != kind {
        navigator.set_guidance("Codex observer setup changed; refresh exact state and retry");
        return;
    }
    if presentation.context().ok().as_ref() != Some(&pending.presentation_context)
        || read_optional_marker(root, presentation) != Ok(pending.marker.clone())
    {
        navigator.set_guidance(
            "Codex observer setup is unavailable; exact presentation or provisional evidence changed",
        );
        return;
    }
    let mut state = match open_current(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance("Codex observer setup is unavailable; exact state is required");
            return;
        }
    };
    let provisional_lease = match state.acquire_provisional_lease() {
        Ok(lease) => lease,
        Err(_) => {
            navigator.set_guidance("Codex observer setup is unavailable; exact lease is required");
            return;
        }
    };
    let current_evidence = match observer_readiness(root, &state) {
        Ok(evidence) => evidence,
        Err(_) => {
            navigator
                .set_guidance("Codex observer setup is unavailable; exact ownership is required");
            return;
        }
    };
    if current_evidence != pending.evidence {
        navigator.set_guidance("Codex observer setup changed; refresh exact state and retry");
        return;
    }
    let activation = match prepare_observer_activation(
        root,
        &mut state,
        &provisional_lease,
        &pending.evidence,
    ) {
        Ok(activation) => activation,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer setup is unavailable; exact ownership and Runtime evidence are required",
            );
            return;
        }
    };
    if presentation.context().ok().as_ref() != Some(&pending.presentation_context)
        || read_optional_marker(root, presentation) != Ok(pending.marker.clone())
    {
        navigator.set_guidance(
            "Codex observer setup is unavailable; exact presentation or provisional evidence changed",
        );
        return;
    }
    let ObserverActivation::ReviewRequired(expected) = activation else {
        let command = pending.intent.into_command();
        drop(provisional_lease);
        drop(state);
        let _ = execute_command(command, root, navigator, presentation, pending_observer);
        return;
    };
    let Some(path) = std::env::var_os("PATH") else {
        navigator
            .set_guidance("Codex observer review is unavailable; exact executable is required");
        return;
    };
    let executable = match ExpectedProviderExecutable::resolve_from_path(ProviderKind::Codex, &path)
    {
        Ok(executable) => executable,
        Err(_) => {
            navigator
                .set_guidance("Codex observer review is unavailable; exact executable is required");
            return;
        }
    };
    let Some(codex_home) = expected.ownership.canonical_path.parent() else {
        navigator.set_guidance("Codex observer review is unavailable; exact profile is required");
        return;
    };
    let mut review_directory = match ReviewDirectory::create(
        &presentation.paths().directory,
        pending.presentation_context.presentation_id(),
        pending.presentation_context.presentation_revision(),
    ) {
        Ok(directory) => directory,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer review is unavailable; disposable review state is required",
            );
            return;
        }
    };
    let detached_workstream_id = match presentation.observer_attachment_context() {
        Ok(workstream_id) => workstream_id,
        Err(_) => {
            let _ = review_directory.cleanup();
            navigator.set_guidance(
                "Codex observer review is unavailable; exact outer attachment evidence changed",
            );
            return;
        }
    };
    drop(provisional_lease);
    drop(state);
    if presentation
        .start_observer_review(
            executable.canonical_path(),
            codex_home,
            &review_directory.path(),
            detached_workstream_id,
        )
        .is_err()
    {
        let _ = review_directory.cleanup();
        navigator.set_guidance(
            "Codex observer review is unavailable; the exact provider pane is not free",
        );
        return;
    }
    pending.expected_integration = Some(expected);
    pending.review_directory = Some(review_directory);
    *pending_observer = Some(pending);
    navigator.set_guidance(
        "Complete Codex native /hooks review in the right-hand pane; the selected action resumes after exact trust proof",
    );
}

/// Polls only the exact native review pane. Once it exits, native trust and
/// presentation/marker evidence are revalidated before the retained action is
/// reconstructed and dispatched through the ordinary boundary.
#[allow(
    clippy::manual_let_else,
    clippy::single_match_else,
    clippy::question_mark,
    clippy::map_unwrap_or,
    reason = "Review completion handles bounded cleanup and fail-closed evidence branches explicitly."
)]
fn finish_pending_observer_review(
    root: &StateRoot,
    navigator: &mut Navigator,
    presentation: &Presentation,
    pending_observer: &mut Option<PendingObserverSetup>,
) -> Option<Command> {
    let Some(pending) = pending_observer.as_ref() else {
        return None;
    };
    if pending.review_directory.is_none() {
        return None;
    }
    let finished = match presentation.observer_review_finished() {
        Ok(finished) => finished,
        Err(_) => {
            let pending = pending_observer.take();
            if let Some(pending) = pending
                && let Some(mut directory) = pending.review_directory
            {
                let _ = directory.cleanup();
            }
            navigator.set_guidance(
                "Codex observer review was interrupted; exact provider evidence changed",
            );
            return None;
        }
    };
    if !finished {
        return None;
    }
    let Some(mut pending) = pending_observer.take() else {
        return None;
    };
    let Some(mut review_directory) = pending.review_directory.take() else {
        return None;
    };
    if review_directory.cleanup().is_err() {
        navigator
            .set_guidance("Codex observer review cleanup is unavailable; action remains stopped");
        return None;
    }
    if presentation.context().ok().as_ref() != Some(&pending.presentation_context)
        || read_optional_marker(root, presentation) != Ok(pending.marker.clone())
    {
        navigator.set_guidance(
            "Codex observer review evidence changed; the selected action was not resumed",
        );
        return None;
    }
    let Some(expected) = pending.expected_integration.take() else {
        navigator.set_guidance("Codex observer review evidence is unavailable; refresh and retry");
        return None;
    };
    let mut state = match open_current(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer review finalization is unavailable; exact state is required",
            );
            return None;
        }
    };
    let lease = match state.acquire_provisional_lease() {
        Ok(lease) => lease,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer review finalization is unavailable; exact lease is required",
            );
            return None;
        }
    };
    if finalize_observer_trust_under_lease(root, state, &lease, &expected).is_err() {
        navigator.set_guidance(
            "Codex observer trust remains pending; the selected action was not resumed",
        );
        return None;
    }
    let state = match open_current(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance("Codex observer readiness is unavailable; refresh and retry");
            return None;
        }
    };
    if observer_readiness(root, &state)
        .map(|evidence| evidence.readiness == ObserverReadiness::Ready)
        .unwrap_or(false)
    {
        Some(pending.intent.into_command())
    } else {
        navigator
            .set_guidance("Codex observer readiness remains unavailable; action was not resumed");
        None
    }
}

/// Runs exactly one lifecycle action, refreshes the passive projection,
/// and attaches only a freshly proved non-onboarding Runtime. Every failure
/// remains bounded Navigator guidance; no management text reaches a provider
/// pane.
fn execute_managed_action(
    root: &StateRoot,
    navigator: &mut Navigator,
    presentation: &Presentation,
    action: ManagedAction,
) {
    let restored_workstream = match &action {
        ManagedAction::Restore { workstream_id, .. } => Some(*workstream_id),
        _ => None,
    };
    let outcome = apply_managed_action(root, action);
    let Ok(attachment_workstream) = outcome else {
        navigator
            .set_guidance("Managed session action is unavailable; refresh exact state and retry");
        return;
    };
    let Ok(snapshot) = read_snapshot(root) else {
        navigator.set_guidance("Managed session action completed; refreshed state is unavailable");
        return;
    };
    navigator.replace_snapshot(snapshot);
    if let Some(workstream_id) = restored_workstream {
        if !navigator.select_workstream(workstream_id) {
            navigator.set_guidance("Managed session restored; its active card is unavailable");
        }
        return;
    }
    let Some(workstream_id) = attachment_workstream else {
        return;
    };
    if !navigator.select_workstream(workstream_id) {
        navigator.set_guidance("Managed session action completed; its active card is unavailable");
        return;
    }
    let attachment = navigator_attachment_for(root, workstream_id);
    let Some((workstream_revision, runtime_id, runtime_revision)) = attachment else {
        navigator.set_guidance("Managed session started; exact Runtime attachment is not ready");
        return;
    };
    if presentation
        .attach_workstream(
            workstream_id,
            workstream_revision,
            runtime_id,
            runtime_revision,
        )
        .is_err()
    {
        navigator.set_guidance("Managed session started; exact Runtime attachment is unavailable");
    }
}

/// Re-reads only the bounded snapshot fields needed for an exact post-action
/// attachment.  onboarding rows remain excluded even if the action result
/// is otherwise successful.
fn navigator_attachment_for(
    root: &StateRoot,
    workstream_id: WorkstreamId,
) -> Option<(Revision, RuntimeId, Revision)> {
    let snapshot = read_snapshot(root).ok()?;
    let workstream = snapshot.workstreams.iter().find(|workstream| {
        workstream.workstream_id == workstream_id
            && !workstream.archived
            && workstream.onboarding.is_none()
    })?;
    let runtime = workstream.runtime?;
    Some((workstream.revision, runtime.runtime_id, runtime.revision))
}

/// Executes against the schema-15 registry only after the current
/// snapshot has revalidated the exact action target. The short preflight is a
/// fence against stale navigator commands; durable action routines repeat
/// their own Workstream revision checks before mutation.
#[allow(
    clippy::too_many_lines,
    reason = "one schema-15 action boundary keeps every lifecycle preflight and post-action attachment outcome auditable"
)]
pub(crate) fn apply_managed_action(
    root: &StateRoot,
    action: ManagedAction,
) -> Result<Option<WorkstreamId>, NavigatorError> {
    let snapshot = read_snapshot(root)?;
    let state = open_current(root).map_err(|_| NavigatorError::ManagedActionUnavailable)?;
    let mut registry = state
        .into_host_registry()
        .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
    match action {
        ManagedAction::Start {
            workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            require_active_workstream(
                &snapshot,
                workstream_id,
                expected_workstream_revision,
                Some(provider),
            )?;
            crate::actions::start(
                root,
                &mut registry,
                workstream_id,
                Some(expected_workstream_revision),
            )
            .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
            Ok(Some(workstream_id))
        }
        ManagedAction::Recover {
            workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            require_active_workstream(
                &snapshot,
                workstream_id,
                expected_workstream_revision,
                Some(provider),
            )?;
            crate::actions::recover(
                root,
                &mut registry,
                workstream_id,
                Some(expected_workstream_revision),
            )
            .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
            Ok(Some(workstream_id))
        }
        ManagedAction::Archive {
            workstream_id,
            expected_workstream_revision,
        } => {
            require_archivable_workstream(&snapshot, workstream_id, expected_workstream_revision)?;
            let resolves_onboarding_recovery = snapshot.workstreams.iter().any(|workstream| {
                workstream.workstream_id == workstream_id
                    && workstream.onboarding
                        == Some(crate::snapshot::OnboardingStatus::RecoveryRequired)
            });
            if resolves_onboarding_recovery {
                // Terminal onboarding recovery has no ordinary Start/Recover
                // authority. Reuse the exact Runtime stop path first, close
                // the matching recovery journal, and only then hide the row.
                let stopped_revision = crate::actions::park(
                    root,
                    &mut registry,
                    workstream_id,
                    Some(expected_workstream_revision),
                )
                .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
                drop(registry);
                resolve_parked_onboarding_recovery(root, workstream_id, stopped_revision)?;
                let state =
                    open_current(root).map_err(|_| NavigatorError::ManagedActionUnavailable)?;
                registry = state
                    .into_host_registry()
                    .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
                crate::actions::archive(root, &mut registry, workstream_id, stopped_revision)
                    .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
            } else {
                crate::actions::archive(
                    root,
                    &mut registry,
                    workstream_id,
                    expected_workstream_revision,
                )
                .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
            }
            Ok(None)
        }
        ManagedAction::Restore {
            workstream_id,
            expected_workstream_revision,
        } => {
            let workstream = snapshot
                .workstreams
                .iter()
                .find(|workstream| {
                    workstream.workstream_id == workstream_id
                        && workstream.archived
                        && workstream.revision == expected_workstream_revision
                })
                .ok_or(NavigatorError::ManagedActionUnavailable)?;
            if workstream.onboarding.is_some() {
                return Err(NavigatorError::ManagedActionUnavailable);
            }
            crate::actions::restore(&mut registry, workstream_id, expected_workstream_revision)
                .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
            Ok(None)
        }
    }
}

/// Closes only the terminal recovery journal for an exact Runtime that the
/// preceding archive cleanup already stopped. This does not retry the
/// original provider launch or roll back its binding.
fn resolve_parked_onboarding_recovery(
    root: &StateRoot,
    workstream_id: WorkstreamId,
    expected_workstream_revision: Revision,
) -> Result<(), NavigatorError> {
    let mut state = open_current(root).map_err(|_| NavigatorError::ManagedActionUnavailable)?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| NavigatorError::ManagedActionUnavailable)?;
    state
        .resolve_parked_recovery_current(
            &provisional_lease,
            workstream_id,
            expected_workstream_revision,
        )
        .map_err(|_| NavigatorError::ManagedActionUnavailable)
}

fn require_active_workstream(
    snapshot: &Snapshot,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
    expected_provider: Option<ProviderKind>,
) -> Result<(), NavigatorError> {
    let workstream = snapshot
        .workstreams
        .iter()
        .find(|workstream| {
            workstream.workstream_id == workstream_id
                && !workstream.archived
                && workstream.revision == expected_revision
                && expected_provider.is_none_or(|provider| workstream.provider == provider)
        })
        .ok_or(NavigatorError::ManagedActionUnavailable)?;
    if workstream.onboarding.is_some() {
        return Err(NavigatorError::ManagedActionUnavailable);
    }
    Ok(())
}

/// Archive is available for ordinary active rows and terminal onboarding
/// recovery rows. The `ActionFenced` state permits no lifecycle mutation at
/// all because its provider effect has not reached a terminal boundary.
fn require_archivable_workstream(
    snapshot: &Snapshot,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
) -> Result<(), NavigatorError> {
    let workstream = snapshot
        .workstreams
        .iter()
        .find(|workstream| {
            workstream.workstream_id == workstream_id
                && !workstream.archived
                && workstream.revision == expected_revision
        })
        .ok_or(NavigatorError::ManagedActionUnavailable)?;
    match workstream.onboarding {
        Some(crate::snapshot::OnboardingStatus::ActionFenced) => {
            Err(NavigatorError::ManagedActionUnavailable)
        }
        Some(crate::snapshot::OnboardingStatus::RecoveryRequired) | None => Ok(()),
    }
}

/// One exact post-start attachment claim for a session created from a selected
/// Workstream. No project path, provider option, or shell cwd crosses this
/// boundary: the retained source Location and provider are the authority.
struct SameLocationAttachment {
    workstream_id: crate::domain::WorkstreamId,
    workstream_revision: crate::domain::Revision,
    runtime_id: RuntimeId,
    runtime_revision: crate::domain::Revision,
}

/// Creates an independent native session using only a selected unfenced source
/// Workstream's stored provider and Location, then returns the fresh passive
/// snapshot plus exact attachment revisions. Retired application paths are
/// intentionally never opened here.
fn start_same_location(
    root: &StateRoot,
    source_workstream_id: crate::domain::WorkstreamId,
    expected_workstream_revision: crate::domain::Revision,
    provider: crate::domain::ProviderKind,
) -> Result<(Snapshot, SameLocationAttachment), NavigatorError> {
    let state = open_current(root).map_err(|_| NavigatorError::SameLocationSessionUnavailable)?;
    if state
        .onboarding_workstream_projections()
        .map_err(|_| NavigatorError::SameLocationSessionUnavailable)?
        .iter()
        .any(|onboarding| onboarding.workstream_id == source_workstream_id)
    {
        return Err(NavigatorError::SameLocationSessionUnavailable);
    }
    let mut registry = state
        .into_host_registry()
        .map_err(|_| NavigatorError::SameLocationSessionUnavailable)?;
    let source = registry
        .workstream_overviews()
        .map_err(|_| NavigatorError::SameLocationSessionUnavailable)?
        .into_iter()
        .find(|workstream| workstream.workstream_id == source_workstream_id)
        .ok_or(NavigatorError::SameLocationSessionUnavailable)?;
    if source.revision != expected_workstream_revision
        || source.provider != provider
        || source.archived_at_millis.is_some()
    {
        return Err(NavigatorError::SameLocationSessionUnavailable);
    }
    let request_key = uuid::Uuid::new_v4().simple().to_string();
    let workstream_id = crate::actions::start_independent_workstream(
        root,
        &mut registry,
        source_workstream_id,
        Some(expected_workstream_revision),
        &request_key,
        provider,
    )
    .map_err(|_| NavigatorError::SameLocationSessionUnavailable)?;
    drop(registry);

    let snapshot = read_snapshot(root)?;
    let workstream = snapshot
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .ok_or(NavigatorError::SameLocationSessionUnavailable)?;
    let runtime = workstream
        .runtime
        .ok_or(NavigatorError::SameLocationSessionUnavailable)?;
    if workstream.provider != provider || workstream.onboarding.is_some() {
        return Err(NavigatorError::SameLocationSessionUnavailable);
    }
    let attachment = SameLocationAttachment {
        workstream_id,
        workstream_revision: workstream.revision,
        runtime_id: runtime.runtime_id,
        runtime_revision: runtime.revision,
    };
    Ok((snapshot, attachment))
}

/// Result of observing and reconciling the presentation's provisional marker.
/// Runtime ownership is reported independently from native-exec reconciliation
/// so the selected shell can become its exact managed card immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProviderExecRefresh {
    Idle,
    RuntimeOwned {
        runtime_id: RuntimeId,
        reconciled: bool,
    },
    Unavailable,
}

/// Applies only the reconciliation-owned presentation guidance. Recovery
/// clears that exact message when proof succeeds or the completed marker has
/// retired into the normal idle state; any newer unrelated guidance remains.
fn apply_provider_exec_refresh(navigator: &mut Navigator, refresh: ProviderExecRefresh) {
    match refresh {
        ProviderExecRefresh::Idle => {
            navigator.clear_guidance_if(MANAGED_SESSION_RECONCILIATION_GUIDANCE);
        }
        ProviderExecRefresh::RuntimeOwned { reconciled, .. } => {
            if reconciled {
                navigator.clear_guidance_if(MANAGED_SESSION_RECONCILIATION_GUIDANCE);
            } else {
                navigator.set_guidance(MANAGED_SESSION_RECONCILIATION_GUIDANCE);
            }
        }
        ProviderExecRefresh::Unavailable => {
            navigator.set_guidance(MANAGED_SESSION_RECONCILIATION_GUIDANCE);
        }
    }
}

/// Calls the post-exec controller only after the helper has transferred
/// Runtime ownership. A missing marker is the normal idle-card state; all
/// other valid provisional phases remain owned by the account shell or
/// completed journal.
///
/// The reconciliation adapter never creates a provider process. Its `OpenCode`
/// branch may start only the already-authorized detached observer after exact
/// native-exec proof, and it cannot activate attachment until that observer is
/// both ready and currently live.
fn refresh_provider_exec(root: &StateRoot, presentation: &Presentation) -> ProviderExecRefresh {
    let slot = match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return ProviderExecRefresh::Idle,
        Err(_) => return ProviderExecRefresh::Unavailable,
    };
    if !matches!(
        slot.phase(),
        ProvisionalPhase::RuntimeOwnedLaunching | ProvisionalPhase::ProviderExecProven
    ) {
        return ProviderExecRefresh::Idle;
    }
    let runtime_id = slot.candidate_runtime_id();
    let reconciled =
        reconcile_provider_exec_from_presentation(root.base(), &presentation.paths().directory)
            .is_ok();
    ProviderExecRefresh::RuntimeOwned {
        runtime_id,
        reconciled,
    }
}

/// Composes the shell card with the marker-first materializer.
/// The retained provisional lease spans candidate allocation, account-shell
/// startup/evidence, and outer-pane replacement; no provider command is
/// constructed or launched here.
fn materialize_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<(), NavigatorError> {
    let recovery = reconcile_pre_handoff_presentation(root, presentation)?;
    if recovery == PreHandoffRecovery::RuntimeOwned {
        return Ok(());
    }
    if reattach_materialized_provisional_shell(root, presentation)? {
        return Ok(());
    }
    materialize_provisional_shell_with_inputs(
        root,
        presentation,
        &account_shell_inputs_from_environment()?,
    )
}

/// Runs the passive pre-effect reconciler while retaining the host-wide
/// provisional lease.  A Runtime-owned result is deliberately surfaced to
/// the caller so it cannot be mistaken for a fresh shell slot.
fn reconcile_pre_handoff_presentation(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<PreHandoffRecovery, NavigatorError> {
    let unavailable = || NavigatorError::ProvisionalShellUnavailable;
    let mut state = open_current(root).map_err(|_| unavailable())?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| unavailable())?;
    reconcile_pre_handoff_under_lease(
        &mut state,
        &provisional_lease,
        &presentation.paths().directory,
    )
    .map_err(|_| unavailable())
}

/// Opens the initially selected provisional shell only after fresh
/// presentation startup has finished creating and proving both owned panes.
/// This uses the same marker/lease/materialization path as explicit Shell-card
/// activation and never creates provider or registry authority.
pub(crate) fn materialize_initial_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<(), NavigatorError> {
    materialize_provisional_shell(root, presentation)
}

/// Reattaches the one exact materialized shell after the provider pane has
/// switched to a managed Workstream. Marker absence is the only authority to
/// continue into fresh materialization; every other phase or malformed claim
/// remains a closed refusal and can never create a duplicate candidate.
fn reattach_materialized_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<bool, NavigatorError> {
    let unavailable = || NavigatorError::ProvisionalShellUnavailable;
    let mut state = open_current(root).map_err(|_| unavailable())?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| unavailable())?;
    let slot = match read_marker(state.root(), &presentation.paths().directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return Ok(false),
        Err(_) => return Err(unavailable()),
    };
    if slot.phase() != ProvisionalPhase::Materialized {
        return Err(unavailable());
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    slot.revalidate_live_shell(&runtime, &process_probe)
        .map_err(|_| unavailable())?;
    presentation
        .attach_provisional_shell(&state, &provisional_lease, &slot)
        .map_err(|_| unavailable())?;
    Ok(true)
}

/// The account-shell values are captured once at materialization. They are
/// passed directly into the fixed launch plan; no user RC file is parsed and
/// no ambient provider configuration becomes authority.
struct AccountShellInputs {
    shell: PathBuf,
    home: PathBuf,
    zdotdir: Option<PathBuf>,
    executable: PathBuf,
}

fn account_shell_inputs_from_environment() -> Result<AccountShellInputs, NavigatorError> {
    let unavailable = || NavigatorError::ProvisionalShellUnavailable;
    Ok(AccountShellInputs {
        shell: env::var_os("SHELL")
            .map(PathBuf::from)
            .ok_or_else(unavailable)?,
        home: env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(unavailable)?,
        zdotdir: env::var_os("ZDOTDIR").map(PathBuf::from),
        executable: env::current_exe().map_err(|_| unavailable())?,
    })
}

fn materialize_provisional_shell_with_inputs(
    root: &StateRoot,
    presentation: &Presentation,
    account_shell: &AccountShellInputs,
) -> Result<(), NavigatorError> {
    let unavailable = || NavigatorError::ProvisionalShellUnavailable;
    let context =
        Presentation::context_from_directory(root.base(), &presentation.paths().directory)
            .map_err(|_| unavailable())?;
    let mut state = open_current(root).map_err(|_| unavailable())?;
    let provisional_lease = state
        .acquire_provisional_lease()
        .map_err(|_| unavailable())?;
    let slot = ProvisionalSlot::materializing(
        state.root(),
        context.presentation_id(),
        context.presentation_revision(),
        provisional_lease.lease_generation(),
        RuntimeId::new(),
        SlotGeneration::new(uuid::Uuid::new_v4()),
        context.seed_cwd(),
    )
    .map_err(|_| unavailable())?;
    let account_context = AccountShellContext::new(state.root(), &presentation.paths().directory)
        .map_err(|_| unavailable())?;
    let launch = AccountShellLaunch::new(
        &account_context,
        slot.runtime_paths(),
        context.seed_cwd(),
        &account_shell.shell,
        &account_shell.home,
        account_shell.zdotdir.as_deref(),
        &account_shell.executable,
    )
    .map_err(|_| unavailable())?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
    let materialized = launch
        .materialize_under_lease(
            &state,
            &provisional_lease,
            &presentation.paths().directory,
            &slot,
            &runtime,
            &process_probe,
        )
        .map_err(|_| unavailable())?;
    materialized
        .revalidate_live_shell(&runtime, &process_probe)
        .map_err(|_| unavailable())?;
    presentation
        .attach_provisional_shell(&state, &provisional_lease, &materialized)
        .map_err(|_| unavailable())
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
            EnableMouseCapture,
            EnableFocusChange
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
            DisableFocusChange,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        fs,
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::Duration,
    };

    use uuid::Uuid;

    use super::{
        AccountShellInputs, ManagedAction, ProviderExecRefresh, apply_managed_action,
        apply_provider_exec_refresh, cycle_selection_attempt, describe_shell_location,
        materialize_provisional_shell_with_inputs, observe_shell_cwd,
        reattach_materialized_provisional_shell, refresh_provider_exec, require_active_workstream,
        require_archivable_workstream, retry_default_navigator_width, start_same_location,
    };
    use crate::{
        domain::{
            LocationId, ProjectId, ProviderKind, RandomIdGenerator, Revision, RuntimeId,
            WorkstreamId, WorkstreamLifecycle,
        },
        navigator::view::{Navigator, ShellLocation},
        presentation::INVALID_TOPOLOGY_RETRY_ATTEMPTS,
        presentation::{
            AttachmentPhase, AttachmentPurpose, AttachmentStatus, Presentation, PresentationError,
        },
        process::output_bounded,
        provisional::{ProvisionalPhase, read_marker},
        snapshot::{OnboardingStatus, ProjectSnapshot, Snapshot, WorkstreamSnapshot},
        state::{StateRoot, create_current, open_current},
    };

    struct DisposableTmuxServerGuard(PathBuf);

    impl Drop for DisposableTmuxServerGuard {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .env_remove("TMUX")
                .args(["-S"])
                .arg(&self.0)
                .args(["kill-server"])
                .status();
        }
    }

    fn make_executable(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn create_current_state(state_path: &Path) {
        drop(create_current(state_path, &RandomIdGenerator).unwrap());
    }

    #[test]
    fn navigator_width_retry_absorbs_a_transient_attach_topology() {
        let attempts = Cell::new(0_usize);

        retry_default_navigator_width(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(PresentationError::InvalidTopology)
            } else {
                Ok(())
            }
        })
        .unwrap();

        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn navigator_width_retry_keeps_a_persistent_topology_failure_closed() {
        let attempts = Cell::new(0_usize);

        assert!(matches!(
            retry_default_navigator_width(|| {
                attempts.set(attempts.get() + 1);
                Err(PresentationError::InvalidTopology)
            }),
            Err(PresentationError::InvalidTopology)
        ));
        assert_eq!(attempts.get(), INVALID_TOPOLOGY_RETRY_ATTEMPTS);
    }

    #[test]
    fn navigator_width_retry_does_not_retry_unrelated_presentation_failure() {
        let attempts = Cell::new(0_usize);

        assert!(matches!(
            retry_default_navigator_width(|| {
                attempts.set(attempts.get() + 1);
                Err(PresentationError::ControlRefused("not a topology race"))
            }),
            Err(PresentationError::ControlRefused("not a topology race"))
        ));
        assert_eq!(attempts.get(), 1);
    }

    fn managed_snapshot(onboarding: Option<OnboardingStatus>) -> (Snapshot, WorkstreamId) {
        let project_id = ProjectId::from(Uuid::from_u128(801));
        let location_id = LocationId::from(Uuid::from_u128(802));
        let workstream_id = WorkstreamId::from(Uuid::from_u128(803));
        (
            Snapshot {
                projects: vec![ProjectSnapshot {
                    project_id,
                    display_name: "checkout".to_owned(),
                    locations: vec![],
                }],
                workstreams: vec![WorkstreamSnapshot {
                    project_id,
                    location_id,
                    workstream_id,
                    provider: ProviderKind::Codex,
                    lifecycle: WorkstreamLifecycle::Open,
                    archived: false,
                    last_activity_sequence: 1,
                    last_activity_at_millis: None,
                    revision: Revision::INITIAL,
                    runtime: None,
                    onboarding,
                    native_name: None,
                }],
                unresolved_operations: vec![],
            },
            workstream_id,
        )
    }

    #[test]
    fn cycle_selection_requires_running_provider_cycle_once_per_attempt() {
        let attempt = Uuid::from_u128(810);
        let status = AttachmentStatus {
            attempt_id: attempt,
            workstream_id: WorkstreamId::from(Uuid::from_u128(811)),
            phase: AttachmentPhase::Running,
            purpose: AttachmentPurpose::ProviderCycle,
        };
        assert_eq!(cycle_selection_attempt(&status, None), Some(attempt));
        assert_eq!(cycle_selection_attempt(&status, Some(attempt)), None);

        for phase in [
            AttachmentPhase::Pending,
            AttachmentPhase::Completed,
            AttachmentPhase::Failed,
        ] {
            let status = AttachmentStatus {
                phase,
                ..status.clone()
            };
            assert_eq!(cycle_selection_attempt(&status, None), None);
        }
        let status = AttachmentStatus {
            purpose: AttachmentPurpose::Ordinary,
            ..status
        };
        assert_eq!(cycle_selection_attempt(&status, None), None);
    }

    #[test]
    fn provider_exec_guidance_clears_after_exact_recovery_or_normal_idle() {
        let (snapshot, _) = managed_snapshot(None);
        let mut navigator = Navigator::new(snapshot);

        apply_provider_exec_refresh(&mut navigator, ProviderExecRefresh::Unavailable);
        assert_eq!(
            navigator.model_mut().guidance(),
            Some("Managed session reconciliation is unavailable; exact recovery required")
        );

        apply_provider_exec_refresh(
            &mut navigator,
            ProviderExecRefresh::RuntimeOwned {
                runtime_id: RuntimeId::new(),
                reconciled: true,
            },
        );
        assert_eq!(navigator.model_mut().guidance(), None);

        apply_provider_exec_refresh(
            &mut navigator,
            ProviderExecRefresh::RuntimeOwned {
                runtime_id: RuntimeId::new(),
                reconciled: false,
            },
        );
        assert_eq!(
            navigator.model_mut().guidance(),
            Some("Managed session reconciliation is unavailable; exact recovery required")
        );
        apply_provider_exec_refresh(&mut navigator, ProviderExecRefresh::Idle);
        assert_eq!(navigator.model_mut().guidance(), None);
    }

    #[test]
    fn provider_exec_guidance_recovery_preserves_newer_unrelated_guidance() {
        let (snapshot, _) = managed_snapshot(None);
        let mut navigator = Navigator::new(snapshot);

        apply_provider_exec_refresh(
            &mut navigator,
            ProviderExecRefresh::RuntimeOwned {
                runtime_id: RuntimeId::new(),
                reconciled: false,
            },
        );
        navigator.set_guidance("another command failed");

        apply_provider_exec_refresh(
            &mut navigator,
            ProviderExecRefresh::RuntimeOwned {
                runtime_id: RuntimeId::new(),
                reconciled: true,
            },
        );
        assert_eq!(
            navigator.model_mut().guidance(),
            Some("another command failed")
        );

        apply_provider_exec_refresh(&mut navigator, ProviderExecRefresh::Idle);
        assert_eq!(
            navigator.model_mut().guidance(),
            Some("another command failed")
        );
    }

    #[test]
    fn current_action_preflight_keeps_onboarding_fences_out_of_lifecycle_actions() {
        let (fenced, workstream_id) = managed_snapshot(Some(OnboardingStatus::ActionFenced));
        assert!(
            require_active_workstream(
                &fenced,
                workstream_id,
                Revision::INITIAL,
                Some(ProviderKind::Codex),
            )
            .is_err()
        );
        assert!(require_archivable_workstream(&fenced, workstream_id, Revision::INITIAL).is_err());

        let (recovery, workstream_id) = managed_snapshot(Some(OnboardingStatus::RecoveryRequired));
        assert!(
            require_active_workstream(
                &recovery,
                workstream_id,
                Revision::INITIAL,
                Some(ProviderKind::Codex),
            )
            .is_err()
        );
        assert!(require_archivable_workstream(&recovery, workstream_id, Revision::INITIAL).is_ok());

        let (ordinary, workstream_id) = managed_snapshot(None);
        assert!(
            require_active_workstream(
                &ordinary,
                workstream_id,
                Revision::INITIAL,
                Some(ProviderKind::Codex),
            )
            .is_ok()
        );
    }

    #[test]
    fn current_restore_uses_only_exact_durable_revisions() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let mut state = create_current(&state_path, &RandomIdGenerator).unwrap();
        let (_, workstream_id) = state
            .seed_test_workstream(
                &checkout,
                "checkout",
                ProviderKind::OpenCode,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);
        let root = StateRoot::select(&state_path);
        let state = open_current(&root).unwrap();
        let mut registry = state.into_host_registry().unwrap();
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        let archived_revision = registry
            .archive_workstream(overview.workstream_id, overview.revision, 1)
            .unwrap();
        drop(registry);

        assert_eq!(
            apply_managed_action(
                &root,
                ManagedAction::Restore {
                    workstream_id,
                    expected_workstream_revision: archived_revision,
                },
            )
            .unwrap(),
            None
        );
        let restored = crate::snapshot::read_snapshot(&root).unwrap();
        assert!(!restored.workstreams[0].archived);
    }

    fn wait_for_private_client(socket: &Path) {
        for _ in 0..50 {
            let mut command = Command::new("tmux");
            command.env_remove("TMUX").args(["-S"]).arg(socket).args([
                "list-clients",
                "-F",
                "#{client_name}",
            ]);
            let output = output_bounded(&mut command, 4 * 1024, 4 * 1024).unwrap();
            if output.status.success() && !output.stdout.is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the provisional Runtime never received the outer provider-pane client");
    }

    #[test]
    fn materialized_shell_stays_unregistered_and_attaches_only_its_private_runtime() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        let home = temporary.path().join("home");
        fs::create_dir(&seed).unwrap();
        fs::create_dir(&home).unwrap();
        create_current_state(&state_path);

        let navigator = temporary.path().join("navigator-fixture");
        make_executable(&navigator, "#!/bin/sh\nexec sleep 60\n");
        let presentation = Presentation::fresh_with_executable(&state_path, navigator);
        presentation.start(Uuid::from_u128(91), &seed).unwrap();
        let _presentation_guard = DisposableTmuxServerGuard(presentation.paths().socket.clone());

        let shell = [PathBuf::from("/usr/bin/bash"), PathBuf::from("/bin/bash")]
            .into_iter()
            .find(|candidate| candidate.is_file())
            .expect("a supported Bash account shell is required for acceptance");
        let inputs = AccountShellInputs {
            shell,
            home,
            zdotdir: None,
            executable: std::env::current_exe().unwrap(),
        };
        let root = StateRoot::select(&state_path);

        materialize_provisional_shell_with_inputs(&root, &presentation, &inputs).unwrap();
        assert_eq!(
            refresh_provider_exec(&root, &presentation),
            ProviderExecRefresh::Idle
        );

        let marker = read_marker(root.base(), &presentation.paths().directory).unwrap();
        assert_eq!(marker.phase(), ProvisionalPhase::Materialized);
        let _runtime_guard = DisposableTmuxServerGuard(marker.runtime_paths().socket.clone());
        wait_for_private_client(&marker.runtime_paths().socket);
        assert_eq!(
            observe_shell_cwd(&root, &presentation, &seed).unwrap(),
            seed.canonicalize().unwrap()
        );

        assert!(reattach_materialized_provisional_shell(&root, &presentation).unwrap());
        assert_eq!(
            read_marker(root.base(), &presentation.paths().directory).unwrap(),
            marker
        );

        let state = open_current(&root).unwrap();
        assert!(state.registered_runtime_paths().unwrap().is_empty());
        drop(state);

        presentation.close().unwrap();
        assert!(!presentation.paths().directory.exists());
        assert!(!marker.runtime_paths().directory.exists());
    }

    #[test]
    fn shell_location_abbreviates_cwd_without_git_display_discovery() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let nested = home.join("checkout/nested");
        let notes = home.join("notes");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&notes).unwrap();

        assert_eq!(
            describe_shell_location(&nested, Some(&home)),
            ShellLocation::cwd("~/c/nested")
        );
        assert_eq!(
            describe_shell_location(&notes, Some(&home)),
            ShellLocation::cwd("~/notes")
        );
    }

    #[test]
    fn idle_presentation_does_not_open_a_provider_reconciler() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        create_current_state(&state_path);

        let navigator = temporary.path().join("navigator-fixture");
        make_executable(&navigator, "#!/bin/sh\nexec sleep 60\n");
        let presentation = Presentation::fresh_with_executable(&state_path, navigator);
        presentation.start(Uuid::from_u128(92), &seed).unwrap();
        let _presentation_guard = DisposableTmuxServerGuard(presentation.paths().socket.clone());

        assert_eq!(
            refresh_provider_exec(&StateRoot::select(&state_path), &presentation),
            ProviderExecRefresh::Idle
        );
        assert!(
            !reattach_materialized_provisional_shell(
                &StateRoot::select(&state_path),
                &presentation,
            )
            .unwrap()
        );
    }

    #[test]
    fn same_location_new_refuses_an_archived_source_before_provider_start() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let mut state = create_current(&state_path, &RandomIdGenerator).unwrap();
        let (_, source_workstream_id) = state
            .seed_test_workstream(
                &checkout,
                "checkout",
                ProviderKind::Codex,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);
        let root = StateRoot::select(&state_path);
        let state = open_current(&root).unwrap();
        let mut registry = state.into_host_registry().unwrap();
        let source_overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|workstream| workstream.workstream_id == source_workstream_id)
            .unwrap();
        let archived_revision = registry
            .archive_workstream(source_workstream_id, source_overview.revision, 1)
            .unwrap();
        drop(registry);

        assert!(
            start_same_location(
                &root,
                source_workstream_id,
                archived_revision,
                ProviderKind::Codex,
            )
            .is_err()
        );
        assert_eq!(
            crate::snapshot::read_snapshot(&root)
                .unwrap()
                .workstreams
                .len(),
            1
        );
    }
}
