use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use wsnav::presentation::{Presentation, PresentationPaths};

const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
const FALSE_PROGRAM: &str = "/usr/bin/false";
const SLEEP_PROGRAM: &str = "/usr/bin/sleep";
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);
const READINESS_POLL: Duration = Duration::from_millis(10);

struct PrivateTmuxGuard {
    directory: PathBuf,
    socket: PathBuf,
}

impl Drop for PrivateTmuxGuard {
    fn drop(&mut self) {
        let mut command = tmux_command(&self.socket);
        let _ = command.args(["kill-server"]).output();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn open_or_create_retires_a_live_session_with_a_dead_navigator_pane() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    assert!(Path::new(FALSE_PROGRAM).is_file());
    assert!(Path::new(SLEEP_PROGRAM).is_file());

    let state_root = tempfile::tempdir().unwrap();
    let paths = PresentationPaths::fresh(state_root.path());
    let presentation_root = paths.directory.parent().unwrap();
    fs::create_dir_all(presentation_root).unwrap();
    fs::create_dir(&paths.directory).unwrap();
    fs::write(
        &paths.config,
        "set -g remain-on-exit on\nset -g status off\n",
    )
    .unwrap();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };

    let mut new_session = tmux_command(&paths.socket);
    let status = new_session
        .arg("-f")
        .arg(&paths.config)
        .args([
            "new-session",
            "-d",
            "-s",
            &paths.session_name,
            "-n",
            "navigator",
            FALSE_PROGRAM,
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let mut split = tmux_command(&paths.socket);
    let status = split
        .args([
            "split-window",
            "-h",
            "-d",
            "-t",
            &format!("{}:{NAVIGATOR_PANE}", paths.session_name),
            SLEEP_PROGRAM,
            "60",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    wait_for_fixture(&paths);
    assert_eq!(
        pane_dead(&paths.socket, &paths.session_name, NAVIGATOR_PANE),
        Some(true)
    );
    assert_eq!(
        pane_dead(&paths.socket, &paths.session_name, PROVIDER_PANE),
        Some(false)
    );

    let failed_directory = paths.directory.clone();
    let (fresh, created) = Presentation::open_or_create(state_root.path()).unwrap();

    assert!(created);
    assert_ne!(fresh.paths().directory, failed_directory);
    assert!(!failed_directory.exists());
    assert!(!session_exists(&paths.socket, &paths.session_name));
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn tmux_command(socket: &Path) -> Command {
    let mut command = Command::new("tmux");
    command.env_remove("TMUX").arg("-S").arg(socket);
    command
}

fn wait_for_fixture(paths: &PresentationPaths) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        if pane_dead(&paths.socket, &paths.session_name, NAVIGATOR_PANE) == Some(true)
            && pane_dead(&paths.socket, &paths.session_name, PROVIDER_PANE) == Some(false)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "private tmux fixture did not become ready"
        );
        thread::sleep(READINESS_POLL);
    }
}

fn pane_dead(socket: &Path, session: &str, target: &str) -> Option<bool> {
    let mut command = tmux_command(socket);
    let output = command
        .args(["display-message", "-p", "-t"])
        .arg(format!("{session}:{target}"))
        .arg("#{pane_dead}")
        .output()
        .ok()?;
    parse_pane_dead(&output)
}

fn parse_pane_dead(output: &Output) -> Option<bool> {
    if !output.status.success() || output.stdout.len() > 16 {
        return None;
    }
    match output.stdout.as_slice() {
        b"0\n" | b"0\r\n" => Some(false),
        b"1\n" | b"1\r\n" => Some(true),
        _ => None,
    }
}

fn session_exists(socket: &Path, session: &str) -> bool {
    let mut command = tmux_command(socket);
    command
        .args(["has-session", "-t"])
        .arg(session)
        .output()
        .is_ok_and(|output| output.status.success())
}
