//! Disposable D15 warm local switching study.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use wsnav::{
    domain::{ProviderKind, RuntimeId, RuntimeStatus, WorkstreamId},
    presentation::{AttachmentPhase, Presentation, PresentationPaths},
    process::output_bounded,
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{HostRegistry, RuntimeRecord, StateRoot},
};

const SAMPLE_PAIRS: usize = 20;
const READINESS_TIMEOUT: Duration = Duration::from_secs(3);
const READINESS_POLL: Duration = Duration::from_millis(5);
const MAX_METADATA_BYTES: usize = 16 * 1024;

const NATIVE_FIXTURE: &str = "#!/bin/sh\nset -eu\n\nmarker=$1\nnative_session=$2\nevent_log=$3\nprogress=$4\nprintf '%s\\n' \"$$\" > \"$marker.pid\"\nprintf '%s\\n' \"$native_session\" > \"$marker.session\"\nprintf '%s\\n' \"provider-start\" >> \"$event_log\"\ncounter=0\nwhile :; do\n    counter=$((counter + 1))\n    printf '%s\\n' \"$counter\" > \"$progress.tmp\"\n    mv \"$progress.tmp\" \"$progress\"\n    sleep 0.01\ndone\n";

struct PrivateTmuxGuard {
    directory: PathBuf,
    socket: PathBuf,
}

impl Drop for PrivateTmuxGuard {
    fn drop(&mut self) {
        let _ = tmux_command(&self.socket)
            .arg("kill-server")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeIdentity {
    workstream_id: WorkstreamId,
    runtime_id: RuntimeId,
    tmux_generation: String,
    tmux_socket: PathBuf,
    tmux_session: String,
    provider_pid: u32,
    process_birth: String,
    native_session: String,
}

struct RuntimeFixture<'a> {
    record: RuntimeRecord,
    runtime: PrivateRuntime<'a>,
    identity_pid: PathBuf,
    identity_session: PathBuf,
    progress_marker: PathBuf,
    native_session: String,
}

struct FixtureLaunch<'a> {
    executable: &'a Path,
    event_log: &'a Path,
    native_label: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeClient {
    name: String,
    session: String,
    tty: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct PhaseSample {
    navigator_focus: f64,
    outer_replacement: f64,
    provider_focus: f64,
    helper_start_observation: f64,
    runtime_client_attachment: f64,
    activation_to_attached: f64,
}

#[test]
#[ignore = "controlled D15 warm switching study; not a shared-CI wall-clock test"]
#[allow(clippy::similar_names, clippy::too_many_lines)]
fn d15_warm_local_switching_study() {
    if !tmux_available() {
        eprintln!("skipped: tmux is unavailable");
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let state_root = StateRoot::create(workspace.path().join("state")).unwrap();
    let study_root = state_root.base().join("d15");
    fs::create_dir_all(&study_root).unwrap();
    let native_executable = workspace.path().join("d15-native-fixture.sh");
    fs::write(&native_executable, NATIVE_FIXTURE).unwrap();
    make_executable(&native_executable);
    let event_log = study_root.join("events.log");

    let project_a = workspace.path().join("project-a");
    let project_b = workspace.path().join("project-b");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();
    let records = reserve_fixture_records(&state_root, &project_a, &project_b);

    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let fixture_a = start_fixture(
        &tmux,
        &process_probe,
        &state_root,
        &study_root,
        records[0].clone(),
        &FixtureLaunch {
            executable: &native_executable,
            event_log: &event_log,
            native_label: "native-a",
        },
    );
    let fixture_b = start_fixture(
        &tmux,
        &process_probe,
        &state_root,
        &study_root,
        records[1].clone(),
        &FixtureLaunch {
            executable: &native_executable,
            event_log: &event_log,
            native_label: "native-b",
        },
    );
    let _runtime_a_guard = PrivateTmuxGuard {
        directory: fixture_a.runtime.paths().directory.clone(),
        socket: fixture_a.runtime.paths().socket.clone(),
    };
    let _runtime_b_guard = PrivateTmuxGuard {
        directory: fixture_b.runtime.paths().directory.clone(),
        socket: fixture_b.runtime.paths().socket.clone(),
    };

    let identity_a = wait_for_identity(&fixture_a);
    let identity_b = wait_for_identity(&fixture_b);
    record_fixture_identities(&state_root, &identity_a, &identity_b);

    let attachment_executable = workspace.path().join("d15-attachment-wrapper.sh");
    write_attachment_wrapper(&attachment_executable, &state_root);
    let presentation =
        Presentation::fresh_with_executable(state_root.base(), attachment_executable);
    let paths = presentation.paths().clone();
    let _presentation_guard = PrivateTmuxGuard {
        directory: paths.directory.clone(),
        socket: paths.socket.clone(),
    };
    presentation.start().unwrap();
    let _outer_client = attach_tmux_client(&paths);

    // Warm the presentation and both provider attachments before sampling.
    presentation.focus_navigator().unwrap();
    let initial = presentation
        .attach_workstream(fixture_a.record.workstream_id)
        .unwrap();
    presentation.focus_provider().unwrap();
    wait_for_attachment_marker(
        &study_root,
        fixture_a.record.workstream_id,
        initial.attempt_id,
    );
    wait_for_helper_start(
        &presentation,
        initial.attempt_id,
        fixture_a.record.workstream_id,
    );
    wait_for_runtime_client(
        &fixture_a.runtime.paths().socket,
        &fixture_a.runtime.paths().session_name,
    );
    wait_until_no_runtime_clients(
        &fixture_b.runtime.paths().socket,
        &fixture_b.runtime.paths().session_name,
    );

    let mut samples = Vec::with_capacity(SAMPLE_PAIRS * 2);
    for _ in 0..SAMPLE_PAIRS {
        let sample_b = switch_once(&presentation, &fixture_b, &study_root);
        assert_invisible_progress(&fixture_a, &identity_a, &fixture_b);
        samples.push(sample_b);

        let sample_a = switch_once(&presentation, &fixture_a, &study_root);
        assert_invisible_progress(&fixture_b, &identity_b, &fixture_a);
        samples.push(sample_a);
    }

    // Re-probe both private servers after all switches. The exact process,
    // generation, and native-session identities must be unchanged; only the
    // outer helper moved.
    assert_eq!(identity_a, wait_for_identity(&fixture_a));
    assert_eq!(identity_b, wait_for_identity(&fixture_b));
    assert_registry_identities(&state_root, &fixture_a, &identity_a);
    assert_registry_identities(&state_root, &fixture_b, &identity_b);

    let events = fs::read_to_string(&event_log).expect("provider event log must be readable");
    assert_eq!(
        events
            .lines()
            .filter(|line| *line == "provider-start")
            .count(),
        2,
        "each fixture must have exactly one native provider start",
    );
    let attachment_helpers = events
        .lines()
        .filter(|line| *line == "attachment-helper")
        .count();
    assert_eq!(attachment_helpers, SAMPLE_PAIRS * 2 + 1);
    assert!(
        events
            .lines()
            .all(|line| matches!(line, "provider-start" | "attachment-helper"))
    );

    let label = std::env::var("D15_PHASE").unwrap_or_else(|_| "candidate".to_owned());
    println!(
        "d15-study label={label} samples={} navigator_focus_ms_p50={:.3} navigator_focus_ms_p95={:.3} outer_replacement_ms_p50={:.3} outer_replacement_ms_p95={:.3} provider_focus_ms_p50={:.3} provider_focus_ms_p95={:.3} helper_start_observation_ms_p50={:.3} helper_start_observation_ms_p95={:.3} runtime_client_attachment_ms_p50={:.3} runtime_client_attachment_ms_p95={:.3} activation_to_attached_ms_p50={:.3} activation_to_attached_ms_p95={:.3}",
        samples.len(),
        percentile(&samples, |sample| sample.navigator_focus, 50, 100),
        percentile(&samples, |sample| sample.navigator_focus, 95, 100),
        percentile(&samples, |sample| sample.outer_replacement, 50, 100),
        percentile(&samples, |sample| sample.outer_replacement, 95, 100),
        percentile(&samples, |sample| sample.provider_focus, 50, 100),
        percentile(&samples, |sample| sample.provider_focus, 95, 100),
        percentile(&samples, |sample| sample.helper_start_observation, 50, 100),
        percentile(&samples, |sample| sample.helper_start_observation, 95, 100),
        percentile(&samples, |sample| sample.runtime_client_attachment, 50, 100),
        percentile(&samples, |sample| sample.runtime_client_attachment, 95, 100),
        percentile(&samples, |sample| sample.activation_to_attached, 50, 100),
        percentile(&samples, |sample| sample.activation_to_attached, 95, 100),
    );
    println!(
        "d15-study identity_proof runtime_ids=true tmux_generations=true tmux_sessions=true provider_pids=true process_births=true native_sessions=true invisible_progress=true provider_starts=2 attachment_helper_calls={attachment_helpers} lifecycle_mutations=not_observed_by_fixture terminal_capture=false"
    );
}

fn reserve_fixture_records(
    state: &StateRoot,
    project_a: &Path,
    project_b: &Path,
) -> [RuntimeRecord; 2] {
    let mut registry = HostRegistry::open(state).unwrap();
    let workstream_a = registry
        .register_project_root(project_a, ProviderKind::Codex)
        .unwrap();
    let workstream_b = registry
        .register_project_root(project_b, ProviderKind::Codex)
        .unwrap();
    [
        registry
            .reserve_runtime_with_provider(workstream_a.workstream_id, ProviderKind::Codex)
            .unwrap(),
        registry
            .reserve_runtime_with_provider(workstream_b.workstream_id, ProviderKind::Codex)
            .unwrap(),
    ]
}

fn start_fixture<'a>(
    tmux: &'a SystemTmux,
    process_probe: &'a LinuxProcessProbe,
    state: &StateRoot,
    study_root: &Path,
    record: RuntimeRecord,
    launch_config: &FixtureLaunch<'_>,
) -> RuntimeFixture<'a> {
    let identity_marker = study_root.join(format!("{}.identity", record.workstream_id));
    let identity_pid = PathBuf::from(format!("{}.pid", identity_marker.display()));
    let identity_session = PathBuf::from(format!("{}.session", identity_marker.display()));
    let progress_marker = study_root.join(format!("{}.progress", record.workstream_id));
    let native_session = format!("{}-{}", launch_config.native_label, record.runtime_id);
    let paths =
        RuntimePaths::for_record(state.base(), record.runtime_id, &record.tmux_session).unwrap();
    let launch = NativeLaunch {
        cwd: record.cwd.clone(),
        program: vec![
            launch_config.executable.to_path_buf().into_os_string(),
            identity_marker.clone().into_os_string(),
            native_session.clone().into(),
            launch_config.event_log.to_path_buf().into_os_string(),
            progress_marker.clone().into_os_string(),
        ],
        environment: BTreeMap::new(),
    };
    let runtime = PrivateRuntime::new(tmux, process_probe, paths);
    runtime.start(&launch).unwrap();
    RuntimeFixture {
        record,
        runtime,
        identity_pid,
        identity_session,
        progress_marker,
        native_session,
    }
}

fn write_attachment_wrapper(path: &Path, state: &StateRoot) {
    let real_binary = shell_quote(Path::new(env!("CARGO_BIN_EXE_wsnav")));
    let state_root = shell_quote(state.base());
    let script = format!(
        "#!/bin/sh\nset -eu\n\nreal_binary={real_binary}\nstate_root={state_root}\nevent_log=\"$state_root/d15/events.log\"\nif [ \"${{3:-}}\" = \"_navigator\" ] || [ \"${{3:-}}\" = \"_provider_wait\" ]; then\n    exec sleep 600\nfi\nif [ \"${{3:-}}\" = \"_provider_attach\" ]; then\n    workstream_id=$4\n    attempt_id=${{10}}\n    printf '%s\\n' \"attachment-helper\" >> \"$event_log\"\n    marker=\"$state_root/d15/$workstream_id-$attempt_id.provider_exec\"\n    : > \"$marker\"\n    exec \"$real_binary\" \"$@\"\nfi\nexit 1\n",
    );
    fs::write(path, script).unwrap();
    make_executable(path);
}

fn record_fixture_identities(
    state: &StateRoot,
    identity_a: &RuntimeIdentity,
    identity_b: &RuntimeIdentity,
) {
    let mut registry = HostRegistry::open(state).unwrap();
    for identity in [identity_a, identity_b] {
        let record = registry
            .runtime_by_id(identity.runtime_id)
            .unwrap()
            .expect("fixture Runtime record must exist");
        registry
            .record_runtime_process_identity(
                identity.runtime_id,
                record.revision,
                identity.provider_pid,
                &identity.process_birth,
            )
            .unwrap();
    }
}

fn assert_registry_identities(
    state: &StateRoot,
    fixture: &RuntimeFixture<'_>,
    identity: &RuntimeIdentity,
) {
    let registry = HostRegistry::open(state).unwrap();
    let record = registry
        .runtime_by_id(fixture.record.runtime_id)
        .unwrap()
        .expect("fixture Runtime record must remain durable");
    assert_eq!(record.workstream_id, identity.workstream_id);
    assert_eq!(record.provider, ProviderKind::Codex);
    assert_eq!(record.tmux_generation, identity.tmux_generation);
    assert_eq!(record.tmux_session, identity.tmux_session);
    assert_eq!(record.provider_pid, Some(identity.provider_pid));
    assert_eq!(
        record.process_birth.as_deref(),
        Some(identity.process_birth.as_str())
    );
    assert_eq!(record.status, RuntimeStatus::Starting);
}

fn switch_once(
    presentation: &Presentation,
    fixture: &RuntimeFixture<'_>,
    study_root: &Path,
) -> PhaseSample {
    let total_started = Instant::now();
    let navigator_started = Instant::now();
    presentation.focus_navigator().unwrap();
    let navigator_focus = elapsed_ms(navigator_started);

    let replacement_started = Instant::now();
    let status = presentation
        .attach_workstream(fixture.record.workstream_id)
        .unwrap();
    let outer_replacement = elapsed_ms(replacement_started);

    let provider_focus_started = Instant::now();
    presentation.focus_provider().unwrap();
    let provider_focus = elapsed_ms(provider_focus_started);

    let helper_started = Instant::now();
    wait_for_attachment_marker(study_root, fixture.record.workstream_id, status.attempt_id);
    wait_for_helper_start(
        presentation,
        status.attempt_id,
        fixture.record.workstream_id,
    );
    let helper_start_observation = elapsed_ms(helper_started);

    let client_started = Instant::now();
    let client = wait_for_runtime_client(
        &fixture.runtime.paths().socket,
        &fixture.runtime.paths().session_name,
    );
    assert!(!client.name.is_empty());
    assert!(!client.tty.is_empty());
    let runtime_client_attachment = elapsed_ms(client_started);
    PhaseSample {
        navigator_focus,
        outer_replacement,
        provider_focus,
        helper_start_observation,
        runtime_client_attachment,
        activation_to_attached: elapsed_ms(total_started),
    }
}

fn wait_for_identity(fixture: &RuntimeFixture<'_>) -> RuntimeIdentity {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        if let RuntimeProbe::Live {
            pane_pid,
            cwd,
            process_birth: Some(process_birth),
            ..
        } = fixture.runtime.probe().unwrap()
            && cwd == fixture.record.cwd
            && fixture.identity_pid.is_file()
            && fixture.identity_session.is_file()
        {
            let marker_pid = fs::read_to_string(&fixture.identity_pid)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok());
            let native_session = fs::read_to_string(&fixture.identity_session)
                .ok()
                .map(|value| value.trim().to_owned());
            if marker_pid == Some(pane_pid)
                && native_session.as_deref() == Some(fixture.native_session.as_str())
            {
                return RuntimeIdentity {
                    workstream_id: fixture.record.workstream_id,
                    runtime_id: fixture.record.runtime_id,
                    tmux_generation: fixture.record.tmux_generation.clone(),
                    tmux_socket: fixture.runtime.paths().socket.clone(),
                    tmux_session: fixture.runtime.paths().session_name.clone(),
                    provider_pid: pane_pid,
                    process_birth,
                    native_session: native_session.expect("checked native session marker"),
                };
            }
        }
        assert!(
            Instant::now() < deadline,
            "runtime identity did not become live: {}",
            fixture.record.workstream_id
        );
        thread::sleep(READINESS_POLL);
    }
}

fn assert_invisible_progress(
    invisible: &RuntimeFixture<'_>,
    identity: &RuntimeIdentity,
    visible: &RuntimeFixture<'_>,
) {
    wait_until_no_runtime_clients(
        &invisible.runtime.paths().socket,
        &invisible.runtime.paths().session_name,
    );
    let before = read_counter(&invisible.progress_marker);
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        let current = read_counter(&invisible.progress_marker);
        if current > before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "invisible provider stopped progressing: {}",
            invisible.record.workstream_id
        );
        thread::sleep(READINESS_POLL);
    }
    assert_eq!(&wait_for_identity(invisible), identity);
    let visible_client = prove_runtime_client(
        &visible.runtime.paths().socket,
        &visible.runtime.paths().session_name,
    );
    assert!(!visible_client.name.is_empty());
    assert!(!visible_client.tty.is_empty());
}

fn wait_for_attachment_marker(
    study_root: &Path,
    workstream_id: WorkstreamId,
    attempt_id: uuid::Uuid,
) {
    let marker = study_root.join(format!("{workstream_id}-{attempt_id}.provider_exec"));
    let deadline = Instant::now() + READINESS_TIMEOUT;
    while !marker.is_file() {
        assert!(
            Instant::now() < deadline,
            "provider helper did not exec wsnav"
        );
        thread::sleep(READINESS_POLL);
    }
}

fn wait_for_helper_start(
    presentation: &Presentation,
    attempt_id: uuid::Uuid,
    workstream_id: WorkstreamId,
) {
    // Running is the helper's start observation. Production reports it before
    // preflight and runtime preparation, so this is deliberately not labeled
    // as a Runtime-preparation phase.
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        let status = presentation
            .attachment_status()
            .unwrap_or_else(|error| panic!("attachment metadata query failed: {error}"));
        if let Some(status) = status {
            assert_eq!(status.attempt_id, attempt_id);
            assert_eq!(status.workstream_id, workstream_id);
            match status.phase {
                AttachmentPhase::Running => return,
                AttachmentPhase::Failed => panic!("production attachment helper failed"),
                AttachmentPhase::Pending | AttachmentPhase::Completed => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "attachment did not report running"
        );
        thread::sleep(READINESS_POLL);
    }
}

fn wait_for_runtime_client(socket: &Path, session: &str) -> RuntimeClient {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        let count = runtime_client_count(socket)
            .unwrap_or_else(|error| panic!("runtime client metadata query failed: {error}"));
        if count > 0 {
            let clients = list_tmux_clients(socket)
                .unwrap_or_else(|error| panic!("runtime client metadata query failed: {error}"));
            assert_eq!(
                clients.len(),
                1,
                "runtime must have exactly one nested client"
            );
            let client = clients.into_iter().next().expect("count proved one client");
            assert_eq!(client.session, session);
            return client;
        }
        assert!(
            Instant::now() < deadline,
            "runtime client did not attach: {session}"
        );
        thread::sleep(READINESS_POLL);
    }
}

fn prove_runtime_client(socket: &Path, session: &str) -> RuntimeClient {
    let clients = list_tmux_clients(socket)
        .unwrap_or_else(|error| panic!("runtime client metadata query failed: {error}"));
    assert_eq!(
        clients.len(),
        1,
        "runtime must have exactly one nested client"
    );
    let client = clients.into_iter().next().expect("count proved one client");
    assert_eq!(client.session, session);
    client
}

fn wait_until_no_runtime_clients(socket: &Path, session: &str) {
    let deadline = Instant::now() + READINESS_TIMEOUT;
    loop {
        let clients = list_tmux_clients(socket)
            .unwrap_or_else(|error| panic!("runtime client metadata query failed: {error}"));
        if clients.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "runtime client did not retire: {session}"
        );
        thread::sleep(READINESS_POLL);
    }
}

fn runtime_client_count(socket: &Path) -> Result<usize, String> {
    list_tmux_clients(socket).map(|clients| clients.len())
}

fn list_tmux_clients(socket: &Path) -> Result<Vec<RuntimeClient>, String> {
    let mut command = tmux_command(socket);
    command.args([
        "list-clients",
        "-F",
        "#{client_name}\t#{client_session}\t#{client_tty}",
    ]);
    let output = output_bounded(&mut command, MAX_METADATA_BYTES, MAX_METADATA_BYTES)
        .map_err(|error| format!("bounded tmux metadata query failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "tmux list-clients rejected: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|_| "tmux list-clients returned invalid UTF-8".to_owned())?;
    output
        .lines()
        .map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) {
                return Err(format!("malformed tmux client metadata: {line:?}"));
            }
            Ok(RuntimeClient {
                name: fields[0].to_owned(),
                session: fields[1].to_owned(),
                tty: fields[2].to_owned(),
            })
        })
        .collect()
}

fn attach_tmux_client(paths: &PresentationPaths) -> ChildGuard {
    let command = format!(
        "env -u TMUX tmux -S {} attach-session -t {}",
        shell_quote(&paths.socket),
        shell_quote(Path::new(&paths.session_name)),
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
    loop {
        match list_tmux_clients(&paths.socket) {
            Ok(clients)
                if clients
                    .iter()
                    .any(|client| client.session == paths.session_name) =>
            {
                return ChildGuard(child);
            }
            Ok(_) => {}
            Err(error) => {
                terminate_child(
                    child,
                    &format!("presentation client metadata query failed: {error}"),
                );
            }
        }
        if Instant::now() >= deadline {
            terminate_child(child, "private presentation client did not attach");
        }
        thread::sleep(READINESS_POLL);
    }
}

fn terminate_child(mut child: Child, message: &str) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    panic!("{message}");
}

fn percentile<F>(samples: &[PhaseSample], select: F, numerator: usize, denominator: usize) -> f64
where
    F: Fn(&PhaseSample) -> f64,
{
    assert!(!samples.is_empty());
    assert!(denominator > 0);
    let mut values: Vec<_> = samples.iter().map(select).collect();
    values.sort_by(f64::total_cmp);
    let rank = (values.len() * numerator).div_ceil(denominator);
    let index = rank.saturating_sub(1).min(values.len() - 1);
    values[index]
}

fn read_counter(path: &Path) -> u64 {
    let value = fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "progress marker is unavailable: {}: {error}",
            path.display()
        )
    });
    value
        .trim()
        .parse::<u64>()
        .unwrap_or_else(|error| panic!("progress marker is malformed: {}: {error}", path.display()))
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
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

fn shell_quote(path: &Path) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_standard_nearest_rank() {
        let samples = [
            PhaseSample {
                navigator_focus: 1.0,
                ..PhaseSample::default()
            },
            PhaseSample {
                navigator_focus: 2.0,
                ..PhaseSample::default()
            },
            PhaseSample {
                navigator_focus: 3.0,
                ..PhaseSample::default()
            },
            PhaseSample {
                navigator_focus: 4.0,
                ..PhaseSample::default()
            },
        ];
        assert!(
            (percentile(&samples, |sample| sample.navigator_focus, 25, 100) - 1.0).abs()
                < f64::EPSILON
        );
        assert!(
            (percentile(&samples, |sample| sample.navigator_focus, 50, 100) - 2.0).abs()
                < f64::EPSILON
        );
        assert!(
            (percentile(&samples, |sample| sample.navigator_focus, 95, 100) - 4.0).abs()
                < f64::EPSILON
        );
    }
}
