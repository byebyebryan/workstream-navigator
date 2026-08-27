//! Dormant D17 schema-14 Navigator pane.
//!
//! This controller deliberately owns only terminal setup and passive D17
//! snapshots while the shell-card materialization and provider-attachment
//! effects are still being completed. It is reachable solely from the hidden
//! D17 presentation pane command, never from the ordinary D16 Navigator.

#![allow(
    dead_code,
    reason = "the D17 Navigator pane remains unreachable until the atomic cutover"
)]

use std::{
    env,
    io::{self, Stdout},
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use thiserror::Error;

use crate::{
    d17_account_shell::{AccountShellContext, AccountShellLaunch},
    d17_shell_control::reconcile_provider_exec_from_presentation,
    d17_snapshot::{D17Snapshot, D17SnapshotError, read_snapshot},
    domain::RuntimeId,
    presentation::{Presentation, PresentationError},
    provisional::{ProvisionalPhase, ProvisionalSlot, SlotError, SlotGeneration, read_marker},
    runtime::{LinuxProcessProbe, PrivateRuntime, SystemTmux},
    state::{StateRoot, open_d17_current_only},
};

use super::d17::{D17Command, D17Navigator};

/// Errors that prevent the dormant D17 pane from rendering its passive,
/// schema-14-only Workstreams view.
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
    #[error("D17 provider reconciliation is unavailable")]
    ProviderReconciliationUnavailable,
    #[error("D17 same-location session creation is unavailable")]
    SameLocationSessionUnavailable,
}

/// Runs the hidden schema-14 D17 Navigator pane. It validates the exact D17
/// presentation context before reading state. The provisional-shell command is
/// a lease-held marker-first materialization followed by an outer-pane attach;
/// all other D17 actions remain inert until their complete controllers exist.
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
    let _context =
        Presentation::d17_context_from_directory(root.base(), &presentation.paths().directory)
            .map_err(|_| PresentationError::D17ContextUnavailable)?;
    let snapshot = read_snapshot(root)?;
    let mut navigator = D17Navigator::new(snapshot);
    let mut terminal = TerminalSession::enter().map_err(D17NavigatorError::Terminal)?;
    let mut redraw = true;
    let mut last_refresh = Instant::now();

    let quit = loop {
        if redraw {
            terminal
                .terminal
                .draw(|frame| navigator.render(frame, frame.area()))
                .map_err(D17NavigatorError::Terminal)?;
            redraw = false;
        }
        if event::poll(Duration::from_millis(100)).map_err(D17NavigatorError::Terminal)?
            && let Event::Key(key) = event::read().map_err(D17NavigatorError::Terminal)?
        {
            match navigator.handle_key(key.code) {
                D17Command::Quit => break true,
                D17Command::MaterializeProvisionalShell => {
                    if materialize_provisional_shell(root, &presentation).is_ok() {
                        if presentation.focus_provider().is_err() {
                            navigator
                                .set_guidance("Shell opened; provider-pane focus is unavailable");
                        }
                    } else {
                        navigator
                            .set_guidance("New session shell unavailable; exact state required");
                    }
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
                        if presentation.focus_provider().is_err() {
                            navigator.set_guidance(
                                "Managed session opened; provider-pane focus is unavailable",
                            );
                        }
                    } else {
                        navigator.set_guidance(
                            "Managed session is unavailable; exact Runtime evidence required",
                        );
                    }
                }
                D17Command::NewAtSameLocation {
                    source_workstream_id,
                    expected_workstream_revision,
                    provider,
                } => match start_d17_same_location(
                    root,
                    source_workstream_id,
                    expected_workstream_revision,
                    provider,
                ) {
                    Ok((snapshot, attachment)) => {
                        navigator.replace_snapshot(snapshot);
                        if presentation
                            .attach_d17_workstream(
                                attachment.workstream_id,
                                attachment.workstream_revision,
                                attachment.runtime_id,
                                attachment.runtime_revision,
                            )
                            .is_ok()
                        {
                            if presentation.focus_provider().is_err() {
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
                },
                D17Command::None => {}
            }
            redraw = true;
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
            if reconcile_provider_exec_if_ready(root, &presentation).is_err() {
                navigator.set_guidance(
                    "Managed session reconciliation is unavailable; exact recovery required",
                );
            }
            if let Ok(snapshot) = read_snapshot(root) {
                navigator.replace_snapshot(snapshot);
            }
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
/// snapshot plus exact attachment revisions. The normal D16 application and
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

/// Calls the post-exec controller only for the one marker phase in which the
/// helper already transferred Runtime ownership and committed its final exec
/// fence. A missing marker is the normal idle-card state; all other valid
/// provisional phases remain owned by the account shell or completed journal.
///
/// The reconciliation adapter never creates a provider process. Its `OpenCode`
/// branch may start only the already-authorized detached observer after exact
/// native-exec proof, and it cannot activate attachment until that observer is
/// both ready and currently live.
fn reconcile_provider_exec_if_ready(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<bool, D17NavigatorError> {
    let slot = match read_marker(root.base(), &presentation.paths().directory) {
        Ok(slot) => slot,
        Err(SlotError::MarkerUnavailable) => return Ok(false),
        Err(_) => return Err(D17NavigatorError::ProviderReconciliationUnavailable),
    };
    if slot.phase() != ProvisionalPhase::RuntimeOwnedLaunching {
        return Ok(false);
    }
    reconcile_provider_exec_from_presentation(root.base(), &presentation.paths().directory)
        .map_err(|_| D17NavigatorError::ProviderReconciliationUnavailable)?;
    Ok(true)
}

/// Composes the dormant D17 shell card with the marker-first materializer.
/// The retained provisional lease spans candidate allocation, account-shell
/// startup/evidence, and outer-pane replacement; no provider command is
/// constructed or launched here.
fn materialize_provisional_shell(
    root: &StateRoot,
    presentation: &Presentation,
) -> Result<(), D17NavigatorError> {
    materialize_provisional_shell_with_inputs(
        root,
        presentation,
        &account_shell_inputs_from_environment()?,
    )
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
        AccountShellInputs, materialize_provisional_shell_with_inputs,
        reconcile_provider_exec_if_ready, start_d17_same_location,
    };
    use crate::{
        domain::{ProviderKind, RandomIdGenerator},
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
        assert!(!reconcile_provider_exec_if_ready(&root, &presentation).unwrap());

        let marker = read_marker(root.base(), &presentation.paths().directory).unwrap();
        assert_eq!(marker.phase(), ProvisionalPhase::Materialized);
        let _runtime_guard = DisposableTmuxServerGuard(marker.runtime_paths().socket.clone());
        wait_for_private_client(&marker.runtime_paths().socket);

        assert!(materialize_provisional_shell_with_inputs(&root, &presentation, &inputs).is_err());
        assert_eq!(
            read_marker(root.base(), &presentation.paths().directory).unwrap(),
            marker
        );

        let state = open_d17_current_only(&root).unwrap();
        assert!(state.d17_registered_runtime_paths().unwrap().is_empty());
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

        assert!(
            !reconcile_provider_exec_if_ready(&StateRoot::select(&state_path), &presentation)
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
