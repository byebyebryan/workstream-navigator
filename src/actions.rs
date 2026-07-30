//! Host-local lifecycle actions shared by direct CLI and remote protocol paths.
//!
//! These actions own native process effects. The CLI and SSH protocol only
//! parse intent and render outcomes; neither gets to reimplement launch or
//! private-tmux authority.

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    domain::{Revision, RuntimeId, WorkstreamId},
    provider::codex::profile::{ObserverProfile, ProfileError},
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
    },
    state::{HostRegistry, IntegrationLifecycle, ProviderBinding, StateError},
};

pub(crate) const OBSERVER_AUTHORITY: &str = "wsnav-observer-v1";
const PARK_CONFIRM_TIMEOUT: Duration = Duration::from_millis(500);
const PARK_CONFIRM_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The durable outcome of a start-or-resume request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    Started,
    AlreadyLive,
}

/// Starts or resumes exactly one Workstream using the host's owned Codex
/// profile and private tmux Runtime.
///
/// # Errors
///
/// Returns an error when the expected Workstream revision is stale, observer
/// ownership/trust is incomplete, process evidence is ambiguous, or the
/// native launch cannot be reconciled safely.
pub fn start(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<StartOutcome, ActionError> {
    ensure_workstream_revision(registry, workstream_id, expected_revision)?;
    let integration = registry
        .codex_integration()?
        .ok_or(ActionError::ObserverNotInstalled)?;
    if integration.lifecycle != IntegrationLifecycle::Ready {
        return Err(ActionError::ObserverNotReady);
    }
    let manager = observer_profile()?;
    manager.install(
        integration.ownership.owner_id.clone(),
        Some(&integration.ownership),
    )?;
    manager.verify_native_trust(&integration.ownership)?;
    let prior_runtime = registry.runtime_for_workstream(workstream_id)?;
    if let Some(prior_runtime) = &prior_runtime {
        let tmux = SystemTmux::default();
        let process_probe = LinuxProcessProbe;
        let prior = PrivateRuntime::new(
            &tmux,
            &process_probe,
            RuntimePaths::for_runtime(root.base(), prior_runtime.runtime_id),
        );
        match prior.probe()? {
            RuntimeProbe::Live { .. } => return Ok(StartOutcome::AlreadyLive),
            RuntimeProbe::Missing => {
                if !matches!(prior_runtime.status, crate::domain::RuntimeStatus::Stopped) {
                    registry
                        .mark_runtime_stopped(prior_runtime.runtime_id, prior_runtime.revision)?;
                }
            }
            RuntimeProbe::Unknown { .. } => return Err(ActionError::RuntimeProbeAmbiguous),
        }
    }
    let prior_binding = prior_runtime
        .as_ref()
        .map(|runtime| registry.binding_for_runtime(runtime.runtime_id))
        .transpose()?
        .flatten();
    let record = registry.reserve_runtime(workstream_id)?;
    let paths = RuntimePaths::for_runtime(root.base(), record.runtime_id);
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(&tmux, &process_probe, paths);
    let launch = NativeLaunch {
        cwd: record.cwd.clone(),
        program: codex_launch_program(&record.cwd, prior_binding.as_ref()),
        environment: managed_codex_environment(
            root.base(),
            &record.runtime_id,
            &record.tmux_generation,
        ),
    };
    if let Err(error) = runtime.start(&launch) {
        let _ = registry.mark_runtime_stopped(record.runtime_id, record.revision);
        return Err(ActionError::Runtime(error));
    }
    let process_birth = match runtime.probe()? {
        RuntimeProbe::Live {
            cwd,
            process_birth: Some(process_birth),
            ..
        } if cwd == record.cwd => process_birth,
        RuntimeProbe::Live { .. } | RuntimeProbe::Missing | RuntimeProbe::Unknown { .. } => {
            let _ = runtime.park();
            let _ = registry.mark_runtime_stopped(record.runtime_id, record.revision);
            return Err(ActionError::RuntimeProbeAmbiguous);
        }
    };
    if let Err(error) =
        registry.record_runtime_process_birth(record.runtime_id, record.revision, &process_birth)
    {
        let _ = runtime.park();
        let _ = registry.mark_runtime_stopped(record.runtime_id, record.revision);
        return Err(ActionError::State(error));
    }
    Ok(StartOutcome::Started)
}

/// Parks one live Runtime while preserving its provider history and checkout.
///
/// # Errors
///
/// Returns an error when the expected Workstream revision is stale, the
/// runtime cannot be parked, or durable state cannot record the exact effect.
pub fn park(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<Revision, ActionError> {
    ensure_workstream_revision(registry, workstream_id, expected_revision)?;
    let record = registry
        .runtime_for_workstream(workstream_id)?
        .ok_or(ActionError::NoRuntime(workstream_id))?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), record.runtime_id),
    );
    runtime.park()?;
    registry.park_runtime(record.runtime_id, record.revision)?;
    workstream_revision(registry, workstream_id)
}

/// Waits briefly for the durable outcome of a concurrently requested park.
///
/// Parking first stops the private tmux server, which makes an already
/// attached native client exit before the park action can commit its `SQLite`
/// transaction. Treat that exit as clean only after the exact Runtime and
/// Workstream record the deliberate parked outcome. A crash, stale Runtime,
/// or replacement generation never satisfies this predicate.
///
/// # Errors
///
/// Returns an error when the registry cannot be opened or queried.
pub fn await_deliberate_park(
    root: &crate::state::StateRoot,
    runtime_id: RuntimeId,
    workstream_id: WorkstreamId,
) -> Result<bool, StateError> {
    let deadline = Instant::now() + PARK_CONFIRM_TIMEOUT;
    loop {
        let registry = HostRegistry::open(root)?;
        if registry.runtime_is_deliberately_parked(runtime_id, workstream_id)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(PARK_CONFIRM_POLL_INTERVAL);
    }
}

/// Builds the only native provider command permitted for a managed Runtime.
#[must_use]
pub fn codex_launch_program(
    cwd: &Path,
    binding: Option<&ProviderBinding>,
) -> Vec<std::ffi::OsString> {
    let mut program = vec![
        "codex".into(),
        "--profile".into(),
        "wsnav-observer".into(),
        "-C".into(),
        cwd.as_os_str().to_owned(),
    ];
    if let Some(binding) = binding {
        program.push("resume".into());
        program.push(binding.native_session_id.clone().into());
    }
    program
}

/// Builds the environment owned by a managed Codex Runtime.
///
/// Remote starts use one-shot non-interactive SSH commands. Those commands can
/// have a POSIX locale even when the terminal that later attaches is UTF-8.
/// Set the locale only for the owned Codex process (and its hook children), so
/// its terminal renderer has a stable UTF-8 contract without changing the
/// user's shell or an unmanaged provider session.
fn managed_codex_environment(
    state_root: &Path,
    runtime_id: &RuntimeId,
    runtime_generation: &str,
) -> BTreeMap<OsString, OsString> {
    const UTF8_LOCALE: &str = "C.UTF-8";

    BTreeMap::from([
        ("LANG".into(), UTF8_LOCALE.into()),
        ("LC_CTYPE".into(), UTF8_LOCALE.into()),
        ("LC_ALL".into(), UTF8_LOCALE.into()),
        ("WSNAV_STATE_ROOT".into(), state_root.as_os_str().to_owned()),
        ("WSNAV_RUNTIME_ID".into(), runtime_id.to_string().into()),
        ("WSNAV_RUNTIME_GENERATION".into(), runtime_generation.into()),
        ("WSNAV_OBSERVER_AUTHORITY".into(), OBSERVER_AUTHORITY.into()),
    ])
}

fn ensure_workstream_revision(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
) -> Result<(), ActionError> {
    let Some(expected_revision) = expected_revision else {
        return Ok(());
    };
    let current = workstream_revision(registry, workstream_id)?;
    if current != expected_revision {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    Ok(())
}

fn workstream_revision(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<Revision, ActionError> {
    registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .map(|overview| overview.revision)
        .ok_or(ActionError::UnknownWorkstream)
}

fn observer_profile() -> Result<ObserverProfile, ActionError> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or(ActionError::CodexHomeUnavailable)?;
    let executable = env::current_exe().map_err(ActionError::Io)?;
    Ok(ObserverProfile::new(codex_home, executable))
}

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("CODEX_HOME cannot be determined")]
    CodexHomeUnavailable,
    #[error("I/O: {0}")]
    Io(std::io::Error),
    #[error("workstream {0} has no runtime")]
    NoRuntime(WorkstreamId),
    #[error("observer profile is not installed; run wsnav setup")]
    ObserverNotInstalled,
    #[error(
        "observer profile trust is pending; run wsnav setup and complete native Codex /hooks review"
    )]
    ObserverNotReady,
    #[error("private runtime probe is ambiguous; refusing to create another Codex process")]
    RuntimeProbeAmbiguous,
    #[error("workstream is unknown")]
    UnknownWorkstream,
    #[error("workstream revision changed; refresh before acting")]
    WorkstreamRevisionConflict,
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error(transparent)]
    State(#[from] StateError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_codex_environment_has_an_explicit_utf8_locale() {
        let environment =
            managed_codex_environment(Path::new("/state"), &RuntimeId::new(), "runtime-generation");

        for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
            assert_eq!(
                environment.get(&OsString::from(key)),
                Some(&OsString::from("C.UTF-8"))
            );
        }
        assert_eq!(
            environment.get(&OsString::from("WSNAV_STATE_ROOT")),
            Some(&OsString::from("/state"))
        );
    }
}
