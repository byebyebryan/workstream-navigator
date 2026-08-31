use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Write,
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
        state::create_current(root.path(), &RandomIdGenerator).expect("fresh schema-15 state root"),
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
        .start(uuid::Uuid::from_u128(0x1701), state_root.path())
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

    presentation.close().unwrap();
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
        .start(uuid::Uuid::from_u128(0x1702), state_root.path())
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
        .start(uuid::Uuid::from_u128(0x1703), state_root.path())
        .unwrap();

    let panes = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            "#{pane_id}|#{@wsnav_role}",
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
        BTreeSet::from(["?", "d", "Up", "Down", "Left", "Right", "C-b",])
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
#[cfg(unix)]
fn primary_mouse_press_validates_focus_and_forwards_native_event() {
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
        .start(uuid::Uuid::from_u128(0x1710), state_root.path())
        .unwrap();

    let mut client = attach_tmux_client(&paths);
    let _initial_client_name = wait_for_presentation_client(&paths);
    let provider = pane_id_for_role(&paths, "provider");
    let navigator = pane_id_for_role(&paths, "navigator");

    // Establish an invalid topology before any mouse input reaches the
    // provider PTY. The synchronous predicate must refuse without either
    // selecting or forwarding the press.
    select_pane(&paths.socket, &navigator);
    let extra = tmux_command(&paths.socket)
        .args([
            "split-window",
            "-v",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &provider,
            SLEEP_PROGRAM,
            "60",
        ])
        .output()
        .unwrap();
    assert!(extra.status.success(), "tmux failed: {:?}", extra.stderr);
    let invalid_ready = state_root.path().join("mouse-invalid-ready");
    let invalid_capture = state_root.path().join("mouse-invalid-capture");
    respawn_mouse_fixture(&paths.socket, &provider, &invalid_ready, &invalid_capture);
    wait_for_path(&invalid_ready);
    thread::sleep(Duration::from_millis(100));
    let (invalid_left, invalid_top, _, _) = pane_geometry(&paths.socket, &provider);
    let invalid_x = invalid_left.saturating_add(4);
    let invalid_y = invalid_top.saturating_add(3);
    assert_eq!(active_pane(&paths.socket), navigator);
    select_pane(&paths.socket, &navigator);
    send_sgr_mouse_press(&mut client, invalid_x, invalid_y);
    thread::sleep(Duration::from_millis(150));
    assert_eq!(active_pane(&paths.socket), navigator);
    assert_eq!(
        fs::metadata(&invalid_capture).map_or(0, |metadata| metadata.len()),
        0
    );
    send_sgr_mouse_release(&mut client, invalid_x, invalid_y);
    thread::sleep(Duration::from_millis(50));

    let extra = String::from_utf8(extra.stdout).unwrap().trim().to_owned();
    let killed = tmux_command(&paths.socket)
        .args(["kill-pane", "-t", &extra])
        .output()
        .unwrap();
    assert!(killed.status.success(), "tmux failed: {:?}", killed.stderr);
    let _ = client.kill();
    let _ = client.wait();
    let mut client = attach_tmux_client(&paths);
    let client_name = wait_for_presentation_client(&paths);

    // With the exact topology restored, a fresh provider PTY receives the
    // translated SGR press after tmux moves focus to the clicked pane.
    let ready = state_root.path().join("mouse-ready");
    let capture = state_root.path().join("mouse-capture");
    respawn_mouse_fixture(&paths.socket, &provider, &ready, &capture);
    wait_for_path(&ready);
    let (left, top, width, height) = pane_geometry(&paths.socket, &provider);
    assert!(width > 8 && height > 6);
    let x = left.saturating_add(4);
    let y = top.saturating_add(3);
    let expected_press = format!(
        "\x1b[<0;{};{}M",
        x.saturating_sub(left).saturating_add(1),
        y.saturating_sub(top).saturating_add(1)
    );
    select_pane(&paths.socket, &navigator);
    assert_eq!(active_pane(&paths.socket), navigator);
    send_sgr_mouse_press(&mut client, x, y);
    wait_for_active_pane(&paths.socket, &provider);
    wait_for_file_len(&capture, expected_press.len() as u64);
    assert_eq!(fs::read(&capture).unwrap(), expected_press.as_bytes());

    // Release and wheel events over an inactive pane never select it.
    select_pane(&paths.socket, &navigator);
    send_sgr_mouse_release(&mut client, x, y);
    send_sgr_mouse_wheel_up(&mut client, x, y);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(active_pane(&paths.socket), navigator);

    // Keep the exact client lookup live in the test so a stale/foreign client
    // cannot accidentally make the valid path pass.
    assert!(!client_name.is_empty());
    let _ = client.kill();
    let _ = client.wait();
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
        .start(uuid::Uuid::from_u128(0x1704), state_root.path())
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
    assert!(
        presentation
            .control(PresentationAction::FocusLeft, "%0")
            .is_err()
    );
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
    let script = "#!/bin/sh\nif [ \"$3\" = \"_navigator\" ] || [ \"$3\" = \"_provider_wait\" ]; then exec sleep 60; fi\n";
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
        .start(uuid::Uuid::from_u128(0x1705), state_root.path())
        .unwrap();

    let prefix = tmux_output(&paths.socket, ["list-keys", "-T", "prefix"]);
    for action in [
        "switch-previous",
        "switch-next",
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
        .start(uuid::Uuid::from_u128(0x1707), state_root.path())
        .unwrap();
    let provider = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            "#{pane_id}|#{@wsnav_role}",
        ],
    )
    .lines()
    .find_map(|line| {
        let (pane, role) = line.split_once('|')?;
        (role == "provider").then_some(pane.to_owned())
    })
    .expect(" provider pane");

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

fn wait_for_presentation_client(paths: &PresentationPaths) -> String {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline {
        let output = tmux_command(&paths.socket)
            .args([
                "list-clients",
                "-F",
                "#{client_name}|#{session_name}|#{window_name}",
            ])
            .output()
            .unwrap();
        if output.status.success()
            && let Some(client) =
                String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        let fields = line.split('|').collect::<Vec<_>>();
                        (fields.len() == 3
                            && fields[1] == paths.session_name
                            && fields[2] == "navigator")
                            .then(|| fields[0].to_owned())
                    })
        {
            return client;
        }
        thread::sleep(READINESS_POLL);
    }
    panic!("private presentation client was not discoverable");
}

fn pane_id_for_role(paths: &PresentationPaths, role: &str) -> String {
    let output = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            "#{pane_id}|#{@wsnav_role}",
        ],
    );
    output
        .lines()
        .find_map(|line| {
            let (pane, pane_role) = line.split_once('|')?;
            (pane_role == role).then(|| pane.to_owned())
        })
        .unwrap_or_else(|| panic!("missing {role} pane"))
}

fn select_pane(socket: &Path, pane: &str) {
    let output = tmux_command(socket)
        .args(["select-pane", "-t", pane])
        .output()
        .unwrap();
    assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
}

fn active_pane(socket: &Path) -> String {
    tmux_output(socket, ["display-message", "-p", "#{pane_id}"])
        .trim()
        .to_owned()
}

fn wait_for_active_pane(socket: &Path, expected: &str) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline {
        if active_pane(socket) == expected {
            return;
        }
        thread::sleep(READINESS_POLL);
    }
    panic!(
        "expected active pane {expected}, got {}",
        active_pane(socket)
    );
}

fn pane_geometry(socket: &Path, pane: &str) -> (u16, u16, u16, u16) {
    let output = tmux_output(
        socket,
        [
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}",
        ],
    );
    let fields = output
        .trim()
        .split('|')
        .map(|field| field.parse::<u16>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    (fields[0], fields[1], fields[2], fields[3])
}

fn respawn_mouse_fixture(socket: &Path, pane: &str, ready: &Path, capture: &Path) {
    let script = format!(
        "printf '\\033[?1000h\\033[?1006h'; stty -icanon min 1 time 0; touch {}; dd if=/dev/stdin of={} bs=1 count=9 status=none; sleep 60",
        shell_quote_for_test(ready),
        shell_quote_for_test(capture),
    );
    let output = tmux_command(socket)
        .args(["respawn-pane", "-k", "-t", pane, "/bin/sh", "-c"])
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(READINESS_POLL);
    }
    assert!(
        path.exists(),
        "fixture did not become ready: {}",
        path.display()
    );
}

fn wait_for_file_len(path: &Path, expected: u64) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while fs::metadata(path).map_or(0, |metadata| metadata.len()) < expected
        && Instant::now() < deadline
    {
        thread::sleep(READINESS_POLL);
    }
    assert_eq!(fs::metadata(path).unwrap().len(), expected);
}

fn send_sgr_mouse_press(client: &mut Child, x: u16, y: u16) {
    send_sgr_mouse(client, b'M', x, y);
}

fn send_sgr_mouse_release(client: &mut Child, x: u16, y: u16) {
    send_sgr_mouse(client, b'm', x, y);
}

fn send_sgr_mouse_wheel_up(client: &mut Child, x: u16, y: u16) {
    let stdin = client.stdin.as_mut().expect("attached client stdin");
    write!(
        stdin,
        "\x1b[<64;{};{}M",
        x.saturating_add(1),
        y.saturating_add(1)
    )
    .unwrap();
    stdin.flush().unwrap();
}

fn send_sgr_mouse(client: &mut Child, suffix: u8, x: u16, y: u16) {
    let stdin = client.stdin.as_mut().expect("attached client stdin");
    write!(
        stdin,
        "\x1b[<0;{};{}{}",
        x.saturating_add(1),
        y.saturating_add(1),
        char::from(suffix)
    )
    .unwrap();
    stdin.flush().unwrap();
}

fn tmux_has_client(socket: &Path, session: &str) -> bool {
    let output = tmux_command(socket)
        .args([
            "list-clients",
            "-F",
            "#{client_name}|#{session_name}|#{window_name}",
        ])
        .output();
    output.is_ok_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.split('|').nth(1) == Some(session))
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
            "#{client_name}|#{client_key_table}|#{client_prefix}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().find_map(|line| {
        let fields = line.split('|').collect::<Vec<_>>();
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
            "#{pane_id}|#{@wsnav_role}|#{@wsnav_workstream_id}|#{pane_top}",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}
