//! Host-side implementation of the bounded `WSNav` control protocol.
//!
//! This module is invoked by the hidden `_remote` command, locally or through
//! SSH. It intentionally owns no listener: each request is one short-lived
//! process with exactly one JSON request and response frame.

use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    process::Stdio,
};

use thiserror::Error;

use crate::{
    domain::{Revision, RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle},
    protocol::{
        CURRENT_PROTOCOL_VERSION, Capabilities, HelloResponse, HostAction, HostRequest,
        HostResponse, MAX_FRAME_BYTES, ObserverStatus, OperationSnapshot, OperationsResponse,
        RequestEnvelope, ResponseEnvelope, SnapshotResponse, SnapshotWorkstream,
    },
    provider::names::{NameContext, resolve_name},
    runtime::{LinuxProcessProbe, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux},
    state::{HostRegistry, IntegrationLifecycle, StateError, StateRoot, WorkstreamOverview},
};

/// Serves one stdin/stdout protocol exchange for a local or SSH caller.
///
/// The response is always one protocol frame when stdout is writable. Request
/// failures deliberately become generic bounded rejections so remote clients
/// do not receive state paths, `SQLite` diagnostics, or provider details.
///
/// # Errors
///
/// Returns an error only if the response frame cannot be written to stdout.
pub fn serve(
    state_root: Option<std::path::PathBuf>,
    input: &mut impl Read,
    output: &mut impl Write,
) -> Result<(), RemoteError> {
    let request = read_frame(input)
        .and_then(|frame| RequestEnvelope::decode(&frame).map_err(RemoteError::Protocol));
    let response = match request {
        Ok(request) => dispatch(state_root, &request),
        Err(error) => rejection_for_error(&error),
    };
    let frame = response.encode()?;
    output.write_all(&frame)?;
    output.flush()?;
    Ok(())
}

/// Attaches the current terminal to exactly one already-known remote Runtime.
/// This path is intentionally separate from the JSON control exchange: it is
/// the native terminal stream reached through `ssh -tt`, with no management
/// output or watch channel.
///
/// # Errors
///
/// Returns an error for an unknown/non-live runtime or a failed private tmux
/// attachment.
pub fn attach(root: &StateRoot, runtime_id: RuntimeId) -> Result<(), RemoteError> {
    let mut registry = HostRegistry::open(root)?;
    let runtime_record = crate::actions::preflight_attachment_runtime(
        root,
        &mut registry,
        runtime_id,
    )
    .map_err(|error| match error {
        crate::actions::ActionError::RuntimeProbeAmbiguous => RemoteError::RuntimeUnavailable,
        other => RemoteError::Action(other),
    })?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(
            root.base(),
            runtime_record.runtime_id,
            &runtime_record.tmux_session,
        )?,
    );
    if !matches!(runtime.probe()?, RuntimeProbe::Live { .. }) {
        return Err(RemoteError::RuntimeUnavailable);
    }
    runtime.prepare_attach()?;
    let mut command = runtime.attach_command();
    command.stderr(Stdio::null());
    let status = command.status()?;
    if status.success()
        || crate::actions::await_deliberate_park(
            root,
            runtime_record.runtime_id,
            runtime_record.workstream_id,
        )?
    {
        Ok(())
    } else {
        Err(RemoteError::AttachFailed)
    }
}

/// Runs the host side of a remote presentation utility shell. Only the
/// opaque Workstream ID crosses SSH; this function resolves the authoritative
/// `ProjectLocation` root and account shell from host-local state.
///
/// # Errors
///
/// Returns a bounded error when the Workstream, Runtime, project root, or
/// account shell cannot be corroborated.
pub fn presentation_shell(
    root: &StateRoot,
    workstream_id: WorkstreamId,
) -> Result<(), RemoteError> {
    let (runtime, overview) = preflight_presentation(root, workstream_id)?;
    let shell = current_account_shell()?;
    let plan = presentation_shell_plan(&overview.project_repository_path, &runtime.cwd, &shell)?;
    let mut command = Command::new(&plan.shell);
    command.current_dir(&plan.cwd).args(&plan.arguments);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(RemoteError::Io(command.exec()))
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(RemoteError::PresentationUnavailable)
        }
    }
}

#[cfg(unix)]
fn current_account_shell() -> Result<PathBuf, RemoteError> {
    use nix::unistd::{User, geteuid};

    let user = User::from_uid(geteuid())
        .map_err(|_| RemoteError::PresentationUnavailable)?
        .ok_or(RemoteError::PresentationUnavailable)?;
    Ok(user.shell)
}

#[cfg(not(unix))]
fn current_account_shell() -> Result<PathBuf, RemoteError> {
    env::var_os("SHELL")
        .map(PathBuf::from)
        .ok_or(RemoteError::PresentationUnavailable)
}

/// Sends one literal C-b to the exact remote private Runtime. The host
/// preflight is repeated here so a stale SSH invocation cannot target a
/// replacement Runtime or a different Workstream.
///
/// # Errors
///
/// Returns a bounded error when the Workstream, Runtime, or private tmux
/// server cannot be corroborated.
pub fn presentation_literal_ctrl_b(
    root: &StateRoot,
    workstream_id: WorkstreamId,
) -> Result<(), RemoteError> {
    let (runtime, _overview) = preflight_presentation(root, workstream_id)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_record(root.base(), runtime.runtime_id, &runtime.tmux_session)?,
    );
    runtime.send_literal_ctrl_b()?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresentationShellPlan {
    cwd: PathBuf,
    shell: PathBuf,
    arguments: Vec<OsString>,
}

fn preflight_presentation(
    root: &StateRoot,
    workstream_id: WorkstreamId,
) -> Result<(crate::state::RuntimeRecord, WorkstreamOverview), RemoteError> {
    let mut registry = HostRegistry::open(root)?;
    let overview = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .ok_or(RemoteError::PresentationUnavailable)?;
    let runtime = crate::actions::preflight_attachment(root, &mut registry, workstream_id)
        .map_err(|_| RemoteError::PresentationUnavailable)?;
    validate_presentation_state(&overview, Some(&runtime))?;
    Ok((runtime, overview))
}

fn validate_presentation_state(
    overview: &WorkstreamOverview,
    runtime: Option<&crate::state::RuntimeRecord>,
) -> Result<PathBuf, RemoteError> {
    if overview.archived_at_millis.is_some() || overview.lifecycle != WorkstreamLifecycle::Open {
        return Err(RemoteError::PresentationUnavailable);
    }
    let runtime = runtime.ok_or(RemoteError::PresentationUnavailable)?;
    // The caller has already corroborated the exact live provider process.
    // `Starting` therefore means lifecycle hooks are still pending, not that
    // the Runtime is merely reserved or safe to adopt by identity guesswork.
    if runtime.workstream_id != overview.workstream_id
        || !matches!(
            runtime.status,
            RuntimeStatus::Starting
                | RuntimeStatus::Idle
                | RuntimeStatus::Working
                | RuntimeStatus::Attention
        )
    {
        return Err(RemoteError::PresentationUnavailable);
    }
    // Canonicalization is performed on the host, after authoritative state
    // lookup. No repository path is accepted from the SSH command line.
    resolve_presentation_root(&overview.project_repository_path, &runtime.cwd)
}

fn presentation_shell_plan(
    project_root: &Path,
    runtime_cwd: &Path,
    shell: &Path,
) -> Result<PresentationShellPlan, RemoteError> {
    let cwd = resolve_presentation_root(project_root, runtime_cwd)?;
    validate_interactive_shell(shell)?;
    Ok(PresentationShellPlan {
        cwd,
        shell: shell.to_path_buf(),
        arguments: vec!["-i".into()],
    })
}

fn resolve_presentation_root(
    project_root: &Path,
    runtime_cwd: &Path,
) -> Result<PathBuf, RemoteError> {
    if !project_root.is_absolute() || !runtime_cwd.is_absolute() || project_root != runtime_cwd {
        return Err(RemoteError::PresentationUnavailable);
    }
    let canonical_root =
        fs::canonicalize(project_root).map_err(|_| RemoteError::PresentationUnavailable)?;
    if canonical_root != project_root || !canonical_root.is_dir() {
        return Err(RemoteError::PresentationUnavailable);
    }
    Ok(canonical_root)
}

fn validate_interactive_shell(shell: &Path) -> Result<(), RemoteError> {
    let value = shell.to_str().ok_or(RemoteError::PresentationUnavailable)?;
    let valid_path = shell.is_absolute()
        && !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace());
    #[cfg(unix)]
    let executable = valid_path
        && shell.is_file()
        && fs::metadata(shell).is_ok_and(|metadata| {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        });
    #[cfg(not(unix))]
    let executable = valid_path && shell.is_file();
    if executable {
        Ok(())
    } else {
        Err(RemoteError::PresentationUnavailable)
    }
}

fn dispatch(state_root: Option<std::path::PathBuf>, request: &RequestEnvelope) -> ResponseEnvelope {
    let Ok(root) = StateRoot::create(state_root.unwrap_or_else(default_state_root)) else {
        return rejected("host state is unavailable");
    };
    let mut registry = match HostRegistry::open(&root) {
        Ok(registry) => registry,
        Err(StateError::UnsupportedSchemaVersion(_)) => {
            return rejected("host state schema requires a matching wsnav update");
        }
        Err(_) => return rejected("host state is unavailable"),
    };
    match &request.request {
        HostRequest::Hello { .. } => match registry.identity() {
            Ok(identity) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Hello(HelloResponse {
                    host_id: identity.host_id,
                    registry_generation: identity.registry_generation,
                    wsnav_version: env!("CARGO_PKG_VERSION").to_owned(),
                    capabilities: local_capabilities(),
                }),
            },
            Err(_) => rejected("host identity is unavailable"),
        },
        HostRequest::Snapshot { cursor } => match snapshot(&mut registry, *cursor) {
            Ok(snapshot) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Snapshot(snapshot),
            },
            Err(_) => rejected("host snapshot is unavailable"),
        },
        HostRequest::Operations => match operations(&registry) {
            Ok(operations) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Operations(operations),
            },
            Err(_) => rejected("host operation list is unavailable"),
        },
        HostRequest::ProjectDirectories {
            relative_path,
            include_hidden,
        } => match registry.project_directories(relative_path, *include_hidden) {
            Ok(directories) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::ProjectDirectories(directories),
            },
            Err(_) => rejected("project browser is unavailable"),
        },
        HostRequest::Attach { runtime_id } => match registry.runtime_by_id(*runtime_id) {
            Ok(Some(runtime)) => match crate::actions::preflight_attachment_runtime(
                &root,
                &mut registry,
                runtime.runtime_id,
            ) {
                Ok(_) => ResponseEnvelope {
                    version: CURRENT_PROTOCOL_VERSION,
                    response: HostResponse::Attach {
                        runtime_id: *runtime_id,
                    },
                },
                Err(crate::actions::ActionError::ProviderReadiness(error)) => {
                    provider_rejected(error)
                }
                Err(crate::actions::ActionError::UnsupportedProvider(provider)) => {
                    rejected(&format!("{provider} provider attachment is unavailable"))
                }
                Err(_) => rejected("runtime is unavailable"),
            },
            Ok(None) | Err(_) => rejected("runtime is unavailable"),
        },
        HostRequest::Apply { action } => apply(&root, &mut registry, action.clone()),
    }
}

fn apply(root: &StateRoot, registry: &mut HostRegistry, action: HostAction) -> ResponseEnvelope {
    match action {
        HostAction::PrepareObserver => {
            match crate::app::prepare_observer_activation(root, registry) {
                Ok(_) => applied(1),
                Err(_) => rejected("observer activation is unavailable"),
            }
        }
        HostAction::RemoveObserver => match crate::app::remove_observer_exact(root, registry) {
            Ok(()) => applied(1),
            Err(_) => rejected("observer removal is unavailable"),
        },
        HostAction::RegisterCheckout {
            checkout_path,
            provider,
        } => apply_register_checkout(registry, Path::new(&checkout_path), provider),
        HostAction::RegisterProjectDirectory {
            relative_path,
            provider,
        } => apply_register_project_directory(registry, &relative_path, provider),
        HostAction::SetProjectBrowserRoot { root_path } => {
            match registry.set_project_browser_root(&root_path) {
                Ok(()) => applied(1),
                Err(_) => rejected("project browser root is unavailable"),
            }
        }
        HostAction::AcknowledgeAttention {
            workstream_id,
            expected_revision,
        } => match registry.acknowledge_result_attention(
            workstream_id,
            Revision::try_from(expected_revision).expect("protocol validates positive revisions"),
        ) {
            Ok(attention) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Applied {
                    revision: attention.revision.value(),
                },
            },
            Err(
                StateError::Domain(crate::domain::DomainError::RevisionConflict { .. })
                | StateError::ConcurrentWrite,
            ) => rejected("revision conflict; refresh this host"),
            Err(_) => rejected("attention is unavailable"),
        },
        HostAction::Park {
            workstream_id,
            expected_revision,
        } => apply_park(root, registry, workstream_id, expected_revision),
        HostAction::Archive {
            workstream_id,
            expected_revision,
        } => apply_archive(root, registry, workstream_id, expected_revision),
        HostAction::Restore {
            workstream_id,
            expected_revision,
        } => apply_restore(registry, workstream_id, expected_revision),
        HostAction::Rename {
            workstream_id,
            expected_revision,
            name,
        } => apply_rename(registry, workstream_id, expected_revision, &name),
        HostAction::Start {
            workstream_id,
            expected_revision,
        } => apply_start(root, registry, workstream_id, expected_revision),
        HostAction::Recover {
            workstream_id,
            expected_revision,
        } => apply_recover(root, registry, workstream_id, expected_revision),
        HostAction::RecoverOperation { operation_id } => {
            apply_recover_operation(root, registry, operation_id)
        }
        HostAction::NewWorkstream {
            source_workstream_id,
            expected_revision,
            request_key,
            provider,
        } => apply_new_workstream(
            root,
            registry,
            source_workstream_id,
            expected_revision,
            &request_key,
            provider,
        ),
        HostAction::ForkWorkstream {
            source_workstream_id,
            expected_revision,
            request_key,
        } => apply_fork_workstream(
            root,
            registry,
            source_workstream_id,
            expected_revision,
            request_key,
        ),
    }
}

fn applied(revision: i64) -> ResponseEnvelope {
    ResponseEnvelope {
        version: CURRENT_PROTOCOL_VERSION,
        response: HostResponse::Applied { revision },
    }
}

fn apply_register_checkout(
    registry: &mut HostRegistry,
    checkout_path: &Path,
    provider: crate::domain::ProviderKind,
) -> ResponseEnvelope {
    apply_register_checkout_with(registry, checkout_path, provider, |registry, provider| {
        crate::provider::require_new_eligible(registry, provider)
    })
}

fn apply_register_checkout_with(
    registry: &mut HostRegistry,
    checkout_path: &Path,
    provider: crate::domain::ProviderKind,
    readiness: impl FnOnce(
        &HostRegistry,
        crate::domain::ProviderKind,
    ) -> Result<(), crate::provider::ProviderReadinessError>,
) -> ResponseEnvelope {
    let Ok(repository) = crate::repository::inspect(checkout_path) else {
        return rejected("project is unavailable");
    };
    if let Err(error) = readiness(registry, provider) {
        return provider_rejected(error);
    }
    match registry.register_external_workstream_with_metadata(
        &repository.project_root,
        &repository.display_name,
        repository.remote_identity_fingerprint.as_deref(),
        repository.remote_identity_display.as_deref(),
        provider,
    ) {
        Ok(registered) => ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::WorkstreamCreated {
                workstream_id: registered.workstream_id,
                provider,
                revision: Revision::INITIAL.value(),
            },
        },
        Err(_) => rejected("project registration is unavailable"),
    }
}

fn apply_register_project_directory(
    registry: &mut HostRegistry,
    relative_path: &str,
    provider: crate::domain::ProviderKind,
) -> ResponseEnvelope {
    let Ok(directory) = registry.project_browser_directory(relative_path) else {
        return rejected("project browser selection is unavailable");
    };
    apply_register_checkout(registry, directory.as_path(), provider)
}

fn apply_new_workstream(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: i64,
    request_key: &str,
    provider: crate::domain::ProviderKind,
) -> ResponseEnvelope {
    apply_created_workstream(
        &crate::actions::start_independent_workstream_with_readiness(
            root,
            registry,
            source_workstream_id,
            Revision::try_from(expected_revision).ok(),
            request_key,
            provider,
            crate::provider::require_new_eligible,
        ),
        registry,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn apply_new_workstream_with<R, S>(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: i64,
    request_key: &str,
    provider: crate::domain::ProviderKind,
    readiness: R,
    starter: S,
) -> ResponseEnvelope
where
    R: FnOnce(
        &HostRegistry,
        crate::domain::ProviderKind,
    ) -> Result<(), crate::provider::ProviderReadinessError>,
    S: FnOnce(
        &StateRoot,
        &mut HostRegistry,
        WorkstreamId,
        Option<Revision>,
        crate::domain::ProviderKind,
    ) -> Result<crate::actions::StartOutcome, crate::actions::ActionError>,
{
    apply_created_workstream(
        &crate::actions::start_independent_workstream_with_readiness_and_starter(
            root,
            registry,
            source_workstream_id,
            Revision::try_from(expected_revision).ok(),
            request_key,
            provider,
            readiness,
            starter,
        ),
        registry,
    )
}

fn apply_fork_workstream(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: i64,
    request_key: String,
) -> ResponseEnvelope {
    apply_created_workstream(
        &crate::actions::fork_workstream(
            root,
            registry,
            source_workstream_id,
            Revision::try_from(expected_revision).ok(),
            request_key,
        ),
        registry,
    )
}

fn apply_recover_operation(
    root: &StateRoot,
    registry: &mut HostRegistry,
    operation_id: crate::domain::OperationId,
) -> ResponseEnvelope {
    apply_created_workstream(
        &crate::actions::recover_managed_operation(root, registry, operation_id),
        registry,
    )
}

fn apply_created_workstream(
    outcome: &Result<WorkstreamId, crate::actions::ActionError>,
    registry: &HostRegistry,
) -> ResponseEnvelope {
    match outcome {
        Ok(workstream_id) => match registry
            .workstream_overviews()
            .ok()
            .and_then(|workstreams| {
                workstreams
                    .into_iter()
                    .find(|overview| overview.workstream_id == *workstream_id)
                    .map(|overview| (overview.provider, overview.revision))
            }) {
            Some((provider, revision)) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::WorkstreamCreated {
                    workstream_id: *workstream_id,
                    provider,
                    revision: revision.value(),
                },
            },
            None => rejected("workstream outcome needs recovery"),
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(crate::actions::ActionError::ForkRecoveryRequired) => {
            rejected("workstream outcome needs recovery")
        }
        Err(crate::actions::ActionError::OpenCodeForkExternalEffectUnknown) => rejected(
            "OpenCode Fork response was lost; an unmanaged provider session may need native inspection or cleanup; use a new explicit Fork to try again",
        ),
        Err(crate::actions::ActionError::ForkSourceUnavailable) => {
            rejected("fork source is no longer available")
        }
        Err(crate::actions::ActionError::ProviderReadiness(error)) => provider_rejected(*error),
        Err(crate::actions::ActionError::UnsupportedProvider(provider)) => {
            rejected(&format!("{provider} provider action is unavailable"))
        }
        Err(crate::actions::ActionError::ProviderRecoveryUnavailable(provider)) => {
            rejected(&format!("{provider} provider recovery is unavailable"))
        }
        Err(_) => rejected("workstream creation is unavailable"),
    }
}

fn apply_park(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: crate::domain::WorkstreamId,
    expected_revision: i64,
) -> ResponseEnvelope {
    match crate::actions::park(
        root,
        registry,
        workstream_id,
        Revision::try_from(expected_revision).ok(),
    ) {
        Ok(revision) => ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Applied {
                revision: revision.value(),
            },
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(_) => rejected("park outcome needs recovery"),
    }
}

fn apply_archive(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: i64,
) -> ResponseEnvelope {
    match crate::actions::archive(
        root,
        registry,
        workstream_id,
        Revision::try_from(expected_revision).expect("protocol validates positive revisions"),
    ) {
        Ok(revision) => ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Applied {
                revision: revision.value(),
            },
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(_) => rejected("archive is unavailable"),
    }
}

fn apply_restore(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: i64,
) -> ResponseEnvelope {
    match crate::actions::restore(
        registry,
        workstream_id,
        Revision::try_from(expected_revision).expect("protocol validates positive revisions"),
    ) {
        Ok(revision) => ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Applied {
                revision: revision.value(),
            },
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(_) => rejected("restore is unavailable"),
    }
}

fn apply_rename(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: i64,
    name: &str,
) -> ResponseEnvelope {
    match crate::actions::rename(
        registry,
        workstream_id,
        Revision::try_from(expected_revision).expect("protocol validates positive revisions"),
        name,
    ) {
        Ok(()) => ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Applied {
                revision: expected_revision,
            },
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(crate::actions::ActionError::UnsupportedProvider(provider)) => {
            rejected(&format!("{provider} provider rename is unavailable"))
        }
        Err(_) => rejected("rename is unavailable"),
    }
}

fn apply_start(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: i64,
) -> ResponseEnvelope {
    match crate::actions::start(
        root,
        registry,
        workstream_id,
        Revision::try_from(expected_revision).ok(),
    ) {
        Ok(_) => match workstream_revision(registry, workstream_id) {
            Ok(revision) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Applied {
                    revision: revision.value(),
                },
            },
            Err(_) => rejected("start outcome needs recovery"),
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(crate::actions::ActionError::ProviderReadiness(error)) => provider_rejected(error),
        Err(crate::actions::ActionError::UnsupportedProvider(provider)) => {
            rejected(&format!("{provider} provider action is unavailable"))
        }
        Err(crate::actions::ActionError::ProviderRecoveryUnavailable(provider)) => {
            rejected(&format!("{provider} provider recovery is unavailable"))
        }
        Err(_) => rejected("remote start is unavailable"),
    }
}

fn apply_recover(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    expected_revision: i64,
) -> ResponseEnvelope {
    match crate::actions::recover(
        root,
        registry,
        workstream_id,
        Revision::try_from(expected_revision).ok(),
    ) {
        Ok(_) => match workstream_revision(registry, workstream_id) {
            Ok(revision) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Applied {
                    revision: revision.value(),
                },
            },
            Err(_) => rejected("native recovery outcome is unavailable"),
        },
        Err(crate::actions::ActionError::WorkstreamRevisionConflict) => {
            rejected("revision conflict; refresh this host")
        }
        Err(crate::actions::ActionError::NativeRecoveryUnavailable) => {
            rejected("workstream is not awaiting native recovery")
        }
        Err(crate::actions::ActionError::RuntimeProbeAmbiguous) => {
            rejected("native recovery probe is ambiguous")
        }
        Err(crate::actions::ActionError::ProviderRecoveryUnavailable(provider)) => {
            rejected(&format!("{provider} provider recovery is unavailable"))
        }
        Err(_) => rejected("native recovery is unavailable"),
    }
}

fn workstream_revision(
    registry: &HostRegistry,
    workstream_id: WorkstreamId,
) -> Result<Revision, StateError> {
    registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == workstream_id)
        .map(|overview| overview.revision)
        .ok_or(StateError::UnknownOpenWorkstream(workstream_id))
}

/// Projects durable lifecycle state without contacting private provider tmux
/// servers. Exact liveness checks remain at recovery and stateful action
/// boundaries, so remote snapshot polling cannot disturb an attached provider.
fn snapshot(
    registry: &mut HostRegistry,
    cursor: Option<u32>,
) -> Result<SnapshotResponse, StateError> {
    crate::repository::refresh_pending_metadata(registry)?;
    let page = registry.workstream_overview_page(
        cursor.unwrap_or(0),
        crate::protocol::SNAPSHOT_PAGE_WORKSTREAMS,
    )?;
    let workstreams = page.workstreams.iter().map(snapshot_workstream).collect();
    let unresolved_operation_count = registry
        .unresolved_operation_overviews()?
        .len()
        .try_into()
        .map_err(|_| StateError::NavigatorSnapshotTooLarge)?;
    Ok(SnapshotResponse {
        workstreams,
        unresolved_operation_count,
        observer_status: observer_status(registry)?,
        provider_capabilities: crate::provider::discover_capabilities(registry)?,
        next_cursor: page.next_cursor,
    })
}

fn observer_status(registry: &HostRegistry) -> Result<ObserverStatus, StateError> {
    Ok(
        match registry
            .codex_integration()?
            .map(|integration| integration.lifecycle)
        {
            None => ObserverStatus::NotInstalled,
            Some(IntegrationLifecycle::TrustPending) => ObserverStatus::TrustPending,
            Some(IntegrationLifecycle::Ready) => ObserverStatus::Ready,
            Some(IntegrationLifecycle::Modified) => ObserverStatus::Modified,
            Some(IntegrationLifecycle::Disabled) => ObserverStatus::Disabled,
        },
    )
}

fn operations(registry: &HostRegistry) -> Result<OperationsResponse, StateError> {
    let operations = registry
        .unresolved_operation_overviews()?
        .into_iter()
        .map(|operation| OperationSnapshot {
            operation_id: operation.operation_id,
            kind: operation.kind,
            source_workstream_id: operation.source_workstream_id,
            phase: operation.phase,
            revision: operation.revision.value(),
        })
        .collect();
    Ok(OperationsResponse { operations })
}

fn snapshot_workstream(overview: &WorkstreamOverview) -> SnapshotWorkstream {
    let runtime_status = observed_runtime_status(overview);
    let attention = overview.attention.as_ref();
    let recovery_required = overview.lifecycle == WorkstreamLifecycle::RecoveryRequired
        || attention
            .and_then(|attention| attention.recovery_unseen_since_revision)
            .is_some();
    SnapshotWorkstream {
        workstream_id: overview.workstream_id,
        location_id: overview.location_id,
        provider: overview.provider,
        project_display_name: bounded_display_name(&overview.project_display_name),
        repository_fingerprint: overview.remote_identity_fingerprint.clone(),
        remote_identity_display: overview
            .remote_identity_display
            .as_deref()
            .map(bounded_display_name),
        display_name: bounded_display_name(&display_name(overview, runtime_status)),
        runtime_id: overview.runtime.as_ref().map(|runtime| runtime.runtime_id),
        runtime_status: if recovery_required {
            RuntimeStatus::Unknown
        } else {
            runtime_status
        },
        lifecycle: overview.lifecycle,
        archived: overview.archived_at_millis.is_some(),
        result_ready: attention
            .and_then(|attention| attention.result_unseen_since_revision)
            .is_some(),
        recovery_required,
        attention_revision: attention.map(|attention| attention.revision.value()),
        activity_sequence: overview.last_activity_sequence,
        last_activity_at_millis: overview.last_activity_at_millis,
        revision: overview.revision.value(),
    }
}

fn observed_runtime_status(overview: &WorkstreamOverview) -> RuntimeStatus {
    if overview.lifecycle == WorkstreamLifecycle::Parked {
        return RuntimeStatus::Stopped;
    }
    let Some(record) = &overview.runtime else {
        return RuntimeStatus::Stopped;
    };
    if record.status == RuntimeStatus::Stopped {
        return RuntimeStatus::Stopped;
    }
    record.status
}

fn display_name(overview: &WorkstreamOverview, runtime_status: RuntimeStatus) -> String {
    let Some(binding) = &overview.binding else {
        return if runtime_status == RuntimeStatus::Starting {
            "starting".to_owned()
        } else {
            "untitled".to_owned()
        };
    };
    let context = if binding.start_source == "clear" {
        NameContext::Cutover {
            prior_effective_name: binding.predecessor_effective_name.as_deref(),
        }
    } else if runtime_status == RuntimeStatus::Starting {
        NameContext::Starting
    } else {
        NameContext::Normal
    };
    resolve_name(
        binding.name_state,
        binding.observed_thread_name.as_deref(),
        binding.observed_thread_name.as_deref(),
        context,
    )
    .text
}

fn bounded_display_name(value: &str) -> String {
    const MAX_BYTES: usize = 256;
    if value.len() <= MAX_BYTES {
        return value.to_owned();
    }
    let mut bounded = String::with_capacity(MAX_BYTES);
    for character in value.chars() {
        if bounded.len() + character.len_utf8() + '…'.len_utf8() > MAX_BYTES {
            break;
        }
        bounded.push(character);
    }
    bounded.push('…');
    bounded
}

fn local_capabilities() -> Capabilities {
    Capabilities {
        git: crate::provider::command_available("git"),
        tmux: crate::provider::command_available("tmux"),
    }
}

fn read_frame(input: &mut impl Read) -> Result<Vec<u8>, RemoteError> {
    let mut frame = Vec::with_capacity(MAX_FRAME_BYTES.min(4096));
    let mut buffer = [0_u8; 4096];
    let mut oversized = false;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let available = MAX_FRAME_BYTES.saturating_sub(frame.len());
        let stored = available.min(read);
        frame.extend_from_slice(&buffer[..stored]);
        oversized |= stored != read;
    }
    if oversized {
        return Err(RemoteError::FrameTooLarge);
    }
    Ok(frame)
}

fn rejection_for_error(error: &RemoteError) -> ResponseEnvelope {
    match error {
        RemoteError::FrameTooLarge => rejected("protocol frame exceeds its maximum size"),
        RemoteError::Protocol(crate::protocol::ProtocolError::UnsupportedVersion(_)) => {
            rejected("unsupported protocol version")
        }
        RemoteError::Io(_) | RemoteError::Protocol(_) => rejected("malformed protocol request"),
        RemoteError::State(_)
        | RemoteError::Action(_)
        | RemoteError::Runtime(_)
        | RemoteError::UnknownRuntime
        | RemoteError::RuntimeUnavailable
        | RemoteError::AttachFailed
        | RemoteError::PresentationUnavailable => rejected("host request is unavailable"),
    }
}

fn rejected(message: &str) -> ResponseEnvelope {
    ResponseEnvelope::rejected(message.to_owned())
        .expect("fixed protocol rejection diagnostics are bounded")
}

fn provider_rejected(error: crate::provider::ProviderReadinessError) -> ResponseEnvelope {
    let reason = match error.reason {
        crate::protocol::ProviderCapabilityReason::None => "unavailable",
        crate::protocol::ProviderCapabilityReason::AdapterUnavailable => "adapter unavailable",
        crate::protocol::ProviderCapabilityReason::NotInstalled => "not installed",
        crate::protocol::ProviderCapabilityReason::UnsupportedVersion => "unsupported version",
        crate::protocol::ProviderCapabilityReason::ObserverNotReady => "observer not ready",
        crate::protocol::ProviderCapabilityReason::RuntimePrerequisiteMissing => {
            "runtime prerequisite missing"
        }
        crate::protocol::ProviderCapabilityReason::ProbeFailed => "probe failed",
    };
    rejected(&format!("{} provider unavailable: {reason}", error.kind))
}

fn default_state_root() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".wsnav-state"))
        .join("wsnav")
}

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Protocol(#[from] crate::protocol::ProtocolError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Action(#[from] crate::actions::ActionError),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error("protocol frame exceeds its maximum size")]
    FrameTooLarge,
    #[error("runtime is unknown")]
    UnknownRuntime,
    #[error("runtime is not live")]
    RuntimeUnavailable,
    #[error("native tmux attach failed")]
    AttachFailed,
    #[error("remote presentation state is unavailable")]
    PresentationUnavailable,
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::{io::Cursor, process::Command};

    use crate::protocol::{ProviderCapability, ProviderCapabilityReason, ProviderCapabilityStatus};

    use super::*;

    fn unavailable(
        provider: crate::domain::ProviderKind,
    ) -> crate::provider::ProviderReadinessError {
        crate::provider::ProviderReadinessError {
            kind: provider,
            status: ProviderCapabilityStatus::Unavailable,
            reason: ProviderCapabilityReason::AdapterUnavailable,
        }
    }

    #[test]
    fn malformed_or_oversized_input_is_drained_then_rejected_in_one_frame() {
        let temporary = tempfile::tempdir().unwrap();
        let input = vec![b'x'; MAX_FRAME_BYTES + 100];
        let mut output = Vec::new();

        serve(
            Some(temporary.path().join("state")),
            &mut Cursor::new(input),
            &mut output,
        )
        .unwrap();

        let response = ResponseEnvelope::decode(&output).unwrap();
        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "protocol frame exceeds its maximum size"
        ));
    }

    #[test]
    fn hello_and_snapshot_expose_only_bounded_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("state");
        let hello = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Hello {
                client_alias: "client".to_owned(),
            },
        }
        .encode()
        .unwrap();
        let mut output = Vec::new();

        serve(Some(root.clone()), &mut Cursor::new(hello), &mut output).unwrap();

        assert!(matches!(
            ResponseEnvelope::decode(&output).unwrap().response,
            HostResponse::Hello(HelloResponse { .. })
        ));

        let snapshot = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Snapshot { cursor: None },
        }
        .encode()
        .unwrap();
        output.clear();
        serve(Some(root), &mut Cursor::new(snapshot), &mut output).unwrap();
        let response = ResponseEnvelope::decode(&output).unwrap();
        assert!(matches!(
            response.response,
            HostResponse::Snapshot(SnapshotResponse {
                ref workstreams,
                observer_status: ObserverStatus::NotInstalled,
                ..
            }) if workstreams.is_empty()
        ));
    }

    #[test]
    fn codex_capability_is_ready_only_with_install_runtime_and_observer_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        registry
            .record_codex_integration(
                crate::provider::codex::profile::ProfileOwnership {
                    canonical_path: PathBuf::from("/tmp/wsnav-observer.json"),
                    owner_id: "owner".to_owned(),
                    profile_schema_version: 2,
                    hook_executable: PathBuf::from("/tmp/wsnav"),
                    content_hash: "hash".to_owned(),
                },
                IntegrationLifecycle::Ready,
            )
            .unwrap();

        let capabilities = crate::provider::discover_capabilities_with(&registry, |program| {
            matches!(program, "codex" | "tmux")
        })
        .unwrap();
        let codex = capabilities
            .iter()
            .find(|capability| capability.kind == crate::domain::ProviderKind::Codex)
            .unwrap();
        assert_eq!(codex.status, ProviderCapabilityStatus::Available);
        assert_eq!(codex.reason, ProviderCapabilityReason::None);
        assert!(codex.is_new_eligible());
        assert_eq!(
            capabilities[1],
            ProviderCapability {
                kind: crate::domain::ProviderKind::OpenCode,
                status: ProviderCapabilityStatus::Unavailable,
                reason: ProviderCapabilityReason::AdapterUnavailable,
                fresh_launch: false,
                exact_resume: false,
                observe: false,
                metadata_read: false,
                rename: false,
                fork: false,
            }
        );
    }

    #[test]
    fn codex_capability_reports_conservative_unready_reasons() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let registry = HostRegistry::open(&root).unwrap();

        let missing =
            crate::provider::discover_capabilities_with(&registry, |program| program == "tmux")
                .unwrap()
                .remove(0);
        assert_eq!(missing.reason, ProviderCapabilityReason::NotInstalled);

        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let no_tmux =
            crate::provider::discover_capabilities_with(&registry, |program| program == "codex")
                .unwrap()
                .remove(0);
        assert_eq!(
            no_tmux.reason,
            ProviderCapabilityReason::RuntimePrerequisiteMissing
        );

        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let no_observer = crate::provider::discover_capabilities_with(&registry, |_| true)
            .unwrap()
            .remove(0);
        assert_eq!(
            no_observer.reason,
            ProviderCapabilityReason::ObserverNotReady
        );
    }

    #[test]
    fn opencode_registration_rejects_before_recording_a_workstream() {
        let temporary = tempfile::tempdir().unwrap();
        let checkout = temporary.path().join("checkout");
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(&checkout)
                .status()
                .unwrap()
                .success()
        );

        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let before = registry.workstream_overviews().unwrap().len();

        let response = apply_register_checkout_with(
            &mut registry,
            &checkout,
            crate::domain::ProviderKind::OpenCode,
            |_, provider| Err(unavailable(provider)),
        );

        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "opencode provider unavailable: adapter unavailable"
        ));
        assert_eq!(registry.workstream_overviews().unwrap().len(), before);
    }

    #[test]
    fn stale_codex_registration_evidence_rejects_before_recording_a_workstream() {
        let temporary = tempfile::tempdir().unwrap();
        let checkout = temporary.path().join("checkout");
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(&checkout)
                .status()
                .unwrap()
                .success()
        );

        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let before = registry.workstream_overviews().unwrap().len();

        let response = apply_register_checkout_with(
            &mut registry,
            &checkout,
            crate::domain::ProviderKind::Codex,
            |_, provider| Err(unavailable(provider)),
        );

        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "codex provider unavailable: adapter unavailable"
        ));
        assert_eq!(registry.workstream_overviews().unwrap().len(), before);
    }

    #[test]
    fn opencode_registration_persists_the_authoritative_provider_after_readiness() {
        let temporary = tempfile::tempdir().unwrap();
        let checkout = temporary.path().join("checkout");
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(&checkout)
                .status()
                .unwrap()
                .success()
        );

        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let response = apply_register_checkout_with(
            &mut registry,
            &checkout,
            crate::domain::ProviderKind::OpenCode,
            |_, provider| {
                assert_eq!(provider, crate::domain::ProviderKind::OpenCode);
                Ok(())
            },
        );

        let workstream_id = match response.response {
            HostResponse::WorkstreamCreated {
                workstream_id,
                provider,
                revision,
            } => {
                assert_eq!(provider, crate::domain::ProviderKind::OpenCode);
                assert_eq!(revision, Revision::INITIAL.value());
                workstream_id
            }
            other => panic!("expected OpenCode WorkstreamCreated response, got {other:?}"),
        };
        let overview = registry
            .workstream_overviews()
            .unwrap()
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .unwrap();
        assert_eq!(overview.provider, crate::domain::ProviderKind::OpenCode);
        assert_eq!(overview.revision, Revision::INITIAL);
    }

    #[test]
    fn opencode_new_rejects_before_recording_or_launching_a_destination() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let source = registry
            .register_project_root(&project, crate::domain::ProviderKind::Codex)
            .unwrap();
        let before = registry.workstream_overviews().unwrap().len();

        let response = apply_new_workstream_with(
            &root,
            &mut registry,
            source.workstream_id,
            Revision::INITIAL.value(),
            "opencode-new",
            crate::domain::ProviderKind::OpenCode,
            |_, provider| Err(unavailable(provider)),
            |_, _, _, _, _| Ok(crate::actions::StartOutcome::Started),
        );

        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "opencode provider unavailable: adapter unavailable"
        ));
        assert_eq!(registry.workstream_overviews().unwrap().len(), before);
        assert!(
            registry
                .runtime_for_workstream(source.workstream_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn opencode_new_returns_the_exact_destination_without_a_codex_effect() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let source = registry
            .register_project_root(&project, crate::domain::ProviderKind::Codex)
            .unwrap();
        let starter_provider = Cell::new(None);
        let starter_revision = Cell::new(None);
        let starter_destination = Cell::new(None);

        let response = apply_new_workstream_with(
            &root,
            &mut registry,
            source.workstream_id,
            Revision::INITIAL.value(),
            "opencode-positive-new",
            crate::domain::ProviderKind::OpenCode,
            |_, provider| {
                assert_eq!(provider, crate::domain::ProviderKind::OpenCode);
                Ok(())
            },
            |_root, registry, workstream_id, expected_revision, provider| {
                starter_provider.set(Some(provider));
                starter_revision.set(expected_revision);
                starter_destination.set(Some(workstream_id));
                let destination = registry
                    .workstream_overviews()
                    .unwrap()
                    .into_iter()
                    .find(|overview| overview.workstream_id == workstream_id)
                    .unwrap();
                assert_eq!(destination.provider, crate::domain::ProviderKind::OpenCode);
                assert_eq!(expected_revision, Some(destination.revision));
                Ok(crate::actions::StartOutcome::Started)
            },
        );
        let destination_id = match response.response {
            HostResponse::WorkstreamCreated {
                workstream_id,
                provider,
                revision,
            } => {
                assert_eq!(provider, crate::domain::ProviderKind::OpenCode);
                assert_eq!(revision, Revision::INITIAL.value());
                workstream_id
            }
            other => panic!("expected OpenCode WorkstreamCreated response, got {other:?}"),
        };

        assert_eq!(
            starter_provider.get(),
            Some(crate::domain::ProviderKind::OpenCode)
        );
        assert_eq!(starter_revision.get(), Some(Revision::INITIAL));
        assert_eq!(starter_destination.get(), Some(destination_id));
        let overviews = registry.workstream_overviews().unwrap();
        assert_eq!(overviews.len(), 2);
        let source_overview = overviews
            .iter()
            .find(|overview| overview.workstream_id == source.workstream_id)
            .unwrap();
        assert_eq!(source_overview.provider, crate::domain::ProviderKind::Codex);
        assert_eq!(source_overview.revision, Revision::INITIAL);
        let destination = overviews
            .iter()
            .find(|overview| overview.workstream_id == destination_id)
            .unwrap();
        assert_eq!(destination.provider, crate::domain::ProviderKind::OpenCode);
        assert_eq!(destination.project_repository_path, project);
        assert!(destination.runtime.is_none());

        let replay = apply_new_workstream_with(
            &root,
            &mut registry,
            source.workstream_id,
            Revision::INITIAL.value(),
            "opencode-positive-new",
            crate::domain::ProviderKind::OpenCode,
            |_, provider| {
                assert_eq!(provider, crate::domain::ProviderKind::OpenCode);
                Ok(())
            },
            |_root, _registry, _workstream_id, _expected_revision, provider| {
                assert_eq!(provider, crate::domain::ProviderKind::OpenCode);
                Ok(crate::actions::StartOutcome::Started)
            },
        );
        assert!(matches!(
            replay.response,
            HostResponse::WorkstreamCreated {
                workstream_id,
                provider: crate::domain::ProviderKind::OpenCode,
                revision: 1,
            } if workstream_id == destination_id
        ));
        assert_eq!(registry.workstream_overviews().unwrap().len(), 2);
        let connection =
            rusqlite::Connection::open(temporary.path().join("state/host.sqlite")).unwrap();
        let request = connection
            .query_row(
                "SELECT source_workstream_id, source_revision, workstream_id
                 FROM independent_creation_requests WHERE request_key = ?1",
                ["opencode-positive-new"],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(request.0, source.workstream_id.to_string());
        assert_eq!(request.1, Revision::INITIAL.value());
        assert_eq!(request.2, destination_id.to_string());
    }

    #[test]
    fn stale_codex_new_evidence_rejects_before_recording_a_destination() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let source = registry
            .register_project_root(&project, crate::domain::ProviderKind::Codex)
            .unwrap();
        let before = registry.workstream_overviews().unwrap().len();

        let response = apply_new_workstream_with(
            &root,
            &mut registry,
            source.workstream_id,
            Revision::INITIAL.value(),
            "stale-codex-new",
            crate::domain::ProviderKind::Codex,
            |_, provider| Err(unavailable(provider)),
            |_, _, _, _, _| Ok(crate::actions::StartOutcome::Started),
        );

        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "codex provider unavailable: adapter unavailable"
        ));
        assert_eq!(registry.workstream_overviews().unwrap().len(), before);
        assert!(
            registry
                .runtime_for_workstream(source.workstream_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn opencode_rename_and_nonrecovering_actions_are_bounded_no_effect_refusals() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let source = registry
            .register_project_root(&project, crate::domain::ProviderKind::OpenCode)
            .unwrap();
        let before = registry.workstream_overviews().unwrap();

        let rename = apply_rename(
            &mut registry,
            source.workstream_id,
            Revision::INITIAL.value(),
            "unavailable-name",
        );
        assert!(matches!(
            rename.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "opencode provider rename is unavailable"
        ));

        let recovery = apply_recover(
            &root,
            &mut registry,
            source.workstream_id,
            Revision::INITIAL.value(),
        );
        assert!(matches!(
            recovery.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "opencode provider recovery is unavailable"
        ));
        assert_eq!(registry.workstream_overviews().unwrap(), before);
    }

    #[test]
    fn lost_opencode_fork_response_has_an_actionable_bounded_diagnostic() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let registry = HostRegistry::open(&root).unwrap();
        let response = apply_created_workstream(
            &Err(crate::actions::ActionError::OpenCodeForkExternalEffectUnknown),
            &registry,
        );

        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "OpenCode Fork response was lost; an unmanaged provider session may need native inspection or cleanup; use a new explicit Fork to try again"
        ));
    }

    #[test]
    fn exact_observer_removal_rejects_without_leaking_or_corrupting_the_frame() {
        let temporary = tempfile::tempdir().unwrap();
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::RemoveObserver,
            },
        }
        .encode()
        .unwrap();
        let mut output = Vec::new();

        serve(
            Some(temporary.path().join("state")),
            &mut Cursor::new(request),
            &mut output,
        )
        .unwrap();

        let response = ResponseEnvelope::decode(&output).unwrap();
        assert!(matches!(
            response.response,
            HostResponse::Rejected { ref diagnostic }
                if diagnostic == "observer removal is unavailable"
        ));
        assert!(!String::from_utf8_lossy(&output).contains("observer integration removed"));
    }

    #[test]
    fn operation_listing_exposes_no_request_key_or_effect_plan() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let operation_id = {
            let mut registry = HostRegistry::open(&root).unwrap();
            let (prepared, _) = registry
                .create_or_get_operation(
                    "private-request-key".to_owned(),
                    crate::domain::OperationKind::Fork,
                    "{}".to_owned(),
                )
                .unwrap();
            registry
                .transition_operation(
                    prepared.id,
                    prepared.revision,
                    crate::domain::OperationPhase::ExternalEffectStarted,
                    None,
                    None,
                )
                .unwrap();
            registry
                .transition_operation(
                    prepared.id,
                    prepared.revision.next(),
                    crate::domain::OperationPhase::RecoveryRequired,
                    None,
                    None,
                )
                .unwrap();
            prepared.id
        };
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Operations,
        }
        .encode()
        .unwrap();
        let mut output = Vec::new();

        serve(
            Some(root.base().to_path_buf()),
            &mut Cursor::new(request),
            &mut output,
        )
        .unwrap();

        let text = String::from_utf8(output.clone()).unwrap();
        assert!(!text.contains("private-request-key"));
        assert!(!text.contains("/private/repository"));
        assert!(matches!(
            ResponseEnvelope::decode(&output).unwrap().response,
            HostResponse::Operations(OperationsResponse { operations })
                if operations.len() == 1 && operations[0].operation_id == operation_id
        ));
    }

    #[test]
    fn future_host_schema_gets_a_safe_manual_upgrade_diagnostic() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        HostRegistry::open(&root).unwrap();
        let connection = rusqlite::Connection::open(root.host_database_path()).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 99;")
            .unwrap();
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Hello {
                client_alias: "client".to_owned(),
            },
        }
        .encode()
        .unwrap();
        let mut output = Vec::new();

        serve(
            Some(root.base().to_path_buf()),
            &mut Cursor::new(request),
            &mut output,
        )
        .unwrap();

        assert!(matches!(
            ResponseEnvelope::decode(&output).unwrap().response,
            HostResponse::Rejected { diagnostic }
                if diagnostic == "host state schema requires a matching wsnav update"
        ));
    }

    #[test]
    fn native_names_are_truncated_before_they_can_overflow_a_protocol_frame() {
        let name = "multi-byte name ".repeat(100);

        let bounded = bounded_display_name(&name);

        assert!(bounded.len() <= 256);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn managed_remote_workstream_keeps_the_project_location_label() {
        let overview = WorkstreamOverview {
            workstream_id: WorkstreamId::new(),
            location_id: crate::domain::LocationId::new(),
            provider: crate::domain::ProviderKind::Codex,
            project_repository_path: PathBuf::from("/private/place/dms-power-status"),
            project_display_name: "dms-power-status".to_owned(),
            remote_identity_fingerprint: Some(format!("git-remote-v1:{}", "a".repeat(64))),
            remote_identity_display: Some("github.com/owner/dms-power-status".to_owned()),
            lifecycle: WorkstreamLifecycle::Open,
            archived_at_millis: Some(1_234),
            last_activity_sequence: 1,
            last_activity_at_millis: None,
            revision: Revision::INITIAL,
            runtime: None,
            binding: None,
            attention: None,
        };

        let snapshot = snapshot_workstream(&overview);
        assert_eq!(snapshot.project_display_name, "dms-power-status");
        assert_eq!(
            snapshot.remote_identity_display.as_deref(),
            Some("github.com/owner/dms-power-status")
        );
        assert!(snapshot.archived);
    }

    #[test]
    fn presentation_root_is_canonicalized_and_must_match_runtime_cwd() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let equivalent = project.join("child").join("..");
        std::fs::create_dir(project.join("child")).unwrap();

        let resolved = resolve_presentation_root(&project, &project).unwrap();
        assert_eq!(resolved, project.canonicalize().unwrap());
        assert!(matches!(
            resolve_presentation_root(&equivalent, &project),
            Err(RemoteError::PresentationUnavailable)
        ));

        let other = temporary.path().join("other");
        std::fs::create_dir(&other).unwrap();
        assert!(matches!(
            resolve_presentation_root(&project, &other),
            Err(RemoteError::PresentationUnavailable)
        ));
        assert!(matches!(
            resolve_presentation_root(Path::new("relative"), &project),
            Err(RemoteError::PresentationUnavailable)
        ));
    }

    #[test]
    fn presentation_shell_plan_requires_an_executable_absolute_shell() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let shell = std::env::current_exe().unwrap();

        let plan = presentation_shell_plan(&project, &project, &shell).unwrap();
        assert_eq!(plan.cwd, project.canonicalize().unwrap());
        assert_eq!(plan.shell, shell);
        assert_eq!(plan.arguments, vec![OsString::from("-i")]);
        assert!(
            plan.arguments
                .iter()
                .all(|argument| argument != project.as_os_str())
        );
        assert!(matches!(
            presentation_shell_plan(&project, &project, Path::new("sh")),
            Err(RemoteError::PresentationUnavailable)
        ));
        assert!(matches!(
            presentation_shell_plan(&project, &project, Path::new("/tmp")),
            Err(RemoteError::PresentationUnavailable)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let non_executable = project.join("shell");
            std::fs::write(&non_executable, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o600))
                .unwrap();
            assert!(matches!(
                presentation_shell_plan(&project, &project, &non_executable),
                Err(RemoteError::PresentationUnavailable)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn presentation_shell_uses_the_effective_accounts_login_shell() {
        use nix::unistd::{User, geteuid};

        let expected = User::from_uid(geteuid()).unwrap().unwrap().shell;
        assert_eq!(current_account_shell().unwrap(), expected);
    }

    fn presentation_overview(
        workstream_id: WorkstreamId,
        project_root: &Path,
        lifecycle: WorkstreamLifecycle,
        archived: bool,
    ) -> WorkstreamOverview {
        WorkstreamOverview {
            workstream_id,
            location_id: crate::domain::LocationId::new(),
            provider: crate::domain::ProviderKind::Codex,
            project_repository_path: project_root.to_path_buf(),
            project_display_name: "project".to_owned(),
            remote_identity_fingerprint: None,
            remote_identity_display: None,
            lifecycle,
            archived_at_millis: archived.then_some(1),
            last_activity_sequence: 0,
            last_activity_at_millis: None,
            revision: Revision::INITIAL,
            runtime: None,
            binding: None,
            attention: None,
        }
    }

    fn presentation_runtime(
        workstream_id: WorkstreamId,
        cwd: &Path,
        status: RuntimeStatus,
    ) -> crate::state::RuntimeRecord {
        crate::state::RuntimeRecord {
            runtime_id: RuntimeId::new(),
            workstream_id,
            provider: crate::domain::ProviderKind::Codex,
            tmux_generation: "generation".to_owned(),
            tmux_session: "wsnav-runtime".to_owned(),
            cwd: cwd.to_path_buf(),
            provider_pid: Some(1),
            process_birth: Some("birth".to_owned()),
            status,
            revision: Revision::INITIAL,
        }
    }

    #[test]
    fn presentation_state_plan_rejects_unknown_archived_closed_mismatched_and_nonlive_state() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let unknown = WorkstreamId::new();
        assert!(matches!(
            preflight_presentation(&root, unknown),
            Err(RemoteError::PresentationUnavailable)
        ));

        let project = temporary.path().join("project");
        let other = temporary.path().join("other");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(&other).unwrap();
        let workstream_id = WorkstreamId::new();
        let live = presentation_runtime(workstream_id, &project, RuntimeStatus::Idle);
        let archived =
            presentation_overview(workstream_id, &project, WorkstreamLifecycle::Open, true);
        assert!(matches!(
            validate_presentation_state(&archived, Some(&live)),
            Err(RemoteError::PresentationUnavailable)
        ));
        let closed =
            presentation_overview(workstream_id, &project, WorkstreamLifecycle::Parked, false);
        assert!(matches!(
            validate_presentation_state(&closed, Some(&live)),
            Err(RemoteError::PresentationUnavailable)
        ));
        let mismatch = presentation_runtime(workstream_id, &other, RuntimeStatus::Idle);
        let open = presentation_overview(workstream_id, &project, WorkstreamLifecycle::Open, false);
        assert!(matches!(
            validate_presentation_state(&open, Some(&mismatch)),
            Err(RemoteError::PresentationUnavailable)
        ));
        assert!(matches!(
            validate_presentation_state(&open, None),
            Err(RemoteError::PresentationUnavailable)
        ));
        let starting = presentation_runtime(workstream_id, &project, RuntimeStatus::Starting);
        assert!(validate_presentation_state(&open, Some(&starting)).is_ok());
        for status in [
            RuntimeStatus::Stopped,
            RuntimeStatus::Unknown,
            RuntimeStatus::Unreachable,
        ] {
            let unavailable = presentation_runtime(workstream_id, &project, status);
            assert!(matches!(
                validate_presentation_state(&open, Some(&unavailable)),
                Err(RemoteError::PresentationUnavailable)
            ));
        }
        assert!(validate_presentation_state(&open, Some(&live)).is_ok());
    }

    #[test]
    fn remote_snapshot_projects_durable_runtime_state_without_a_tmux_probe() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_project_root(&project, crate::domain::ProviderKind::Codex)
            .unwrap();
        let runtime = registry.reserve_runtime(registered.workstream_id).unwrap();
        registry
            .record_runtime_process_identity(runtime.runtime_id, runtime.revision, 42, "birth-a")
            .unwrap();

        let snapshot = snapshot(&mut registry, None).unwrap();

        assert_eq!(snapshot.workstreams.len(), 1);
        assert_eq!(
            snapshot.workstreams[0].runtime_status,
            RuntimeStatus::Starting
        );
    }

    #[test]
    fn revision_guarded_acknowledgement_uses_the_host_transaction() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut registry = HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_project_root(
                Path::new("/disposable/repository"),
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();
        let workstream_id = registered.workstream_id;
        let attention = registry
            .mark_result_attention(
                workstream_id,
                crate::domain::ProviderSessionId::codex("session").unwrap(),
                "turn".to_owned(),
            )
            .unwrap();
        let request = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::AcknowledgeAttention {
                    workstream_id,
                    expected_revision: attention.revision.value(),
                },
            },
        }
        .encode()
        .unwrap();
        let mut output = Vec::new();

        serve(
            Some(root.base().to_path_buf()),
            &mut Cursor::new(request),
            &mut output,
        )
        .unwrap();

        assert!(matches!(
            ResponseEnvelope::decode(&output).unwrap().response,
            HostResponse::Applied { revision } if revision == attention.revision.next().value()
        ));
        assert_eq!(
            HostRegistry::open(&root)
                .unwrap()
                .attention(workstream_id)
                .unwrap()
                .unwrap()
                .result_unseen_since_revision,
            None
        );
    }

    #[test]
    fn archive_and_restore_are_revision_guarded_remote_visibility_actions() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let registered = HostRegistry::open(&root)
            .unwrap()
            .register_external_workstream(
                PathBuf::from("/private/repository"),
                "common-dir".to_owned(),
                "deadbeef".to_owned(),
            )
            .unwrap();
        let archive = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::Archive {
                    workstream_id: registered.workstream_id,
                    expected_revision: Revision::INITIAL.value(),
                },
            },
        }
        .encode()
        .unwrap();
        let mut archive_output = Vec::new();
        serve(
            Some(root.base().to_path_buf()),
            &mut Cursor::new(archive),
            &mut archive_output,
        )
        .unwrap();
        let archived_revision = match ResponseEnvelope::decode(&archive_output).unwrap().response {
            HostResponse::Applied { revision } => revision,
            response => panic!("unexpected archive response: {response:?}"),
        };

        let snapshot = snapshot(&mut HostRegistry::open(&root).unwrap(), None).unwrap();
        assert_eq!(snapshot.workstreams.len(), 1);
        assert!(snapshot.workstreams[0].archived);

        let restore = RequestEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            request: HostRequest::Apply {
                action: HostAction::Restore {
                    workstream_id: registered.workstream_id,
                    expected_revision: archived_revision,
                },
            },
        }
        .encode()
        .unwrap();
        let mut restore_output = Vec::new();
        serve(
            Some(root.base().to_path_buf()),
            &mut Cursor::new(restore),
            &mut restore_output,
        )
        .unwrap();
        assert!(matches!(
            ResponseEnvelope::decode(&restore_output).unwrap().response,
            HostResponse::Applied { revision } if revision == archived_revision + 1
        ));
        let restored = HostRegistry::open(&root)
            .unwrap()
            .workstream_overviews()
            .unwrap();
        assert_eq!(restored[0].archived_at_millis, None);
        assert!(restored[0].runtime.is_none());
    }
}
