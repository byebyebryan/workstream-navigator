use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use wsnav::{
    domain::{RandomIdGenerator, WorkstreamId},
    presentation::{AttachmentPhase, Presentation, PresentationAction, PresentationPaths},
    runtime::{LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, SystemTmux},
    state,
};

const NAVIGATOR_PANE: &str = "0.0";
const PROVIDER_PANE: &str = "0.1";
const FALSE_PROGRAM: &str = "/usr/bin/false";
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
    drop(state::fresh_create(root.path(), &RandomIdGenerator).expect("fresh schema-13 state root"));
    root
}

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
fn open_or_create_refuses_an_unmarked_path_shaped_presentation() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    assert!(Path::new(FALSE_PROGRAM).is_file());
    assert!(Path::new(SLEEP_PROGRAM).is_file());

    let state_root = current_state_root();
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
    for (pane, role) in [(NAVIGATOR_PANE, "navigator"), (PROVIDER_PANE, "provider")] {
        let status = tmux_command(&paths.socket)
            .args(["set-option", "-p", "-t"])
            .arg(format!("{}:{pane}", paths.session_name))
            .args(["@wsnav_role", role])
            .status()
            .unwrap();
        assert!(status.success());
    }

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
    let result = Presentation::open_or_create(state_root.path());

    assert!(matches!(
        result,
        Err(wsnav::presentation::PresentationError::ControlRefused(message))
            if message.contains("ownership") || message.contains("foreign")
    ));
    assert!(failed_directory.exists());
    assert!(session_exists(&paths.socket, &paths.session_name));
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
    presentation.start().unwrap();

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
    presentation.start().unwrap();
    let ordinary_after = tmux_output(&ordinary_socket, ["list-keys", "-T", "root"]);
    assert_eq!(ordinary_before, ordinary_after);

    let mut resize = tmux_command(&paths.socket);
    let status = resize
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
    presentation.start().unwrap();

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
        BTreeSet::from([
            "\"", "%", "?", "d", "o", "x", "Up", "Down", "Left", "Right", "C-b",
        ])
    );
    let started = Instant::now();
    let status = tmux_command(&paths.socket)
        .args(["run-shell", "sleep 0.05"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(started.elapsed() >= Duration::from_millis(30));
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
    let normalized_root = root
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        normalized_root
            .contains("MouseDrag1Pane if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\"")
    );
    assert!(normalized_root.contains(
        "WheelUpPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\""
    ));
    assert!(normalized_root.contains(
        "WheelDownPane if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\""
    ));
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
    presentation.start().unwrap();
    let output = tmux_command(&paths.socket)
        .args(["split-window", "-v", "-d", "-P", "-F", "#{pane_id}", "-t"])
        .arg("%0")
        .args([SLEEP_PROGRAM, "60"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let utility = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    for (option, value) in [
        ("@wsnav_role", "utility".to_owned()),
        ("@wsnav_workstream_id", WorkstreamId::new().to_string()),
    ] {
        let status = tmux_command(&paths.socket)
            .args(["set-option", "-p", "-t", &utility, option, &value])
            .status()
            .unwrap();
        assert!(status.success());
    }
    let before = pane_snapshot(&paths);
    assert_eq!(before.len(), 3);
    assert!(presentation.focus_navigator().is_err());
    assert_eq!(pane_snapshot(&paths), before);
}

#[test]
fn local_shell_create_is_idempotent_and_failed_launch_restores_two_panes() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = current_state_root();
    let project_root = tempfile::tempdir().unwrap();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();
    let workstream_id = WorkstreamId::new();
    exercise_local_shell(&presentation, &paths, workstream_id, project_root.path());
    presentation.close().unwrap();

    let failed_root = current_state_root();
    let failed = Presentation::fresh_with_executable(
        failed_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let failed_paths = failed.paths().clone();
    let _failed_guard = PrivateTmuxGuard {
        directory: failed_paths.directory.clone(),
        socket: failed_paths.socket.clone(),
    };
    failed.start().unwrap();
    exercise_failed_shell(&failed, &failed_paths, workstream_id, project_root.path());
}

#[test]
fn guarded_close_targets_the_attached_client_and_only_utility_pane() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = current_state_root();
    let project_root = tempfile::tempdir().unwrap();
    let presentation = Presentation::fresh_with_executable(
        state_root.path(),
        PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
    );
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();
    let workstream_id = WorkstreamId::new();
    respawn_fixture_panes(&paths);
    set_provider_context(&paths, workstream_id);
    presentation
        .create_or_focus_shell(
            "%0",
            workstream_id,
            project_root.path(),
            Path::new("/bin/sh"),
        )
        .unwrap();
    let utility = pane_value(&paths, "utility", "#{pane_id}");

    let mut client = attach_tmux_client(&paths);
    let client_name = attached_client_name(&paths).expect("attached presentation client");
    let navigator = pane_value(&paths, "navigator", "#{pane_id}");
    presentation
        .control_with_client(
            PresentationAction::CloseShell,
            &navigator,
            Some(&client_name),
        )
        .unwrap();
    assert_eq!(pane_snapshot(&paths).len(), 3);

    let close_presentation = presentation.clone();
    let close_client = client_name.clone();
    let close_utility = utility.clone();
    let closer = thread::spawn(move || {
        close_presentation.control_with_client(
            PresentationAction::CloseShell,
            &close_utility,
            Some(&close_client),
        )
    });
    thread::sleep(Duration::from_millis(100));
    client
        .stdin
        .as_mut()
        .expect("tmux attach stdin")
        .write_all(b"y\n")
        .unwrap();
    closer.join().unwrap().unwrap();
    wait_for_pane_count(&paths, 2);
    assert_eq!(pane_value(&paths, "navigator", "#{pane_id}"), navigator);
    assert!(pane_value_if_present(&paths, "utility", "#{pane_id}").is_none());
    assert!(pane_value_if_present(&paths, "provider", "#{pane_id}").is_some());
    let _ = client.kill();
    let _ = client.wait();
}

#[test]
fn tmux_control_binding_expands_only_intentional_formats() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let fixture_root = tempfile::tempdir().unwrap();
    let marker = fixture_root.path().join("m");
    let recorder = fixture_root.path().join("argv-record");
    let malicious_root = fixture_root
        .path()
        .join(format!("s'/#{{danger}}/#(touch {})", marker.display()));
    fs::create_dir_all(&malicious_root).unwrap();
    let executable = malicious_root.join(format!("x'/#{{danger}}/#(touch {})", marker.display()));
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    let recorder_path = shell_quote_for_test(&recorder);
    let script = format!(
        "#!/bin/sh\nif [ \"$3\" = \"_navigator\" ] || [ \"$3\" = \"_provider_wait\" ]; then exec sleep 60; fi\n: > {recorder_path}\nfor arg in \"$@\"; do printf '%s\\0' \"$arg\" >> {recorder_path}; done\n",
    );
    fs::write(&executable, script).unwrap();
    make_executable(&executable);
    let presentation = Presentation::fresh_with_executable(&malicious_root, executable.clone());
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();
    let mut client = attach_tmux_client(&paths);
    let client_name = attached_client_name(&paths).expect("attached presentation client");
    let binding = tmux_output(&paths.socket, ["list-keys", "-T", "prefix"]);
    let command = binding
        .lines()
        .find(|line| line.contains("--action suppress-split"))
        .and_then(extract_run_shell_command)
        .expect("fixed suppress-split binding");
    let output = tmux_command(&paths.socket)
        .args(["run-shell", "-t", NAVIGATOR_PANE])
        .arg(&command)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tmux failed: {:?} stdout={:?} command={command}",
        output.stderr,
        output.stdout
    );
    let bytes = fs::read(&recorder).unwrap();
    let arguments = bytes
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| String::from_utf8(argument.to_vec()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(arguments[0], "--state-root");
    assert_eq!(arguments[1], malicious_root.display().to_string());
    assert_eq!(arguments[2], "_presentation_control");
    assert_eq!(arguments[3], "--presentation-socket");
    assert_eq!(arguments[4], paths.socket.display().to_string());
    assert_eq!(arguments[5], "--presentation-session");
    assert_eq!(arguments[6], paths.session_name);
    assert_eq!(
        &arguments[7..11],
        &[
            "--action".to_owned(),
            "suppress-split".to_owned(),
            "--source-pane".to_owned(),
            "%0".to_owned(),
        ]
    );
    assert_eq!(
        &arguments[11..],
        &["--client-name".to_owned(), client_name.clone()]
    );
    assert!(!marker.exists());
    let _ = fs::remove_file(&marker);
    let _ = client.kill();
    let _ = client.wait();
}

fn exercise_local_shell(
    presentation: &Presentation,
    paths: &PresentationPaths,
    workstream_id: WorkstreamId,
    project_root: &Path,
) {
    respawn_fixture_panes(paths);
    set_provider_context(paths, workstream_id);
    let concurrent = Arc::new(presentation.clone());
    let gate = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let presentation = Arc::clone(&concurrent);
            let gate = Arc::clone(&gate);
            let cwd = project_root.to_path_buf();
            thread::spawn(move || {
                gate.wait();
                presentation.create_or_focus_shell("%0", workstream_id, &cwd, Path::new("/bin/sh"))
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    wait_for_pane_count(paths, 3);
    let first = pane_snapshot(paths);
    assert_eq!(first.len(), 3);
    assert!(first.iter().any(|line| line.contains("\tutility\t")));
    let utility_top = pane_field(paths, "utility", "#{pane_top}");
    let provider_top = pane_field(paths, "provider", "#{pane_top}");
    assert!(utility_top > provider_top);
    wait_for_pane_value(
        paths,
        "utility",
        "#{pane_current_path}",
        &project_root.display().to_string(),
    );
    assert_eq!(pane_option(paths, "utility", "remain-on-exit"), "off");

    let owned_panes: Vec<String> = ["navigator", "provider", "utility"]
        .into_iter()
        .map(|role| pane_value(paths, role, "#{pane_id}"))
        .collect();
    for source in &owned_panes {
        presentation
            .control(PresentationAction::FocusNext, source)
            .unwrap();
        assert!(owned_panes.contains(&active_pane(paths)));
    }
    presentation
        .create_or_focus_shell("%1", workstream_id, project_root, Path::new("/bin/sh"))
        .unwrap();
    assert_eq!(pane_snapshot(paths).len(), 3);
    for source in ["navigator", "provider"] {
        presentation
            .control(
                PresentationAction::CloseShell,
                &pane_value(paths, source, "#{pane_id}"),
            )
            .unwrap();
    }
    assert_eq!(pane_snapshot(paths).len(), 3);
    let utility = pane_value(paths, "utility", "#{pane_id}");
    let status = tmux_command(&paths.socket)
        .args(["send-keys", "-t", &utility, "C-d"])
        .status()
        .unwrap();
    assert!(status.success());
    wait_for_pane_count(paths, 2);
    assert_eq!(pane_option(paths, "navigator", "remain-on-exit"), "on");
    assert_eq!(pane_option(paths, "provider", "remain-on-exit"), "on");
}

fn exercise_failed_shell(
    presentation: &Presentation,
    paths: &PresentationPaths,
    workstream_id: WorkstreamId,
    project_root: &Path,
) {
    respawn_fixture_panes(paths);
    set_provider_context(paths, workstream_id);
    assert!(
        presentation
            .create_or_focus_shell("%999", workstream_id, project_root, Path::new("/bin/sh"),)
            .is_err()
    );
    assert_eq!(pane_snapshot(paths).len(), 2);
    assert!(
        presentation
            .create_or_focus_shell(
                "%0",
                workstream_id,
                project_root,
                Path::new("/bin/sh invalid"),
            )
            .is_err()
    );
    assert_eq!(pane_snapshot(paths).len(), 2);
    presentation
        .create_or_focus_shell("%0", workstream_id, project_root, Path::new(FALSE_PROGRAM))
        .unwrap();
    wait_for_pane_count(paths, 2);
    assert_eq!(pane_snapshot(paths).len(), 2);
}

#[test]
fn local_attachment_switch_closes_only_a_different_workstream_shell() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    let fake = state_root.path().join("provider-helper");
    fs::write(&fake, "#!/bin/sh\nexec /usr/bin/sleep 60\n").unwrap();
    make_executable(&fake);
    let presentation = Presentation::fresh_with_executable(state_root.path(), fake);
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();

    let first_workstream = WorkstreamId::new();
    presentation.attach_workstream(first_workstream).unwrap();
    presentation
        .create_or_focus_shell(
            "%1",
            first_workstream,
            project_root.path(),
            Path::new("/bin/sh"),
        )
        .unwrap();
    wait_for_pane_count(&paths, 3);
    let utility = pane_value(&paths, "utility", "#{pane_id}");
    let utility_pid = pane_value(&paths, "utility", "#{pane_pid}");

    // Reconnecting the same exact attachment leaves its utility pane and
    // process untouched.
    presentation.attach_workstream(first_workstream).unwrap();
    assert_eq!(pane_value(&paths, "utility", "#{pane_id}"), utility);
    assert_eq!(pane_value(&paths, "utility", "#{pane_pid}"), utility_pid);
    assert_eq!(pane_snapshot(&paths).len(), 3);

    let second_workstream = WorkstreamId::new();
    presentation.attach_workstream(second_workstream).unwrap();
    wait_for_pane_count(&paths, 2);
    wait_for_pid_exit(&utility_pid);
    assert!(pane_value_if_present(&paths, "utility", "#{pane_id}").is_none());
    assert_eq!(
        pane_value(&paths, "provider", "#{@wsnav_workstream_id}"),
        second_workstream.to_string()
    );
    assert_eq!(pane_field(&paths, "navigator", "#{pane_top}"), 0);
    assert_eq!(pane_field(&paths, "provider", "#{pane_top}"), 0);
}

#[test]
fn completed_attachment_pane_can_be_replaced_by_another_workstream() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = tempfile::tempdir().unwrap();
    let fake = state_root.path().join("provider-helper");
    fs::write(&fake, "#!/bin/sh\nexec /usr/bin/sleep 60\n").unwrap();
    make_executable(&fake);
    let presentation = Presentation::fresh_with_executable(state_root.path(), fake);
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();

    let first_workstream = WorkstreamId::new();
    presentation.attach_workstream(first_workstream).unwrap();
    let provider = pane_value(&paths, "provider", "#{pane_id}");
    let status = tmux_command(&paths.socket)
        .args(["respawn-pane", "-k", "-t", &provider, FALSE_PROGRAM])
        .status()
        .unwrap();
    assert!(status.success());
    wait_for_pane_state(&paths, PROVIDER_PANE, true);
    assert_eq!(
        presentation.attachment_status().unwrap().unwrap().phase,
        AttachmentPhase::Failed
    );

    let second_workstream = WorkstreamId::new();
    presentation.attach_workstream(second_workstream).unwrap();
    wait_for_pane_state(&paths, PROVIDER_PANE, false);
    assert_eq!(pane_value(&paths, "provider", "#{pane_id}"), provider);
    assert_eq!(
        pane_value(&paths, "provider", "#{@wsnav_workstream_id}"),
        second_workstream.to_string()
    );
}

#[test]
fn ambiguous_attachment_topology_refuses_switch_before_provider_mutation() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    let fake = state_root.path().join("provider-helper");
    fs::write(&fake, "#!/bin/sh\nexec /usr/bin/sleep 60\n").unwrap();
    make_executable(&fake);
    let presentation = Presentation::fresh_with_executable(state_root.path(), fake);
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();

    let first_workstream = WorkstreamId::new();
    presentation.attach_workstream(first_workstream).unwrap();
    presentation
        .create_or_focus_shell(
            "%1",
            first_workstream,
            project_root.path(),
            Path::new("/bin/sh"),
        )
        .unwrap();
    wait_for_pane_count(&paths, 3);
    let provider = pane_value(&paths, "provider", "#{pane_id}");
    let utility = pane_value(&paths, "utility", "#{pane_id}");
    let before = pane_snapshot(&paths);

    // An untagged extra pane makes the owned topology ambiguous. The target
    // provider must retain its exact context and the utility must remain.
    let output = tmux_command(&paths.socket)
        .args([
            "split-window",
            "-v",
            "-d",
            "-t",
            &provider,
            SLEEP_PROGRAM,
            "60",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let second_workstream = WorkstreamId::new();
    assert!(presentation.attach_workstream(second_workstream).is_err());
    assert_eq!(
        presentation.attachment_status().unwrap().unwrap().phase,
        AttachmentPhase::Failed
    );
    assert_eq!(pane_value(&paths, "provider", "#{pane_id}"), provider);
    assert_eq!(
        pane_value(&paths, "provider", "#{@wsnav_workstream_id}"),
        first_workstream.to_string()
    );
    assert_eq!(pane_value(&paths, "utility", "#{pane_id}"), utility);
    assert_ne!(pane_snapshot(&paths), before);
}

#[test]
fn extra_presentation_window_refuses_cross_workstream_attachment_without_mutation() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    let fake = state_root.path().join("provider-helper");
    fs::write(&fake, "#!/bin/sh\nexec /usr/bin/sleep 60\n").unwrap();
    make_executable(&fake);
    let presentation = Presentation::fresh_with_executable(state_root.path(), fake);
    let paths = presentation.paths().clone();
    let _guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();

    let first_workstream = WorkstreamId::new();
    presentation.attach_workstream(first_workstream).unwrap();
    presentation
        .create_or_focus_shell(
            "%1",
            first_workstream,
            project_root.path(),
            Path::new("/bin/sh"),
        )
        .unwrap();
    wait_for_pane_count(&paths, 3);
    let provider = pane_value(&paths, "provider", "#{pane_id}");
    let utility = pane_value(&paths, "utility", "#{pane_id}");
    let utility_pid = pane_value(&paths, "utility", "#{pane_pid}");

    let output = tmux_command(&paths.socket)
        .args([
            "new-window",
            "-d",
            "-n",
            "hidden",
            "-t",
            &paths.session_name,
            SLEEP_PROGRAM,
            "60",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
    assert_eq!(
        tmux_output(
            &paths.socket,
            [
                "list-windows",
                "-t",
                &paths.session_name,
                "-F",
                "#{window_name}"
            ]
        )
        .lines()
        .count(),
        2
    );

    let second_workstream = WorkstreamId::new();
    assert!(presentation.attach_workstream(second_workstream).is_err());
    assert_eq!(
        presentation.attachment_status().unwrap().unwrap().phase,
        AttachmentPhase::Failed
    );
    assert_eq!(pane_value(&paths, "provider", "#{pane_id}"), provider);
    assert_eq!(
        pane_value(&paths, "provider", "#{@wsnav_workstream_id}"),
        first_workstream.to_string()
    );
    assert_eq!(pane_value(&paths, "utility", "#{pane_id}"), utility);
    assert_eq!(pane_value(&paths, "utility", "#{pane_pid}"), utility_pid);
    assert_eq!(pane_snapshot(&paths).len(), 3);
}

#[test]
fn nested_runtime_literal_ctrl_b_reaches_the_provider_as_one_byte() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let state_root = tempfile::tempdir().unwrap();
    let capture = state_root.path().join("literal-byte");
    let marker = state_root.path().join("runtime-prefix-marker");
    let script = format!(
        "stty -icanon min 1 time 0; dd if=/dev/stdin of={} bs=1 count=1 status=none; sleep 60",
        shell_quote_for_test(&capture)
    );
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(state_root.path(), wsnav::domain::RuntimeId::new()),
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

    thread::sleep(Duration::from_millis(50));
    let runtime_client = wait_for_runtime_client(&runtime.paths().socket);
    runtime.send_literal_ctrl_b().unwrap();

    let deadline = Instant::now() + READINESS_TIMEOUT;
    while !capture.exists() && Instant::now() < deadline {
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

fn attached_client_name(paths: &PresentationPaths) -> Option<String> {
    attached_client_name_for(&paths.socket, &paths.session_name, Some("navigator"))
}

fn attached_client_name_for(
    socket: &Path,
    expected_session: &str,
    expected_window: Option<&str>,
) -> Option<String> {
    let output = tmux_command(socket)
        .args([
            "list-clients",
            "-F",
            "#{client_name}\t#{session_name}\t#{window_name}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| {
            let mut fields = line.split('\t');
            let client = fields.next()?;
            let session = fields.next()?;
            let window = fields.next()?;
            (session == expected_session
                && expected_window.is_none_or(|expected| window == expected)
                && fields.next().is_none())
            .then(|| client.to_owned())
        })
}

fn tmux_has_client(socket: &Path, session: &str) -> bool {
    attached_client_name_for(socket, session, None).is_some()
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

fn extract_run_shell_command(line: &str) -> Option<String> {
    let command = line.split_once(" run-shell ")?.1.trim();
    let command = command.strip_prefix('"')?.strip_suffix('"')?;
    Some(command.replace("\\\\", "\\").replace("\\\"", "\""))
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

fn wait_for_pane_value(paths: &PresentationPaths, role: &str, format: &str, expected: &str) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        if pane_value(paths, role, format) == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "private tmux pane value did not become ready"
        );
        thread::sleep(READINESS_POLL);
    }
}

fn wait_for_pane_state(paths: &PresentationPaths, pane: &str, expected_dead: bool) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        if pane_dead(&paths.socket, &paths.session_name, pane) == Some(expected_dead) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "private tmux pane {pane} did not reach dead={expected_dead}"
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

fn pane_width(socket: &Path, session: &str, target: &str) -> Option<u16> {
    let mut command = tmux_command(socket);
    let output = command
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
    let mut command = tmux_command(socket);
    command
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

fn set_provider_context(paths: &PresentationPaths, workstream_id: WorkstreamId) {
    let target = format!("{}:{PROVIDER_PANE}", paths.session_name);
    let value = workstream_id.to_string();
    let status = tmux_command(&paths.socket)
        .args([
            "set-option",
            "-p",
            "-t",
            &target,
            "@wsnav_workstream_id",
            &value,
        ])
        .status()
        .unwrap();
    assert!(status.success());
}

fn respawn_fixture_panes(paths: &PresentationPaths) {
    for target in [NAVIGATOR_PANE, PROVIDER_PANE] {
        let status = tmux_command(&paths.socket)
            .args(["respawn-pane", "-k", "-t"])
            .arg(format!("{}:{target}", paths.session_name))
            .args([SLEEP_PROGRAM, "60"])
            .status()
            .unwrap();
        assert!(status.success());
    }
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

fn pane_field(paths: &PresentationPaths, role: &str, format: &str) -> u16 {
    let output = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            &format!("#{{@wsnav_role}}\t{format}"),
        ],
    );
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role}\t")))
        .unwrap()
        .parse()
        .unwrap()
}

fn pane_value(paths: &PresentationPaths, role: &str, format: &str) -> String {
    let output = tmux_output(
        &paths.socket,
        [
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            &format!("#{{@wsnav_role}}\t{format}"),
        ],
    );
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role}\t")))
        .unwrap()
        .to_owned()
}

fn pane_value_if_present(paths: &PresentationPaths, role: &str, format: &str) -> Option<String> {
    let output = tmux_command(&paths.socket)
        .args([
            "list-panes",
            "-t",
            &format!("{}:navigator", paths.session_name),
            "-F",
            &format!("#{{@wsnav_role}}\t{format}"),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role}\t")).map(str::to_owned))
}

fn active_pane(paths: &PresentationPaths) -> String {
    let output = tmux_command(&paths.socket)
        .args(["display-message", "-p", "-t"])
        .arg(format!("{}:navigator", paths.session_name))
        .arg("#{pane_id}")
        .output()
        .unwrap();
    assert!(output.status.success(), "tmux failed: {:?}", output.stderr);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn pane_option(paths: &PresentationPaths, role: &str, option: &str) -> String {
    let pane = pane_value(paths, role, "#{pane_id}");
    tmux_output(
        &paths.socket,
        ["show-options", "-p", "-v", "-t", &pane, option],
    )
    .trim()
    .to_owned()
}

fn wait_for_pane_count(paths: &PresentationPaths, expected: usize) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline {
        if pane_snapshot(paths).len() == expected {
            return;
        }
        thread::sleep(READINESS_POLL);
    }
    panic!(
        "expected {expected} private presentation panes, got {:?}",
        pane_snapshot(paths)
    );
}

fn wait_for_pid_exit(pid: &str) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while Instant::now() < deadline {
        let exited = Command::new("kill")
            .args(["-0", pid])
            .status()
            .is_ok_and(|status| !status.success());
        if exited {
            return;
        }
        thread::sleep(READINESS_POLL);
    }
    panic!("utility process {pid} did not exit");
}
