use super::{
    EphemeralAppServer, LifecycleEvent, LinuxProcessProbe, ObserverProfile, Path, PathBuf,
    PrivateRuntime, ProviderSessionId, RuntimePaths, RuntimeProbe, StateRoot, SystemTmux, env,
    is_direct_provider_hook,
};
use crate::provider::codex::app_server::ThreadMetadata;
use crate::provider::codex::hooks::drain_stdin_and_parse_until;
use crate::state::{ObserverDatabaseDeadline, open_current};
use std::time::{Duration, Instant};

const CODEX_HOOK_PREPARATION_BUDGET: Duration = Duration::from_millis(1_750);
const CODEX_HOOK_DATABASE_BUDGET: Duration = Duration::from_millis(750);
const CODEX_HOOK_METADATA_RESERVE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThreadMetadataRefresh {
    Required,
    BestEffort,
    None,
}

const fn thread_metadata_refresh(event: LifecycleEvent) -> ThreadMetadataRefresh {
    match event {
        LifecycleEvent::SessionStart => ThreadMetadataRefresh::Required,
        LifecycleEvent::Stop => ThreadMetadataRefresh::BestEffort,
        LifecycleEvent::UserPromptSubmit | LifecycleEvent::SessionEnd => {
            ThreadMetadataRefresh::None
        }
    }
}

fn read_thread_metadata_for_hook(
    event: LifecycleEvent,
    native_session_id: &str,
    preparation_deadline: Instant,
) -> Result<Option<ThreadMetadata>, ()> {
    match thread_metadata_refresh(event) {
        ThreadMetadataRefresh::Required => EphemeralAppServer::default()
            .read_thread_for_hook(native_session_id, preparation_deadline)
            .map(Some)
            .map_err(|_| ()),
        ThreadMetadataRefresh::BestEffort => Ok(preparation_deadline
            .checked_sub(CODEX_HOOK_METADATA_RESERVE)
            .filter(|deadline| Instant::now() < *deadline)
            .and_then(|deadline| {
                EphemeralAppServer::default()
                    .read_thread_for_hook(native_session_id, deadline)
                    .ok()
            })),
        ThreadMetadataRefresh::None => Ok(None),
    }
}

pub(super) fn observer_profile(
    root: &StateRoot,
) -> Result<ObserverProfile, super::model::AppError> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or(super::model::AppError::CodexHomeUnavailable)?;
    let executable = env::current_exe().map_err(super::model::AppError::Io)?;
    Ok(ObserverProfile::new(codex_home, executable, root.base()))
}

pub(super) fn observe_hook(state_root: Option<PathBuf>) {
    // The preparation deadline starts before reading stdin. A provider hook
    // may leave its pipe open, so payload drain must not consume the entire
    // outer hook budget before state/process evidence gets a chance to run.
    let preparation_deadline = Instant::now() + CODEX_HOOK_PREPARATION_BUDGET;
    let Ok(observation) = drain_stdin_and_parse_until(preparation_deadline) else {
        return;
    };
    let Some(state_root) = state_root else {
        return;
    };
    // Hooks are passive evidence. Select the already-existing root and open
    // only the exact current schema-15 authority; never create or migrate
    // state on this path.
    let root = StateRoot::select(state_root);
    if Instant::now() >= preparation_deadline {
        return;
    }
    let Ok(mut state) = open_current(&root) else {
        return;
    };
    if Instant::now() >= preparation_deadline {
        return;
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let Ok(candidates) = state.observer_hook_runtime_candidates() else {
        return;
    };
    let mut matches = Vec::new();
    for record in candidates {
        if Instant::now() >= preparation_deadline {
            return;
        }
        if record.cwd.as_path() != Path::new(&observation.cwd) {
            continue;
        }
        let Ok(paths) =
            RuntimePaths::for_record(root.base(), record.runtime_id, &record.tmux_session)
        else {
            continue;
        };
        let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
        let Ok(RuntimeProbe::Live {
            pane_pid,
            cwd,
            process_birth: Some(actual_birth),
            ..
        }) = runtime.probe()
        else {
            continue;
        };
        let Some(expected_birth) = record.process_birth.as_deref() else {
            continue;
        };
        if cwd == record.cwd
            && record.provider_pid == Some(pane_pid)
            && actual_birth == expected_birth
            && is_direct_provider_hook(pane_pid, expected_birth)
        {
            matches.push(record);
        }
    }
    let [record] = matches.as_slice() else {
        return;
    };
    if Instant::now() >= preparation_deadline {
        return;
    }
    let Ok(metadata) = read_thread_metadata_for_hook(
        observation.event,
        &observation.native_session_id,
        preparation_deadline,
    ) else {
        return;
    };
    let Ok(session_id) = ProviderSessionId::codex(observation.native_session_id.clone()) else {
        return;
    };
    if Instant::now() >= preparation_deadline {
        return;
    }
    // Lifecycle and optional provider metadata are one observer transition;
    // they must consume the same absolute SQLite budget rather than gaining
    // a second 750 ms window after the first write.
    let database_deadline = ObserverDatabaseDeadline::from_now(CODEX_HOOK_DATABASE_BUDGET);
    if state
        .observer_apply_codex_lifecycle_observation(
            record.runtime_id,
            &record.tmux_generation,
            &observation,
            database_deadline,
        )
        .is_ok()
        && let Some(metadata) = metadata
    {
        let _ = state.observer_record_thread_metadata(
            record.runtime_id,
            &record.tmux_generation,
            &session_id,
            metadata.name.as_deref(),
            database_deadline,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{LifecycleEvent, ThreadMetadataRefresh, thread_metadata_refresh};

    #[test]
    fn thread_names_refresh_at_binding_and_after_settled_agent_work() {
        assert_eq!(
            thread_metadata_refresh(LifecycleEvent::SessionStart),
            ThreadMetadataRefresh::Required
        );
        assert_eq!(
            thread_metadata_refresh(LifecycleEvent::Stop),
            ThreadMetadataRefresh::BestEffort
        );
        assert_eq!(
            thread_metadata_refresh(LifecycleEvent::UserPromptSubmit),
            ThreadMetadataRefresh::None
        );
        assert_eq!(
            thread_metadata_refresh(LifecycleEvent::SessionEnd),
            ThreadMetadataRefresh::None
        );
    }
}
