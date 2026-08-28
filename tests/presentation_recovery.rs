use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use wsnav::{
    domain::{RandomIdGenerator, RuntimeId},
    presentation::{Presentation, PresentationAction, PresentationPaths},
    runtime::{NativeLaunch, PrivateRuntime, RuntimePaths, SystemTmux},
    state,
};

const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
const SLEEP_PROGRAM: &str = "/usr/bin/sleep";
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);
const READINESS_POLL: Duration = Duration::from_millis(10);
const DEFAULT_NAVIGATOR_PANE_WIDTH: u16 = 32;

fn current_state_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary state root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
            .expect("private state root");
    }
    drop(
        state::fresh_create_d17(root.path(), &RandomIdGenerator)
            .expect("fresh schema-14 state root"),
    );
    root
}

struct PrivateTmuxGuard {
    directory: PathBuf,
    socket: PathBuf,
}

impl Drop for PrivateTmuxGuard {
    fn drop(&mut self) {
        let _ = tmux_command(&self.socket).args(["kill-server"]).output();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn navigator_stop_leaves_cleanup_to_the_outer_owner() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }

    let state_root = current_state_root();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation
        .start_d17(uuid::Uuid::from_u128(0x1701), state_root.path())
        .unwrap();

    let mut client = attach_tmux_client(&paths);
    assert!(
        tmux_command(&paths.socket)
            .args(["send-keys", "-t"])
            .arg(format!("{}:{NAVIGATOR_PANE}", paths.session_name))
            .arg("q")
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline
        && (client.try_wait().unwrap().is_none()
            || pane_dead(&paths.socket, &paths.session_name, NAVIGATOR_PANE) != Some(true))
    {
        thread::sleep(READINESS_POLL);
    }
    let navigator_target = format!("{}:{NAVIGATOR_PANE}", paths.session_name);
    let navigator_output = tmux_output(
        &paths.socket,
        ["capture-pane", "-p", "-J", "-t", &navigator_target],
    );
    assert!(
        client.try_wait().unwrap().is_some(),
        "navigator pane: {navigator_output}"
    );
    assert_eq!(
        pane_dead(&paths.socket, &paths.session_name, NAVIGATOR_PANE),
        Some(true)
    );
    assert!(session_exists(&paths.socket, &paths.session_name));
    assert!(paths.directory.exists());

    presentation.close_d17().unwrap();
    assert!(!paths.directory.exists());
}

#[test]
fn private_presentation_restores_navigator_width_when_the_window_resizes() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }

    let state_root = current_state_root();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    let ordinary_root = tempfile::tempdir().unwrap();
    let ordinary_socket = ordinary_root.path().join("ordinary.sock");
    let _ordinary_guard = PrivateTmuxGuard {
        directory: ordinary_root.path().to_path_buf(),
        socket: ordinary_socket.clone(),
    };
    let status = tmux_command(&ordinary_socket)
        .args(["new-session", "-d", "-s", "ordinary", SLEEP_PROGRAM, "60"])
        .status()
        .unwrap();
    assert!(status.success());
    let status = tmux_command(&ordinary_socket)
        .args(["bind-key", "-T", "root", "z", "display-message", "ordinary"])
        .status()
        .unwrap();
    assert!(status.success());
    let ordinary_before = tmux_output(&ordinary_socket, ["list-keys", "-T", "root"]);
    presentation
        .start_d17(uuid::Uuid::from_u128(0x1702), state_root.path())
        .unwrap();
    let ordinary_after = tmux_output(&ordinary_socket, ["list-keys", "-T", "root"]);
    assert_eq!(ordinary_before, ordinary_after);

    let status = tmux_command(&paths.socket)
        .args(["resize-window", "-t"])
        .arg(format!("{}:0", paths.session_name))
        .args(["-x", "150", "-y", "40"])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        pane_width(&paths.socket, &paths.session_name, NAVIGATOR_PANE),
        Some(DEFAULT_NAVIGATOR_PANE_WIDTH)
    );
    assert!(
        pane_width(&paths.socket, &paths.session_name, PROVIDER_PANE)
            .is_some_and(|width| width > DEFAULT_NAVIGATOR_PANE_WIDTH)
    );
}

#[test]
fn private_presentation_has_only_owned_roles_and_bounded_key_tables() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = current_state_root();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation
        .start_d17(uuid::Uuid::from_u128(0x1703), state_root.path())
        .unwrap();

    let panes = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            "#{pane_id}\t#{@wsnav_role}",
        ],
    );
    assert!(panes.contains("navigator"));
    assert!(panes.contains("provider"));
    assert!(!panes.contains("utility"));

    let prefix = tmux_output(&paths.socket, ["list-keys", "-T", "prefix"]);
    for unsafe_binding in [
        "split-window",
        "swap-pane",
        "kill-pane",
        "respawn-pane",
        "display-menu",
        "command-prompt",
    ] {
        assert!(
            !prefix.contains(unsafe_binding),
            "unexpected binding: {unsafe_binding}"
        );
    }
    assert!(prefix.contains("run-shell"));
    assert!(!prefix.contains("run-shell -b"));
    assert_eq!(
        binding_keys(&prefix),
        BTreeSet::from(["?", "d", "o", "Up", "Down", "Left", "Right", "C-b",])
    );
    let root = tmux_output(&paths.socket, ["list-keys", "-T", "root"]);
    assert_eq!(
        binding_keys(&root),
        BTreeSet::from([
            "MouseDown1Pane",
            "MouseUp1Pane",
            "MouseDrag1Pane",
            "WheelUpPane",
            "WheelDownPane",
        ])
    );
}

#[test]
fn mutated_private_geometry_fails_closed_without_rearranging_panes() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = current_state_root();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation
        .start_d17(uuid::Uuid::from_u128(0x1704), state_root.path())
        .unwrap();
    let output = tmux_command(&paths.socket)
        .args(["split-window", "-v", "-d", "-P", "-F", "#{pane_id}", "-t"])
        .arg("%0")
        .args([SLEEP_PROGRAM, "60"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let extra = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let status = tmux_command(&paths.socket)
        .args(["set-option", "-p", "-t", &extra, "@wsnav_role", "external"])
        .status()
        .unwrap();
    assert!(status.success());
    let before = pane_snapshot(&paths);
    assert_eq!(before.len(), 3);
    assert!(presentation.focus_navigator().is_err());
    assert_eq!(pane_snapshot(&paths), before);
}

#[test]
fn tmux_control_bindings_are_parseable_active_helpers_only() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let fixture_root = tempfile::tempdir().unwrap();
    let executable = fixture_root.path().join("wsnav-fixture");
    let script = "#!/bin/sh\nif [ \"$3\" = \"_navigator_d17\" ] || [ \"$3\" = \"_provider_wait\" ]; then exec sleep 60; fi\n";
    fs::write(&executable, script).unwrap();
    make_executable(&executable);
    let state_root = current_state_root();
    let presentation = Presentation::fresh_with_executable(state_root.path(), executable);
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation
        .start_d17(uuid::Uuid::from_u128(0x1705), state_root.path())
        .unwrap();

    let prefix = tmux_output(&paths.socket, ["list-keys", "-T", "prefix"]);
    for action in [
        "focus-next",
        "focus-up",
        "focus-down",
        "focus-left",
        "focus-right",
        "literal-c-b",
    ] {
        assert!(prefix.contains(&format!("--action {action}")));
    }
    for retired in ["create-or-focus-shell", "suppress-split", "close-shell"] {
        assert!(!prefix.contains(retired));
    }
}

#[test]
fn outer_presentation_protects_nested_provider_literal_prefix() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = current_state_root();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation
        .start_d17(uuid::Uuid::from_u128(0x1707), state_root.path())
        .unwrap();
    let provider = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            "#{pane_id}\t#{@wsnav_role}",
        ],
    )
    .lines()
    .find_map(|line| {
        let (pane, role) = line.split_once('\t')?;
        (role == "provider").then_some(pane.to_owned())
    })
    .expect("D17 provider pane");

    assert!(matches!(
        presentation.control(PresentationAction::LiteralCtrlB, &provider),
        Err(wsnav::presentation::PresentationError::ControlRefused(message))
            if message.contains("Runtime preflight")
    ));
}

#[test]
fn nested_runtime_literal_ctrl_b_reaches_the_provider_as_one_byte() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = tempfile::tempdir().unwrap();
    let capture = state_root.path().join("literal-byte");
    let ready = state_root.path().join("literal-ready");
    let marker = state_root.path().join("runtime-prefix-marker");
    let script = format!(
        "stty -icanon min 1 time 0; touch {}; dd if=/dev/stdin of={} bs=1 count=1 status=none; sleep 60",
        shell_quote_for_test(&ready),
        shell_quote_for_test(&capture),
    );
    let tmux = SystemTmux::default();
    let process_probe = wsnav::runtime::LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(state_root.path(), RuntimeId::new()),
    );
    runtime
        .start(&NativeLaunch {
            cwd: state_root.path().to_path_buf(),
            program: vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(script),
            ],
            environment: std::collections::BTreeMap::new(),
        })
        .unwrap();
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while !ready.exists() && Instant::now() < deadline {
        thread::sleep(READINESS_POLL);
    }
    assert!(
        ready.exists(),
        "provider input fixture did not become ready"
    );
    let marker_command = format!("touch {}", shell_quote_for_test(&marker));
    let status = tmux_command(&runtime.paths().socket)
        .args(["bind-key", "-T", "prefix", "C-b", "run-shell"])
        .arg(marker_command)
        .status()
        .unwrap();
    assert!(status.success());

    let outer_root = tempfile::tempdir().unwrap();
    let outer_socket = outer_root.path().join("outer.sock");
    let outer_session = "outer";
    let _outer_guard = PrivateTmuxGuard {
        directory: outer_root.path().to_path_buf(),
        socket: outer_socket.clone(),
    };
    let status = tmux_command(&outer_socket)
        .args([
            "new-session",
            "-d",
            "-s",
            outer_session,
            SLEEP_PROGRAM,
            "60",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let nested_attach = format!(
        "unset TMUX; exec tmux -u -S {} attach-session -t {}",
        shell_quote_for_test(&runtime.paths().socket),
        shell_quote_for_test(Path::new(&runtime.paths().session_name)),
    );
    let status = tmux_command(&outer_socket)
        .args(["split-window", "-d", "-t", "outer:0.0"])
        .arg(nested_attach)
        .status()
        .unwrap();
    assert!(status.success());
    let mut outer_client = attach_tmux_session_client(&outer_socket, outer_session);

    let runtime_client = wait_for_runtime_client(&runtime.paths().socket);
    runtime.send_literal_ctrl_b().unwrap();

    let deadline = Instant::now() + READINESS_TIMEOUT;
    while fs::metadata(&capture).map_or(0, |metadata| metadata.len()) < 1
        && Instant::now() < deadline
    {
        thread::sleep(READINESS_POLL);
    }
    assert_eq!(fs::read(&capture).unwrap(), [2]);
    let runtime_client_after = wait_for_runtime_client(&runtime.paths().socket);
    assert_eq!(runtime_client_after.0, runtime_client.0);
    assert_eq!(runtime_client_after.1, "root");
    assert_eq!(runtime_client_after.2, "0");
    assert!(!marker.exists());
    runtime.park().unwrap();
    let _ = outer_client.kill();
    let _ = outer_client.wait();
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

fn shell_quote_for_test(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn attach_tmux_client(paths: &PresentationPaths) -> Child {
    attach_tmux_session_client(&paths.socket, &paths.session_name)
}

fn attach_tmux_session_client(socket: &Path, session: &str) -> Child {
    let command = format!(
        "env -u TMUX tmux -S {} attach-session -t {}",
        shell_quote_for_test(socket),
        shell_quote_for_test(Path::new(session)),
    );
    let child = Command::new("script")
        .env("TERM", "xterm-256color")
        .args(["-qefc", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("script is required for disposable tmux client");
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline {
        if tmux_has_client(socket, session) {
            return child;
        }
        thread::sleep(READINESS_POLL);
    }
    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
    panic!("private presentation client did not attach");
}

fn tmux_has_client(socket: &Path, session: &str) -> bool {
    let output = tmux_command(socket)
        .args([
            "list-clients",
            "-F",
            "#{client_name}\t#{session_name}\t#{window_name}",
        ])
        .output();
    output.is_ok_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.split('\t').nth(1) == Some(session))
    })
}

fn wait_for_runtime_client(socket: &Path) -> (String, String, String) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(client) = runtime_client_status(socket) {
            return client;
        }
        thread::sleep(READINESS_POLL);
    }
    panic!("nested runtime client did not attach");
}

fn runtime_client_status(socket: &Path) -> Option<(String, String, String)> {
    let output = tmux_command(socket)
        .args([
            "list-clients",
            "-F",
            "#{client_name}\t#{client_key_table}\t#{client_prefix}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().find_map(|line| {
        let fields = line.split('\t').collect::<Vec<_>>();
        (fields.len() == 3).then(|| {
            (
                fields[0].to_owned(),
                fields[1].to_owned(),
                fields[2].to_owned(),
            )
        })
    })
}

fn binding_keys(output: &str) -> BTreeSet<&str> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let table_index = fields.iter().position(|field| *field == "-T")?;
            let key = fields.get(table_index + 2)?;
            Some(key.strip_prefix('\\').unwrap_or(key))
        })
        .collect()
}

fn pane_dead(socket: &Path, session: &str, target: &str) -> Option<bool> {
    let output = tmux_command(socket)
        .args(["display-message", "-p", "-t"])
        .arg(format!("{session}:{target}"))
        .arg("#{pane_dead}")
        .output()
        .ok()?;
    parse_pane_dead(&output)
}

fn pane_width(socket: &Path, session: &str, target: &str) -> Option<u16> {
    let output = tmux_command(socket)
        .args(["display-message", "-p", "-t"])
        .arg(format!("{session}:{target}"))
        .arg("#{pane_width}")
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 16 {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
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
    tmux_command(socket)
        .args(["has-session", "-t"])
        .arg(session)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn tmux_output<const N: usize>(socket: &Path, arguments: [&str; N]) -> String {
    let output = tmux_command(socket).args(arguments).output().unwrap();
    assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
    String::from_utf8(output.stdout).unwrap()
}

fn pane_snapshot(paths: &PresentationPaths) -> Vec<String> {
    tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            "#{pane_id}\t#{@wsnav_role}\t#{@wsnav_workstream_id}\t#{pane_top}",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}
