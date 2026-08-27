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
    d17_snapshot::{D17SnapshotError, read_snapshot},
    presentation::{Presentation, PresentationError},
    state::StateRoot,
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
}

/// Runs the hidden schema-14 D17 Navigator pane. It validates the exact D17
/// presentation context before reading state; unimplemented effect commands
/// intentionally remain inert until their complete controller is present.
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
            if navigator.handle_key(key.code) == D17Command::Quit {
                break true;
            }
            redraw = true;
        }
        if last_refresh.elapsed() >= Duration::from_millis(500) {
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
