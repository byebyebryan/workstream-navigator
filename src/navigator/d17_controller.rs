//! D17 schema-14 Navigator pane.
//!
//! This controller owns terminal setup, passive snapshots, shell-card
//! materialization, and exact presentation attachment. It is reached solely
//! from the hidden D17 presentation-pane command.

use std::{
    env,
    ffi::OsStr,
    io::{self, Stdout},
    path::{Component, Path, PathBuf},
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
    app::observer::{
        ObserverActivation, ObserverReadiness, ObserverReadinessEvidence,
        finalize_observer_trust_d17_under_lease, observer_readiness,
        prepare_observer_activation_d17,
    },
    d17_account_shell::{AccountShellContext, AccountShellLaunch},
    d17_reconcile::ExpectedProviderExecutable,
    d17_review::D17ReviewDirectory,
    d17_shell_control::reconcile_provider_exec_from_presentation,
    d17_snapshot::{D17Snapshot, D17SnapshotError, read_snapshot},
    domain::{OperationId, ProviderKind, Revision, RuntimeId, WorkstreamId},
    presentation::{Presentation, PresentationError},
    provisional::{
        PreHandoffRecovery, ProvisionalPhase, ProvisionalSlot, SlotError, SlotGeneration,
        read_marker, reconcile_pre_handoff_under_lease,
    },
    runtime::{LinuxProcessProbe, PrivateRuntime, SystemTmux},
    state::{StateRoot, open_d17_current_only},
};

use super::d17::{D17Command, D17Navigator, D17ObserverSetupKind, D17ShellLocation};

/// Errors that prevent the D17 pane from rendering its schema-14-only
/// Workstreams view.
#[derive(Debug, Error)]
pub(crate) enum D17NavigatorError {
    #[error("D17 navigator terminal setup failed: {0}")]
    Terminal(#[source] io::Error),
    #[error("D17 navigator presentation setup failed: {0}")]
    Presentation(#[from] PresentationError),
    #[error("D17 navigator state is unavailable: {0}")]
    Snapshot(#[from] D17SnapshotError),
    #[error("D17 provisional shell is unavailable")]
    ProvisionalShellUnavailable,
    #[error("D17 same-location session creation is unavailable")]
    SameLocationSessionUnavailable,
    #[error("D17 managed action is unavailable")]
    ManagedActionUnavailable,
}

/// Runs the hidden schema-14 D17 Navigator pane. It validates the exact D17
/// presentation context before reading state. The provisional-shell command is
/// a lease-held marker-first materialization followed by an outer-pane attach.
#[allow(
    clippy::too_many_lines,
    reason = "The D17 loop keeps shell, promotion, exact attachment, and focus ordering in one auditable owner."
)]
pub(crate) fn run_d17_navigator(
    root: &StateRoot,
    socket: PathBuf,
    session_name: String,
) -> Result<(), D17NavigatorError> {
    let presentation = Presentation::from_control(root.base(), socket, session_name)?;
    let context =
        Presentation::d17_context_from_directory(root.base(), &presentation.paths().directory)
            .map_err(|_| PresentationError::D17ContextUnavailable)?;
    let seed_cwd = context.seed_cwd().to_path_buf();
    let home = env::var_os("HOME").map(PathBuf::from);
    // Reconcile any interrupted pre-effect marker before taking the passive
    // snapshot.  A failure stays visible as bounded guidance; it must not
    // become permission to allocate a replacement candidate.
    let startup_recovery = reconcile_pre_handoff_presentation(root, &presentation);
    let snapshot = read_snapshot(root)?;
    let mut navigator = D17Navigator::new(snapshot);
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
    let mut terminal = TerminalSession::enter().map_err(D17NavigatorError::Terminal)?;
    let mut redraw = true;
    let mut last_refresh = Instant::now();
    let mut mouse_down = None;
    let mut promoted_runtime = None;
    let mut pending_observer = None;

    let quit = loop {
        if redraw {
            terminal
                .terminal
                .draw(|frame| navigator.render(frame, frame.area()))
                .map_err(D17NavigatorError::Terminal)?;
            redraw = false;
        }
        if event::poll(Duration::from_millis(100)).map_err(D17NavigatorError::Terminal)? {
            match event::read().map_err(D17NavigatorError::Terminal)? {
                Event::Key(key) => {
                    let command = navigator.handle_key(key.code);
                    if execute_d17_command(
                        command,
                        root,
                        &mut navigator,
                        &presentation,
                        FocusAfter::Provider,
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
                            let size = terminal
                                .terminal
                                .size()
                                .map_err(D17NavigatorError::Terminal)?;
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
                                .map_err(D17NavigatorError::Terminal)?;
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
                        && execute_d17_command(
                            command,
                            root,
                            &mut navigator,
                            &presentation,
                            FocusAfter::Navigator,
                            &mut pending_observer,
                        )
                    {
                        break true;
                    }
                    redraw = true;
                }
                Event::Resize(_, _) => {
                    if presentation.set_default_navigator_width().is_err() {
                        navigator.set_guidance(
                            "Navigator resize is unavailable; exact presentation evidence changed",
                        );
                    }
                    redraw = true;
                }
                _ => {}
            }
        }
        if let Some(command) = finish_pending_observer_review(
            root,
            &mut navigator,
            &presentation,
            &mut pending_observer,
        ) && execute_d17_command(
            command,
            root,
            &mut navigator,
            &presentation,
            FocusAfter::Provider,
            &mut pending_observer,
        ) {
            break true;
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            match refresh_provider_exec(root, &presentation) {
                ProviderExecRefresh::Idle => {}
                ProviderExecRefresh::RuntimeOwned {
                    runtime_id,
                    reconciled,
                } => {
                    promoted_runtime = Some(runtime_id);
                    if !reconciled {
                        navigator.set_guidance(
                            "Managed session reconciliation is unavailable; exact recovery required",
                        );
                    }
                }
                ProviderExecRefresh::Unavailable => navigator.set_guidance(
                    "Managed session reconciliation is unavailable; exact recovery required",
                ),
            }
            if let Ok(snapshot) = read_snapshot(root) {
                navigator.replace_snapshot(snapshot);
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
        presentation.stop_d17_session()?;
    }
    Ok(())
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
    navigator: &mut D17Navigator,
) {
    let cwd = observe_shell_cwd(root, presentation, seed_cwd).ok();
    if cwd.as_ref() == observed.as_ref() {
        return;
    }
    let location = cwd.as_ref().map_or_else(
        || D17ShellLocation::cwd("unavailable"),
        |cwd| describe_shell_location(cwd, home),
    );
    navigator.set_shell_location(location);
    *observed = cwd;
}

fn observe_shell_cwd(
    root: &StateRoot,
    presentation: &Presentation,
    seed_cwd: &Path,
) -> Result<PathBuf, D17NavigatorError> {
    let slot = match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return Ok(seed_cwd.to_path_buf()),
        Err(_) => return Err(D17NavigatorError::ProvisionalShellUnavailable),
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
        .map_err(|_| D17NavigatorError::ProvisionalShellUnavailable)
}

fn describe_shell_location(cwd: &Path, home: Option<&Path>) -> D17ShellLocation {
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
    D17ShellLocation::cwd(&cwd_label)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusAfter {
    Provider,
    Navigator,
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
    fn into_command(self) -> D17Command {
        match self {
            Self::Managed(action) => match action {
                ManagedAction::Start {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                } => D17Command::Start {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                },
                ManagedAction::Recover {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                } => D17Command::Recover {
                    workstream_id,
                    expected_workstream_revision,
                    provider,
                },
                ManagedAction::Fork {
                    source_workstream_id,
                    expected_workstream_revision,
                    provider,
                } => D17Command::Fork {
                    source_workstream_id,
                    expected_workstream_revision,
                    provider,
                },
                ManagedAction::RecoverOperation {
                    operation_id,
                    expected_operation_revision,
                    provider,
                } => D17Command::RecoverOperation {
                    operation_id,
                    expected_operation_revision,
                    provider,
                },
                ManagedAction::Park {
                    workstream_id,
                    expected_workstream_revision,
                } => D17Command::Park {
                    workstream_id,
                    expected_workstream_revision,
                },
                ManagedAction::Archive {
                    workstream_id,
                    expected_workstream_revision,
                } => D17Command::Archive {
                    workstream_id,
                    expected_workstream_revision,
                },
                ManagedAction::Restore {
                    workstream_id,
                    expected_workstream_revision,
                } => D17Command::Restore {
                    workstream_id,
                    expected_workstream_revision,
                },
                ManagedAction::AcknowledgeResult {
                    workstream_id,
                    expected_attention_revision,
                } => D17Command::AcknowledgeResult {
                    workstream_id,
                    expected_attention_revision,
                },
                ManagedAction::Rename {
                    workstream_id,
                    expected_workstream_revision,
                    name,
                } => D17Command::Rename {
                    workstream_id,
                    expected_workstream_revision,
                    name,
                },
            },
            Self::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            } => D17Command::NewAtSameLocation {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            },
        }
    }
}

struct PendingObserverSetup {
    intent: PendingObserverIntent,
    kind: D17ObserverSetupKind,
    evidence: ObserverReadinessEvidence,
    presentation_context: crate::presentation::D17PresentationContext,
    marker: Option<ProvisionalSlot>,
    expected_integration: Option<crate::state::CodexIntegration>,
    review_directory: Option<D17ReviewDirectory>,
}

/// Executes one D17 model command while keeping keyboard- and mouse-originated
/// focus policy explicit. Mouse activation switches the provider attachment
/// but leaves keyboard focus in Navigator.
#[allow(
    clippy::too_many_lines,
    reason = "The small D17 command set keeps exact attachment and focus outcomes in one controller seam."
)]
fn execute_d17_command(
    command: D17Command,
    root: &StateRoot,
    navigator: &mut D17Navigator,
    presentation: &Presentation,
    focus_after: FocusAfter,
    pending_observer: &mut Option<PendingObserverSetup>,
) -> bool {
    match command {
        D17Command::Quit => true,
        D17Command::MaterializeProvisionalShell => {
            if materialize_provisional_shell(root, presentation).is_ok() {
                if focus_after == FocusAfter::Provider && presentation.focus_provider().is_err() {
                    navigator.set_guidance("Shell opened; provider-pane focus is unavailable");
                }
            } else {
                navigator.set_guidance("New session shell unavailable; exact state required");
            }
            false
        }
        D17Command::Attach {
            workstream_id,
            expected_workstream_revision,
            runtime_id,
            expected_runtime_revision,
        } => {
            if presentation
                .attach_d17_workstream(
                    workstream_id,
                    expected_workstream_revision,
                    runtime_id,
                    expected_runtime_revision,
                )
                .is_ok()
            {
                if focus_after == FocusAfter::Provider && presentation.focus_provider().is_err() {
                    navigator
                        .set_guidance("Managed session opened; provider-pane focus is unavailable");
                }
            } else {
                navigator.set_guidance(
                    "Managed session is unavailable; exact Runtime evidence required",
                );
            }
            false
        }
        D17Command::NewAtSameLocation {
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
            match start_d17_same_location(
                root,
                source_workstream_id,
                expected_workstream_revision,
                provider,
            ) {
                Ok((snapshot, attachment)) => {
                    navigator.replace_snapshot(snapshot);
                    navigator.select_runtime(attachment.runtime_id);
                    if presentation
                        .attach_d17_workstream(
                            attachment.workstream_id,
                            attachment.workstream_revision,
                            attachment.runtime_id,
                            attachment.runtime_revision,
                        )
                        .is_ok()
                    {
                        if focus_after == FocusAfter::Provider
                            && presentation.focus_provider().is_err()
                        {
                            navigator.set_guidance(
                                "New session started; provider-pane focus is unavailable",
                            );
                        }
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
        D17Command::Start {
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
                focus_after,
                provider,
                action,
                pending_observer,
            );
            false
        }
        D17Command::Recover {
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
                focus_after,
                provider,
                action,
                pending_observer,
            );
            false
        }
        D17Command::Park {
            workstream_id,
            expected_workstream_revision,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                focus_after,
                ManagedAction::Park {
                    workstream_id,
                    expected_workstream_revision,
                },
            );
            false
        }
        D17Command::Archive {
            workstream_id,
            expected_workstream_revision,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                focus_after,
                ManagedAction::Archive {
                    workstream_id,
                    expected_workstream_revision,
                },
            );
            false
        }
        D17Command::Restore {
            workstream_id,
            expected_workstream_revision,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                focus_after,
                ManagedAction::Restore {
                    workstream_id,
                    expected_workstream_revision,
                },
            );
            false
        }
        D17Command::AcknowledgeResult {
            workstream_id,
            expected_attention_revision,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                focus_after,
                ManagedAction::AcknowledgeResult {
                    workstream_id,
                    expected_attention_revision,
                },
            );
            false
        }
        D17Command::Fork {
            source_workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            let action = ManagedAction::Fork {
                source_workstream_id,
                expected_workstream_revision,
                provider,
            };
            execute_managed_action_or_request(
                root,
                navigator,
                presentation,
                focus_after,
                provider,
                action,
                pending_observer,
            );
            false
        }
        D17Command::RecoverOperation {
            operation_id,
            expected_operation_revision,
            provider,
        } => {
            let action = ManagedAction::RecoverOperation {
                operation_id,
                expected_operation_revision,
                provider,
            };
            execute_managed_action_or_request(
                root,
                navigator,
                presentation,
                focus_after,
                provider,
                action,
                pending_observer,
            );
            false
        }
        D17Command::AcceptObserverSetup { kind } => {
            accept_observer_setup(root, navigator, presentation, kind, pending_observer);
            false
        }
        D17Command::CancelObserverSetup => {
            pending_observer.take();
            navigator.set_guidance(
                "Codex observer setup was declined; no profile or trust state was changed",
            );
            false
        }
        D17Command::Rename {
            workstream_id,
            expected_workstream_revision,
            name,
        } => {
            execute_managed_action(
                root,
                navigator,
                presentation,
                focus_after,
                ManagedAction::Rename {
                    workstream_id,
                    expected_workstream_revision,
                    name,
                },
            );
            false
        }
        D17Command::ShowGuidance(guidance) => {
            navigator.set_guidance(guidance);
            false
        }
        D17Command::None => false,
    }
}

/// Schema-14-native managed lifecycle intent. This deliberately bypasses the
/// retired schema-13 application facade: each variant carries only the exact
/// durable IDs/revisions supplied by the passive D17 snapshot.
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
    Park {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Archive {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    Restore {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
    },
    AcknowledgeResult {
        workstream_id: WorkstreamId,
        expected_attention_revision: Revision,
    },
    Fork {
        source_workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        provider: ProviderKind,
    },
    RecoverOperation {
        operation_id: OperationId,
        expected_operation_revision: Revision,
        provider: ProviderKind,
    },
    Rename {
        workstream_id: WorkstreamId,
        expected_workstream_revision: Revision,
        name: String,
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
    navigator: &mut D17Navigator,
    presentation: &Presentation,
    focus_after: FocusAfter,
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
    execute_managed_action(root, navigator, presentation, focus_after, action);
}

/// Reads the exact schema-14 observer state without reserving onboarding
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
    navigator: &mut D17Navigator,
    presentation: &Presentation,
    intent: PendingObserverIntent,
    pending_observer: &mut Option<PendingObserverSetup>,
) -> Option<PendingObserverIntent> {
    let provider = match &intent {
        PendingObserverIntent::Managed(action) => match action {
            ManagedAction::Start { provider, .. }
            | ManagedAction::Recover { provider, .. }
            | ManagedAction::Fork { provider, .. }
            | ManagedAction::RecoverOperation { provider, .. } => *provider,
            ManagedAction::Park { .. }
            | ManagedAction::Archive { .. }
            | ManagedAction::Restore { .. }
            | ManagedAction::AcknowledgeResult { .. }
            | ManagedAction::Rename { .. } => ProviderKind::Codex,
        },
        PendingObserverIntent::NewAtSameLocation { provider, .. } => *provider,
    };
    if provider != ProviderKind::Codex {
        return Some(intent);
    }
    let state = match open_d17_current_only(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer readiness is unavailable; exact schema-14 state is required",
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
                ObserverReadiness::SetupRequired => D17ObserverSetupKind::Create,
                ObserverReadiness::UpdateRequired => D17ObserverSetupKind::Update,
                ObserverReadiness::TrustReviewRequired
                | ObserverReadiness::TrustFinalizationRequired => D17ObserverSetupKind::TrustReview,
                _ => unreachable!("observer setup arm is exhaustive"),
            };
            let presentation_context = match presentation.d17_context() {
                Ok(context) => context,
                Err(_) => {
                    navigator.set_guidance(
                        "Codex observer setup is unavailable; exact presentation evidence changed",
                    );
                    return None;
                }
            };
            let marker = match read_optional_d17_marker(root, presentation) {
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

fn read_optional_d17_marker(
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
/// action. Profile setup occurs under the exact D17 provisional lease; native
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
    navigator: &mut D17Navigator,
    presentation: &Presentation,
    kind: D17ObserverSetupKind,
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
    if presentation.d17_context().ok().as_ref() != Some(&pending.presentation_context)
        || read_optional_d17_marker(root, presentation) != Ok(pending.marker.clone())
    {
        navigator.set_guidance(
            "Codex observer setup is unavailable; exact presentation or provisional evidence changed",
        );
        return;
    }
    let mut state = match open_d17_current_only(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance("Codex observer setup is unavailable; exact state is required");
            return;
        }
    };
    let provisional_lease = match state.acquire_d17_provisional_lease() {
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
    let activation = match prepare_observer_activation_d17(
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
    if presentation.d17_context().ok().as_ref() != Some(&pending.presentation_context)
        || read_optional_d17_marker(root, presentation) != Ok(pending.marker.clone())
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
        let _ = execute_d17_command(
            command,
            root,
            navigator,
            presentation,
            FocusAfter::Provider,
            pending_observer,
        );
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
    let mut review_directory = match D17ReviewDirectory::create(
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
    let detached_workstream_id = match presentation.d17_observer_attachment_context() {
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
        .start_d17_observer_review(
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
    if presentation.focus_provider().is_err() {
        navigator.set_guidance(
            "Complete Codex native /hooks review; provider-pane focus is unavailable",
        );
    } else {
        navigator.set_guidance(
            "Complete Codex native /hooks review in the right-hand pane; the selected action resumes after exact trust proof",
        );
    }
}

/// Polls only the exact native review pane. Once it exits, native trust and
/// presentation/marker evidence are revalidated before the retained action is
/// reconstructed and dispatched through the ordinary D17 boundary.
#[allow(
    clippy::manual_let_else,
    clippy::single_match_else,
    clippy::question_mark,
    clippy::map_unwrap_or,
    reason = "Review completion handles bounded cleanup and fail-closed evidence branches explicitly."
)]
fn finish_pending_observer_review(
    root: &StateRoot,
    navigator: &mut D17Navigator,
    presentation: &Presentation,
    pending_observer: &mut Option<PendingObserverSetup>,
) -> Option<D17Command> {
    let Some(pending) = pending_observer.as_ref() else {
        return None;
    };
    if pending.review_directory.is_none() {
        return None;
    }
    let finished = match presentation.d17_observer_review_finished() {
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
    if presentation.d17_context().ok().as_ref() != Some(&pending.presentation_context)
        || read_optional_d17_marker(root, presentation) != Ok(pending.marker.clone())
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
    let mut state = match open_d17_current_only(root) {
        Ok(state) => state,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer review finalization is unavailable; exact state is required",
            );
            return None;
        }
    };
    let lease = match state.acquire_d17_provisional_lease() {
        Ok(lease) => lease,
        Err(_) => {
            navigator.set_guidance(
                "Codex observer review finalization is unavailable; exact lease is required",
            );
            return None;
        }
    };
    if finalize_observer_trust_d17_under_lease(root, state, &lease, &expected).is_err() {
        navigator.set_guidance(
            "Codex observer trust remains pending; the selected action was not resumed",
        );
        return None;
    }
    let state = match open_d17_current_only(root) {
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

/// Runs exactly one D17 lifecycle action, refreshes the passive projection,
/// and attaches only a freshly proved non-onboarding Runtime. Every failure
/// remains bounded Navigator guidance; no management text reaches a provider
/// pane.
fn execute_managed_action(
    root: &StateRoot,
    navigator: &mut D17Navigator,
    presentation: &Presentation,
    focus_after: FocusAfter,
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
        .attach_d17_workstream(
            workstream_id,
            workstream_revision,
            runtime_id,
            runtime_revision,
        )
        .is_err()
    {
        navigator.set_guidance("Managed session started; exact Runtime attachment is unavailable");
    } else if focus_after == FocusAfter::Provider && presentation.focus_provider().is_err() {
        navigator.set_guidance("Managed session started; provider-pane focus is unavailable");
    }
}

/// Re-reads only the bounded snapshot fields needed for an exact post-action
/// attachment. D17 onboarding rows remain excluded even if the action result
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

/// Executes against the schema-14 registry only after the current D17
/// snapshot has revalidated the exact action target. The short preflight is a
/// fence against stale navigator commands; durable action routines repeat
/// their own Workstream/attention revision checks before mutation.
#[allow(
    clippy::too_many_lines,
    reason = "one schema-14 action boundary keeps every lifecycle preflight and post-action attachment outcome auditable"
)]
pub(crate) fn apply_managed_action(
    root: &StateRoot,
    action: ManagedAction,
) -> Result<Option<WorkstreamId>, D17NavigatorError> {
    let snapshot = read_snapshot(root)?;
    let state =
        open_d17_current_only(root).map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
    let mut registry = state
        .into_d17_host_registry()
        .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
    match action {
        ManagedAction::Start {
            workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            require_active_d17_workstream(
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
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(Some(workstream_id))
        }
        ManagedAction::Recover {
            workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            require_active_d17_workstream(
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
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(Some(workstream_id))
        }
        ManagedAction::Park {
            workstream_id,
            expected_workstream_revision,
        } => {
            require_parkable_d17_workstream(
                &snapshot,
                workstream_id,
                expected_workstream_revision,
            )?;
            let resolves_onboarding_recovery = snapshot.workstreams.iter().any(|workstream| {
                workstream.workstream_id == workstream_id
                    && workstream.onboarding
                        == Some(crate::d17_snapshot::D17OnboardingStatus::RecoveryRequired)
            });
            let parked_revision = crate::actions::park(
                root,
                &mut registry,
                workstream_id,
                Some(expected_workstream_revision),
            )
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            drop(registry);
            if resolves_onboarding_recovery {
                resolve_parked_onboarding_recovery(root, workstream_id, parked_revision)?;
            }
            Ok(None)
        }
        ManagedAction::Archive {
            workstream_id,
            expected_workstream_revision,
        } => {
            require_active_d17_workstream(
                &snapshot,
                workstream_id,
                expected_workstream_revision,
                None,
            )?;
            crate::actions::archive(
                root,
                &mut registry,
                workstream_id,
                expected_workstream_revision,
            )
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
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
                .ok_or(D17NavigatorError::ManagedActionUnavailable)?;
            if workstream.onboarding.is_some() {
                return Err(D17NavigatorError::ManagedActionUnavailable);
            }
            crate::actions::restore(&mut registry, workstream_id, expected_workstream_revision)
                .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(None)
        }
        ManagedAction::AcknowledgeResult {
            workstream_id,
            expected_attention_revision,
        } => {
            snapshot
                .workstreams
                .iter()
                .find(|workstream| {
                    workstream.workstream_id == workstream_id
                        && workstream.onboarding.is_none()
                        && workstream.result_unseen
                        && workstream.attention_revision == expected_attention_revision
                })
                .ok_or(D17NavigatorError::ManagedActionUnavailable)?;
            registry
                .acknowledge_result_attention(workstream_id, expected_attention_revision)
                .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(None)
        }
        ManagedAction::Fork {
            source_workstream_id,
            expected_workstream_revision,
            provider,
        } => {
            require_active_d17_workstream(
                &snapshot,
                source_workstream_id,
                expected_workstream_revision,
                Some(provider),
            )?;
            let request_key = uuid::Uuid::new_v4().simple().to_string();
            let workstream_id = crate::actions::fork_workstream(
                root,
                &mut registry,
                source_workstream_id,
                Some(expected_workstream_revision),
                request_key,
            )
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(Some(workstream_id))
        }
        ManagedAction::RecoverOperation {
            operation_id,
            expected_operation_revision,
            provider,
        } => {
            snapshot
                .unresolved_operations
                .iter()
                .find(|operation| {
                    operation.operation_id == operation_id
                        && operation.revision == expected_operation_revision
                        && operation.provider == provider
                        && operation.kind == crate::domain::OperationKind::Fork
                })
                .ok_or(D17NavigatorError::ManagedActionUnavailable)?;
            let workstream_id = crate::actions::recover_managed_operation(
                root,
                &mut registry,
                operation_id,
                Some(expected_operation_revision),
            )
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(Some(workstream_id))
        }
        ManagedAction::Rename {
            workstream_id,
            expected_workstream_revision,
            name,
        } => {
            require_active_d17_workstream(
                &snapshot,
                workstream_id,
                expected_workstream_revision,
                Some(ProviderKind::Codex),
            )?;
            if name.trim().is_empty() {
                return Err(D17NavigatorError::ManagedActionUnavailable);
            }
            crate::actions::rename(
                &mut registry,
                workstream_id,
                expected_workstream_revision,
                &name,
            )
            .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
            Ok(None)
        }
    }
}

/// Closes only the terminal recovery journal for an exact Runtime that the
/// preceding Park action already stopped. This does not retry the original
/// provider launch or roll back its binding.
fn resolve_parked_onboarding_recovery(
    root: &StateRoot,
    workstream_id: WorkstreamId,
    expected_workstream_revision: Revision,
) -> Result<(), D17NavigatorError> {
    let mut state =
        open_d17_current_only(root).map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
        .map_err(|_| D17NavigatorError::ManagedActionUnavailable)?;
    state
        .resolve_d17_parked_recovery_current(
            &provisional_lease,
            workstream_id,
            expected_workstream_revision,
        )
        .map_err(|_| D17NavigatorError::ManagedActionUnavailable)
}

fn require_active_d17_workstream(
    snapshot: &D17Snapshot,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
    expected_provider: Option<ProviderKind>,
) -> Result<(), D17NavigatorError> {
    let workstream = snapshot
        .workstreams
        .iter()
        .find(|workstream| {
            workstream.workstream_id == workstream_id
                && !workstream.archived
                && workstream.revision == expected_revision
                && expected_provider.is_none_or(|provider| workstream.provider == provider)
        })
        .ok_or(D17NavigatorError::ManagedActionUnavailable)?;
    if workstream.onboarding.is_some() {
        return Err(D17NavigatorError::ManagedActionUnavailable);
    }
    Ok(())
}

/// The terminal onboarding-recovery state exposes only explicit Park. The
/// `ActionFenced` state permits no lifecycle mutation at all.
fn require_parkable_d17_workstream(
    snapshot: &D17Snapshot,
    workstream_id: WorkstreamId,
    expected_revision: Revision,
) -> Result<(), D17NavigatorError> {
    let workstream = snapshot
        .workstreams
        .iter()
        .find(|workstream| {
            workstream.workstream_id == workstream_id
                && !workstream.archived
                && workstream.revision == expected_revision
        })
        .ok_or(D17NavigatorError::ManagedActionUnavailable)?;
    if workstream.onboarding == Some(crate::d17_snapshot::D17OnboardingStatus::ActionFenced) {
        return Err(D17NavigatorError::ManagedActionUnavailable);
    }
    Ok(())
}

/// One exact post-start attachment claim for a session created from a selected
/// D17 Workstream. No project path, provider option, or shell cwd crosses this
/// boundary: the retained source Location and provider are the authority.
struct SameLocationAttachment {
    workstream_id: crate::domain::WorkstreamId,
    workstream_revision: crate::domain::Revision,
    runtime_id: RuntimeId,
    runtime_revision: crate::domain::Revision,
}

/// Creates an independent native session using only a selected unfenced source
/// Workstream's stored provider and Location, then returns the fresh passive
/// snapshot plus exact attachment revisions. The retired application and
/// Project-browser paths are intentionally never opened here.
fn start_d17_same_location(
    root: &StateRoot,
    source_workstream_id: crate::domain::WorkstreamId,
    expected_workstream_revision: crate::domain::Revision,
    provider: crate::domain::ProviderKind,
) -> Result<(D17Snapshot, SameLocationAttachment), D17NavigatorError> {
    let state = open_d17_current_only(root)
        .map_err(|_| D17NavigatorError::SameLocationSessionUnavailable)?;
    if state
        .d17_onboarding_workstream_projections()
        .map_err(|_| D17NavigatorError::SameLocationSessionUnavailable)?
        .iter()
        .any(|onboarding| onboarding.workstream_id == source_workstream_id)
    {
        return Err(D17NavigatorError::SameLocationSessionUnavailable);
    }
    let mut registry = state
        .into_d17_host_registry()
        .map_err(|_| D17NavigatorError::SameLocationSessionUnavailable)?;
    let source = registry
        .workstream_overviews()
        .map_err(|_| D17NavigatorError::SameLocationSessionUnavailable)?
        .into_iter()
        .find(|workstream| workstream.workstream_id == source_workstream_id)
        .ok_or(D17NavigatorError::SameLocationSessionUnavailable)?;
    if source.revision != expected_workstream_revision
        || source.provider != provider
        || source.archived_at_millis.is_some()
    {
        return Err(D17NavigatorError::SameLocationSessionUnavailable);
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
    .map_err(|_| D17NavigatorError::SameLocationSessionUnavailable)?;
    drop(registry);

    let snapshot = read_snapshot(root)?;
    let workstream = snapshot
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .ok_or(D17NavigatorError::SameLocationSessionUnavailable)?;
    let runtime = workstream
        .runtime
        .ok_or(D17NavigatorError::SameLocationSessionUnavailable)?;
    if workstream.provider != provider || workstream.onboarding.is_some() {
        return Err(D17NavigatorError::SameLocationSessionUnavailable);
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

/// Composes the D17 shell card with the marker-first materializer.
/// The retained provisional lease spans candidate allocation, account-shell
/// startup/evidence, and outer-pane replacement; no provider command is
/// constructed or launched here.
fn materialize_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<(), D17NavigatorError> {
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
) -> Result<PreHandoffRecovery, D17NavigatorError> {
    let unavailable = || D17NavigatorError::ProvisionalShellUnavailable;
    let mut state = open_d17_current_only(root).map_err(|_| unavailable())?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
        .map_err(|_| unavailable())?;
    reconcile_pre_handoff_under_lease(
        &mut state,
        &provisional_lease,
        &presentation.paths().directory,
    )
    .map_err(|_| unavailable())
}

/// Opens the initially selected provisional shell only after fresh D17
/// presentation startup has finished creating and proving both owned panes.
/// This uses the same marker/lease/materialization path as explicit Shell-card
/// activation and never creates provider or registry authority.
pub(crate) fn materialize_initial_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<(), D17NavigatorError> {
    materialize_provisional_shell(root, presentation)
}

/// Reattaches the one exact materialized shell after the provider pane has
/// switched to a managed Workstream. Marker absence is the only authority to
/// continue into fresh materialization; every other phase or malformed claim
/// remains a closed refusal and can never create a duplicate candidate.
fn reattach_materialized_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<bool, D17NavigatorError> {
    let unavailable = || D17NavigatorError::ProvisionalShellUnavailable;
    let mut state = open_d17_current_only(root).map_err(|_| unavailable())?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
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
        .attach_d17_provisional_shell(&state, &provisional_lease, &slot)
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

fn account_shell_inputs_from_environment() -> Result<AccountShellInputs, D17NavigatorError> {
    let unavailable = || D17NavigatorError::ProvisionalShellUnavailable;
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
) -> Result<(), D17NavigatorError> {
    let unavailable = || D17NavigatorError::ProvisionalShellUnavailable;
    let context =
        Presentation::d17_context_from_directory(root.base(), &presentation.paths().directory)
            .map_err(|_| unavailable())?;
    let mut state = open_d17_current_only(root).map_err(|_| unavailable())?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
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
        .attach_d17_provisional_shell(&state, &provisional_lease, &materialized)
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
    use std::{
        fs::{self, OpenOptions},
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::Duration,
    };

    use uuid::Uuid;

    use super::{
        AccountShellInputs, ManagedAction, ProviderExecRefresh, apply_managed_action,
        describe_shell_location, materialize_provisional_shell_with_inputs, observe_shell_cwd,
        reattach_materialized_provisional_shell, refresh_provider_exec,
        require_active_d17_workstream, require_parkable_d17_workstream, start_d17_same_location,
    };
    use crate::{
        d17_snapshot::{
            D17OnboardingStatus, D17ProjectSnapshot, D17Snapshot, D17WorkstreamSnapshot,
        },
        domain::{
            LocationId, ProjectId, ProviderKind, ProviderSessionId, RandomIdGenerator, Revision,
            WorkstreamId, WorkstreamLifecycle,
        },
        navigator::d17::D17ShellLocation,
        presentation::Presentation,
        process::output_bounded,
        provisional::{ProvisionalPhase, read_marker},
        state::{
            StateRoot, TRANSITION_LOCK_FILE, acquire_transition_lease, fresh_create,
            open_cutover_transition, open_d17_current_only,
        },
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

    fn migrate_to_schema14(state_path: &Path) {
        drop(fresh_create(state_path, &RandomIdGenerator).unwrap());
        migrate_existing_to_schema14(state_path);
    }

    fn migrate_existing_to_schema14(state_path: &Path) {
        let root = StateRoot::select(state_path);
        let transition_lock = state_path.join(TRANSITION_LOCK_FILE);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&transition_lock)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&transition_lock, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let transition = acquire_transition_lease(state_path).unwrap();
        let mut state = open_cutover_transition(&root, &transition).unwrap();
        state.migrate_schema13_to14(&transition).unwrap();
        drop(state);
        drop(transition);
        fs::remove_file(transition_lock).unwrap();
    }

    fn managed_snapshot(onboarding: Option<D17OnboardingStatus>) -> (D17Snapshot, WorkstreamId) {
        let project_id = ProjectId::from(Uuid::from_u128(801));
        let location_id = LocationId::from(Uuid::from_u128(802));
        let workstream_id = WorkstreamId::from(Uuid::from_u128(803));
        (
            D17Snapshot {
                projects: vec![D17ProjectSnapshot {
                    project_id,
                    display_name: "checkout".to_owned(),
                    locations: vec![],
                }],
                workstreams: vec![D17WorkstreamSnapshot {
                    project_id,
                    location_id,
                    workstream_id,
                    provider: ProviderKind::Codex,
                    lifecycle: WorkstreamLifecycle::Open,
                    archived: false,
                    revision: Revision::INITIAL,
                    runtime: None,
                    onboarding,
                    native_name: None,
                    attention_revision: Revision::INITIAL,
                    result_unseen: false,
                    recovery_unseen: false,
                }],
                unresolved_operations: vec![],
            },
            workstream_id,
        )
    }

    #[test]
    fn schema14_action_preflight_keeps_onboarding_fences_out_of_lifecycle_actions() {
        let (fenced, workstream_id) = managed_snapshot(Some(D17OnboardingStatus::ActionFenced));
        assert!(
            require_active_d17_workstream(
                &fenced,
                workstream_id,
                Revision::INITIAL,
                Some(ProviderKind::Codex),
            )
            .is_err()
        );
        assert!(
            require_parkable_d17_workstream(&fenced, workstream_id, Revision::INITIAL).is_err()
        );

        let (recovery, workstream_id) =
            managed_snapshot(Some(D17OnboardingStatus::RecoveryRequired));
        assert!(
            require_active_d17_workstream(
                &recovery,
                workstream_id,
                Revision::INITIAL,
                Some(ProviderKind::Codex),
            )
            .is_err()
        );
        assert!(
            require_parkable_d17_workstream(&recovery, workstream_id, Revision::INITIAL).is_ok()
        );

        let (ordinary, workstream_id) = managed_snapshot(None);
        assert!(
            require_active_d17_workstream(
                &ordinary,
                workstream_id,
                Revision::INITIAL,
                Some(ProviderKind::Codex),
            )
            .is_ok()
        );
    }

    #[test]
    fn schema14_restore_and_acknowledge_use_only_exact_durable_revisions() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let mut state = fresh_create(&state_path, &RandomIdGenerator).unwrap();
        let registered = state
            .register_project_location_with_initial_workstream(
                &checkout,
                "checkout",
                None,
                None,
                ProviderKind::OpenCode,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);
        migrate_existing_to_schema14(&state_path);

        let root = StateRoot::select(&state_path);
        let state = open_d17_current_only(&root).unwrap();
        let mut registry = state.into_d17_host_registry().unwrap();
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == registered.workstream.workstream_id)
            .unwrap();
        let archived_revision = registry
            .archive_workstream(overview.workstream_id, overview.revision, 1)
            .unwrap();
        drop(registry);

        assert_eq!(
            apply_managed_action(
                &root,
                ManagedAction::Restore {
                    workstream_id: registered.workstream.workstream_id,
                    expected_workstream_revision: archived_revision,
                },
            )
            .unwrap(),
            None
        );
        let restored = crate::d17_snapshot::read_snapshot(&root).unwrap();
        assert!(!restored.workstreams[0].archived);

        let state = open_d17_current_only(&root).unwrap();
        let mut registry = state.into_d17_host_registry().unwrap();
        let attention = registry
            .mark_result_attention(
                registered.workstream.workstream_id,
                ProviderSessionId::new(ProviderKind::OpenCode, "session-a").unwrap(),
                "turn-a".to_owned(),
            )
            .unwrap();
        drop(registry);
        assert_eq!(
            apply_managed_action(
                &root,
                ManagedAction::AcknowledgeResult {
                    workstream_id: registered.workstream.workstream_id,
                    expected_attention_revision: attention.revision,
                },
            )
            .unwrap(),
            None
        );
        let acknowledged = crate::d17_snapshot::read_snapshot(&root).unwrap();
        assert!(!acknowledged.workstreams[0].result_unseen);
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
    fn materialized_d17_shell_stays_unregistered_and_attaches_only_its_private_runtime() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        let home = temporary.path().join("home");
        fs::create_dir(&seed).unwrap();
        fs::create_dir(&home).unwrap();
        migrate_to_schema14(&state_path);

        let navigator = temporary.path().join("navigator-fixture");
        make_executable(&navigator, "#!/bin/sh\nexec sleep 60\n");
        let presentation = Presentation::fresh_with_executable(&state_path, navigator);
        presentation.start_d17(Uuid::from_u128(91), &seed).unwrap();
        let _presentation_guard = DisposableTmuxServerGuard(presentation.paths().socket.clone());

        let shell = [PathBuf::from("/usr/bin/bash"), PathBuf::from("/bin/bash")]
            .into_iter()
            .find(|candidate| candidate.is_file())
            .expect("a supported Bash account shell is required for D17 acceptance");
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

        let state = open_d17_current_only(&root).unwrap();
        assert!(state.d17_registered_runtime_paths().unwrap().is_empty());
        drop(state);

        presentation.close_d17().unwrap();
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
            D17ShellLocation::cwd("~/c/nested")
        );
        assert_eq!(
            describe_shell_location(&notes, Some(&home)),
            D17ShellLocation::cwd("~/notes")
        );
    }

    #[test]
    fn idle_d17_presentation_does_not_open_a_provider_reconciler() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let seed = temporary.path().join("seed");
        fs::create_dir(&seed).unwrap();
        migrate_to_schema14(&state_path);

        let navigator = temporary.path().join("navigator-fixture");
        make_executable(&navigator, "#!/bin/sh\nexec sleep 60\n");
        let presentation = Presentation::fresh_with_executable(&state_path, navigator);
        presentation.start_d17(Uuid::from_u128(92), &seed).unwrap();
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
        let mut state = fresh_create(&state_path, &RandomIdGenerator).unwrap();
        let source = state
            .register_project_location_with_initial_workstream(
                &checkout,
                "checkout",
                None,
                None,
                ProviderKind::Codex,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);
        migrate_existing_to_schema14(&state_path);

        let root = StateRoot::select(&state_path);
        let state = open_d17_current_only(&root).unwrap();
        let mut registry = state.into_d17_host_registry().unwrap();
        let source_overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|workstream| workstream.workstream_id == source.workstream.workstream_id)
            .unwrap();
        let archived_revision = registry
            .archive_workstream(source.workstream.workstream_id, source_overview.revision, 1)
            .unwrap();
        drop(registry);

        assert!(
            start_d17_same_location(
                &root,
                source.workstream.workstream_id,
                archived_revision,
                ProviderKind::Codex,
            )
            .is_err()
        );
        assert_eq!(
            crate::d17_snapshot::read_snapshot(&root)
                .unwrap()
                .workstreams
                .len(),
            1
        );
    }
}
