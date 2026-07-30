//! Host-side implementation of the bounded `WSNav` control protocol.
//!
//! This module is invoked by the hidden `_remote` command, locally or through
//! SSH. It intentionally owns no listener: each request is one short-lived
//! process with exactly one JSON request and response frame.

use std::{
    io::{self, Read, Write},
    process::{Command, Stdio},
};

use thiserror::Error;

use crate::{
    domain::{Revision, RuntimeId, RuntimeStatus, WorkstreamLifecycle},
    protocol::{
        CURRENT_PROTOCOL_VERSION, Capabilities, HelloResponse, HostAction, HostRequest,
        HostResponse, MAX_FRAME_BYTES, RequestEnvelope, ResponseEnvelope, SnapshotResponse,
        SnapshotWorkstream,
    },
    provider::codex::names::{NameContext, resolve_name},
    runtime::{LinuxProcessProbe, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux},
    state::{HostRegistry, StateError, StateRoot, WorkstreamOverview},
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
    let registry = HostRegistry::open(root)?;
    let runtime_record = registry
        .runtime_by_id(runtime_id)?
        .ok_or(RemoteError::UnknownRuntime)?;
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), runtime_record.runtime_id),
    );
    if !matches!(runtime.probe()?, RuntimeProbe::Live { .. }) {
        return Err(RemoteError::RuntimeUnavailable);
    }
    let status = runtime.attach_command().status()?;
    if status.success() {
        Ok(())
    } else {
        Err(RemoteError::AttachFailed)
    }
}

fn dispatch(state_root: Option<std::path::PathBuf>, request: &RequestEnvelope) -> ResponseEnvelope {
    let Ok(root) = StateRoot::create(state_root.unwrap_or_else(default_state_root)) else {
        return rejected("host state is unavailable");
    };
    let Ok(mut registry) = HostRegistry::open(&root) else {
        return rejected("host state is unavailable");
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
        HostRequest::Snapshot => match snapshot(&root, &registry) {
            Ok(snapshot) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Snapshot(snapshot),
            },
            Err(_) => rejected("host snapshot is unavailable"),
        },
        HostRequest::Attach { runtime_id } => match registry.runtime_by_id(*runtime_id) {
            Ok(Some(_)) => ResponseEnvelope {
                version: CURRENT_PROTOCOL_VERSION,
                response: HostResponse::Attach {
                    runtime_id: *runtime_id,
                },
            },
            Ok(None) | Err(_) => rejected("runtime is unavailable"),
        },
        HostRequest::Apply { action } => apply(&root, &mut registry, *action),
    }
}

fn apply(root: &StateRoot, registry: &mut HostRegistry, action: HostAction) -> ResponseEnvelope {
    match action {
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
        HostAction::Start { .. } => {
            // Start crosses native process ownership and is added with its
            // recovery implementation in the next D3 slice. Returning a
            // protocol rejection is intentional: the client must not guess
            // that an unsupported action succeeded.
            rejected("remote start is not available yet")
        }
    }
}

fn apply_park(
    root: &StateRoot,
    registry: &mut HostRegistry,
    workstream_id: crate::domain::WorkstreamId,
    expected_revision: i64,
) -> ResponseEnvelope {
    let Ok(overview) = registry.workstream_overviews().and_then(|workstreams| {
        workstreams
            .into_iter()
            .find(|overview| overview.workstream_id == workstream_id)
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))
    }) else {
        return rejected("workstream is unavailable");
    };
    if overview.revision.value() != expected_revision {
        return rejected("revision conflict; refresh this host");
    }
    let Some(runtime_record) = overview.runtime else {
        return rejected("workstream has no live runtime");
    };
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), runtime_record.runtime_id),
    );
    match runtime.park() {
        Ok(()) => {}
        Err(_) => return rejected("runtime park failed"),
    }
    match registry.park_runtime(runtime_record.runtime_id, runtime_record.revision) {
        Ok(()) => ResponseEnvelope {
            version: CURRENT_PROTOCOL_VERSION,
            response: HostResponse::Applied {
                revision: runtime_record.revision.next().value(),
            },
        },
        Err(_) => rejected("park outcome needs recovery"),
    }
}

fn snapshot(root: &StateRoot, registry: &HostRegistry) -> Result<SnapshotResponse, StateError> {
    let workstreams = registry
        .workstream_overviews()?
        .iter()
        .map(|overview| snapshot_workstream(root, overview))
        .collect();
    Ok(SnapshotResponse { workstreams })
}

fn snapshot_workstream(root: &StateRoot, overview: &WorkstreamOverview) -> SnapshotWorkstream {
    let runtime_status = observed_runtime_status(root, overview);
    let attention = overview.attention.as_ref();
    let recovery_required = overview.lifecycle == WorkstreamLifecycle::RecoveryRequired
        || attention
            .and_then(|attention| attention.recovery_unseen_since_revision)
            .is_some();
    SnapshotWorkstream {
        workstream_id: overview.workstream_id,
        location_id: overview.location_id,
        display_name: display_name(overview, runtime_status),
        runtime_id: overview.runtime.as_ref().map(|runtime| runtime.runtime_id),
        runtime_status: if recovery_required {
            RuntimeStatus::Unknown
        } else {
            runtime_status
        },
        lifecycle: overview.lifecycle,
        result_ready: attention
            .and_then(|attention| attention.result_unseen_since_revision)
            .is_some(),
        recovery_required,
        attention_revision: attention.map(|attention| attention.revision.value()),
        activity_sequence: overview.last_activity_sequence,
        revision: overview.revision.value(),
    }
}

fn observed_runtime_status(root: &StateRoot, overview: &WorkstreamOverview) -> RuntimeStatus {
    if overview.lifecycle == WorkstreamLifecycle::Parked {
        return RuntimeStatus::Stopped;
    }
    let Some(record) = &overview.runtime else {
        return RuntimeStatus::Stopped;
    };
    if record.status == RuntimeStatus::Stopped {
        return RuntimeStatus::Stopped;
    }
    let tmux = SystemTmux::default();
    let process_probe = LinuxProcessProbe;
    let runtime = PrivateRuntime::new(
        &tmux,
        &process_probe,
        RuntimePaths::for_runtime(root.base(), record.runtime_id),
    );
    match runtime.probe() {
        Ok(RuntimeProbe::Live { .. }) => record.status,
        Ok(RuntimeProbe::Missing | RuntimeProbe::Unknown { .. }) | Err(_) => RuntimeStatus::Unknown,
    }
}

fn display_name(overview: &WorkstreamOverview, runtime_status: RuntimeStatus) -> String {
    let Some(binding) = &overview.binding else {
        return if runtime_status == RuntimeStatus::Starting {
            format!("starting · {}", overview.workstream_id.short())
        } else {
            format!("untitled · {}", overview.workstream_id.short())
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
        &overview.workstream_id.short(),
    )
    .text
}

fn local_capabilities() -> Capabilities {
    Capabilities {
        codex: command_available("codex"),
        git: command_available("git"),
        tmux: command_available("tmux"),
    }
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
        | RemoteError::Runtime(_)
        | RemoteError::UnknownRuntime
        | RemoteError::RuntimeUnavailable
        | RemoteError::AttachFailed => rejected("host request is unavailable"),
    }
}

fn rejected(message: &str) -> ResponseEnvelope {
    ResponseEnvelope::rejected(message.to_owned())
        .expect("fixed protocol rejection diagnostics are bounded")
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
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error("protocol frame exceeds its maximum size")]
    FrameTooLarge,
    #[error("runtime is unknown")]
    UnknownRuntime,
    #[error("runtime is not live")]
    RuntimeUnavailable,
    #[error("native tmux attach failed")]
    AttachFailed,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

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
            request: HostRequest::Snapshot,
        }
        .encode()
        .unwrap();
        output.clear();
        serve(Some(root), &mut Cursor::new(snapshot), &mut output).unwrap();
        let response = ResponseEnvelope::decode(&output).unwrap();
        assert!(matches!(
            response.response,
            HostResponse::Snapshot(SnapshotResponse { ref workstreams }) if workstreams.is_empty()
        ));
    }
}
