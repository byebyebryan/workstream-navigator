use super::model::{AppError, runtime_probe_label};
use super::{
    EphemeralAppServer, HostRegistry, LifecycleEvent, LinuxProcessProbe, ObserverProfile,
    OperationId, Path, PathBuf, PrivateRuntime, ProviderSessionId, Revision, RuntimePaths,
    RuntimeProbe, StateRoot, Stdio, SystemTmux, WorkstreamId, actions, drain_and_parse, env,
    is_direct_provider_hook,
};

pub(super) fn start(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    match actions::start(root, registry, workstream_id, None)? {
        actions::StartOutcome::Started => println!("started workstream {workstream_id}"),
        actions::StartOutcome::AlreadyLive => {
            println!("workstream {workstream_id} is already live");
        }
    }
    Ok(())
}

pub(super) fn recover(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    match actions::recover(root, registry, workstream_id, None)? {
        actions::StartOutcome::Started => {
            println!("recovering workstream {workstream_id}; completing exact native resume");
        }
        actions::StartOutcome::AlreadyLive => {
            println!("workstream {workstream_id} is already live");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn codex_launch_program(
    cwd: &Path,
    binding: Option<&crate::state::ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    actions::codex_launch_program(cwd, binding)
}

pub(super) fn observer_profile(root: &StateRoot) -> Result<ObserverProfile, AppError> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or(AppError::CodexHomeUnavailable)?;
    let executable = env::current_exe().map_err(AppError::Io)?;
    Ok(ObserverProfile::new(codex_home, executable, root.base()))
}

pub(super) fn observe_hook(state_root: Option<PathBuf>) {
    // Drain before inspecting state or process evidence. Codex can still be
    // writing a large lifecycle payload when an unmanaged hook is rejected.
    let Ok(observation) = drain_and_parse(&mut std::io::stdin().lock()) else {
        return;
    };
    let Some(state_root) = state_root else {
        return;
    };
    let Ok(root) = StateRoot::create(state_root) else {
        return;
    };
    let Ok(mut registry) = HostRegistry::open(&root) else {
        return;
    };
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let Ok(candidates) = registry.hook_runtime_candidates() else {
        return;
    };
    let matches = candidates
        .into_iter()
        .filter(|record| record.cwd.as_path() == Path::new(&observation.cwd))
        .filter_map(|record| {
            let paths =
                RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)
                    .ok()?;
            let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
            let RuntimeProbe::Live {
                pane_pid,
                cwd,
                process_birth: Some(actual_birth),
                ..
            } = runtime.probe().ok()?
            else {
                return None;
            };
            let expected_birth = record.process_birth.as_deref()?;
            (cwd == record.cwd
                && record.provider_pid == Some(pane_pid)
                && actual_birth == expected_birth
                && is_direct_provider_hook(pane_pid, expected_birth))
            .then_some(record)
        })
        .collect::<Vec<_>>();
    let [record] = matches.as_slice() else {
        return;
    };
    let metadata = if matches!(observation.event, LifecycleEvent::SessionStart) {
        match EphemeralAppServer::default().read_thread_for_hook(&observation.native_session_id) {
            Ok(metadata) => Some(metadata),
            Err(_) => return,
        }
    } else {
        None
    };
    let Ok(session_id) = ProviderSessionId::codex(observation.native_session_id.clone()) else {
        return;
    };
    if registry
        .apply_lifecycle_observation(record.runtime_id, &record.tmux_generation, observation)
        .is_ok()
        && let Some(metadata) = metadata
    {
        let _ = registry.record_thread_metadata(
            record.runtime_id,
            &session_id,
            metadata.name.as_deref(),
        );
    }
}

pub(super) fn rename(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    revision: Revision,
    name: &str,
) -> Result<(), AppError> {
    actions::rename(registry, workstream_id, revision, name)?;
    println!("renamed workstream {workstream_id}");
    Ok(())
}

pub(super) fn attach(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let record = actions::preflight_attachment(root, registry, workstream_id)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    runtime.prepare_attach()?;
    let mut command = runtime.attach_command();
    command.stderr(Stdio::null());
    let status = command.status().map_err(AppError::Io)?;
    if status.success()
        || actions::await_deliberate_park(root, record.runtime_id, record.workstream_id)?
    {
        Ok(())
    } else {
        Err(AppError::AttachFailed)
    }
}

pub(super) fn park(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    actions::park(root, registry, workstream_id, None)?;
    println!("parked workstream {workstream_id}");
    Ok(())
}

pub(super) fn archive(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    revision: Revision,
) -> Result<(), AppError> {
    actions::archive(root, registry, workstream_id, revision)?;
    println!("archived workstream {workstream_id}");
    Ok(())
}

pub(super) fn restore(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    revision: Revision,
) -> Result<(), AppError> {
    actions::restore(registry, workstream_id, revision)?;
    println!("restored workstream {workstream_id}");
    Ok(())
}

pub(super) fn status(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    actions::reconcile_lost_runtimes(root, registry)?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(AppError::NoRuntime(workstream_id))?;
    let record = overview.runtime.ok_or(AppError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)?,
    );
    let probe = runtime.probe()?;
    let binding = registry.binding_for_runtime(record.runtime_id)?.is_some();
    let attention = overview.attention;
    println!("workstream: {:?}", overview.lifecycle);
    println!("lifecycle: {:?}", record.status);
    println!("private runtime: {}", runtime_probe_label(&probe));
    println!(
        "provider binding: {}",
        if binding { "bound" } else { "pending" }
    );
    println!(
        "result attention: {}",
        if attention
            .as_ref()
            .and_then(|value| value.result_unseen_since_revision)
            .is_some()
        {
            "unseen"
        } else {
            "none"
        }
    );
    println!(
        "recovery attention: {}",
        if attention
            .as_ref()
            .and_then(|value| value.recovery_unseen_since_revision)
            .is_some()
        {
            "unseen"
        } else {
            "none"
        }
    );
    Ok(())
}

pub(super) fn operations(registry: &HostRegistry) -> Result<(), AppError> {
    let operations = registry.unresolved_operation_overviews()?;
    print_operations(operations.into_iter().map(|operation| {
        (
            operation.operation_id,
            operation.kind,
            operation.phase,
            operation.revision.value(),
        )
    }));
    Ok(())
}

pub(super) fn print_operations(
    operations: impl IntoIterator<
        Item = (
            OperationId,
            crate::domain::OperationKind,
            crate::domain::OperationPhase,
            i64,
        ),
    >,
) {
    let mut any = false;
    for (operation_id, kind, phase, revision) in operations {
        any = true;
        println!("operation {operation_id} {kind:?} {phase:?} revision {revision}");
    }
    if !any {
        println!("no unresolved operations");
    }
}

pub(super) fn recover_operation(
    root: &StateRoot,
    registry: &mut HostRegistry,
    operation_id: OperationId,
) -> Result<(), AppError> {
    let workstream_id = actions::recover_managed_operation(root, registry, operation_id)?;
    println!("recovered operation {operation_id}; workstream {workstream_id}");
    Ok(())
}
