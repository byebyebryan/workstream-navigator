//! The disconnected `OpenCode` lifecycle observer.
//!
//! This module owns the long-lived HTTP/SSE supervision loop.  The CLI only
//! parses its hidden arguments and delegates one typed context here; no
//! provider payload or observer diagnostic is written to the native pane.

use std::{
    collections::VecDeque,
    io::Write,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    domain::{ProviderSessionId, RuntimeId, RuntimeStatus},
    runtime::{LinuxProcessProbe, ProcessProbe},
    state::{
        D16State, HostRegistry, ObserverDatabaseDeadline, ObserverHandoverActivationAck,
        ObserverProcessIdentity, OpenCodeLifecycleObservation, OpenCodeRuntimeHandle,
        RuntimeRecord, StateError, StateRoot, open_d17_current_only, open_observer_transition,
        write_observer_handover_activation_ack,
    },
};

use super::{
    LOOPBACK_HOST, LifecycleHint, OpenCodeClient, OpenCodeEndpoint, OpenCodeError, OpenCodeEvent,
    OpenCodeEventStream, OpenCodeSessionStatus, endpoint_owned_by_process,
};

const HEALTH_DEADLINE: Duration = Duration::from_secs(15);
const SUPERVISION_INTERVAL: Duration = Duration::from_millis(500);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(500);
const STATUS_FAILURE_LIMIT: u8 = 4;
const RECONNECT_LIMIT: u8 = 4;
const OBSERVER_DATABASE_BUDGET: Duration = Duration::from_millis(750);
const STANDBY_READY_TIMEOUT: Duration = Duration::from_secs(15);
const STANDBY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const STANDBY_MAX_EVENTS: usize = 256;
const STANDBY_MAX_EVENT_METADATA_BYTES: usize = 64 * 1024;
const STANDBY_READY_LINE_MAX_BYTES: usize = 512;
const STANDBY_ACTIVATION_ACK_LINE_MAX_BYTES: usize = 512;

/// The explicit process-generation marker carried by a hidden observer
/// entrypoint. A pre-D16 argv marker is recognized only by the cutover process
/// authority; this binary never launches or mutates through that legacy path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenCodeObserverMode {
    D16,
    /// D17's observer can open only the explicit schema-14 current boundary.
    /// It is still started solely by the dormant presentation controller.
    D17,
    D16Standby,
}

impl OpenCodeObserverMode {
    #[must_use]
    pub const fn command_name(self) -> &'static str {
        match self {
            Self::D16 => "_opencode_observer_d16",
            Self::D17 => "_opencode_observer_d17",
            Self::D16Standby => "_opencode_observer_standby",
        }
    }
}

/// Typed hidden-command arguments after the CLI has parsed and validated IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeObserverContext {
    pub runtime_id: RuntimeId,
    pub generation: String,
    pub endpoint: OpenCodeEndpoint,
    pub session: ProviderSessionId,
    pub pane_pid: u32,
    pub cwd: PathBuf,
    pub provider_birth: String,
    /// The exact provider release fingerprint observed by the state owner.
    /// Standby receives this out of band because it cannot open host state
    /// before activation.
    pub provider_version: Option<String>,
    pub mode: OpenCodeObserverMode,
}

/// A bounded observer failure.  Ambiguous process/handle evidence is never
/// converted into a lifecycle mutation.
#[derive(Debug, Error)]
pub enum OpenCodeObserverError {
    #[error("private runtime probe is ambiguous")]
    RuntimeProbeAmbiguous,
    #[error(transparent)]
    OpenCode(#[from] OpenCodeError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("standby observer activation is unavailable on this platform")]
    StandbyActivationUnavailable,
    #[error("standby observer readiness handshake failed")]
    StandbyReadinessFailed,
    #[error("standby observer event buffer is full")]
    StandbyBufferFull,
}

/// The only state operations an active observer may request. Every active
/// observer mutation below maps to the typed schema-12/13 transition
/// authority; pre-D16 evidence is handled only by the cutover process adapter.
trait ObserverAuthority {
    fn runtime_by_id(&mut self, runtime_id: RuntimeId)
    -> Result<Option<RuntimeRecord>, StateError>;
    fn opencode_runtime_handle(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<Option<OpenCodeRuntimeHandle>, StateError>;
    fn mark_ready(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: crate::domain::Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError>;
    fn mark_unknown(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: crate::domain::Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError>;
    fn apply_observation(
        &mut self,
        runtime_id: RuntimeId,
        observation: &OpenCodeLifecycleObservation,
    ) -> Result<(), StateError>;
}

impl ObserverAuthority for D16State {
    fn runtime_by_id(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        D16State::observer_runtime_by_id(self, runtime_id)
    }

    fn opencode_runtime_handle(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<Option<OpenCodeRuntimeHandle>, StateError> {
        D16State::observer_opencode_runtime_handle(self, runtime_id)
    }

    fn mark_ready(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: crate::domain::Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError> {
        D16State::observer_mark_opencode_ready(
            self,
            runtime_id,
            generation,
            expected_revision,
            observer_pid,
            observer_birth,
            ObserverDatabaseDeadline::from_now(OBSERVER_DATABASE_BUDGET),
        )
        .map(|_| ())
    }

    fn mark_unknown(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: crate::domain::Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError> {
        D16State::observer_mark_opencode_unknown(
            self,
            runtime_id,
            generation,
            expected_revision,
            observer_pid,
            observer_birth,
            ObserverDatabaseDeadline::from_now(OBSERVER_DATABASE_BUDGET),
        )
    }

    fn apply_observation(
        &mut self,
        runtime_id: RuntimeId,
        observation: &OpenCodeLifecycleObservation,
    ) -> Result<(), StateError> {
        D16State::observer_apply_opencode_lifecycle_observation(
            self,
            runtime_id,
            observation,
            ObserverDatabaseDeadline::from_now(OBSERVER_DATABASE_BUDGET),
        )
        .map(|_| ())
    }
}

impl ObserverAuthority for HostRegistry {
    fn runtime_by_id(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        self.observer_runtime_by_id(runtime_id)
    }

    fn opencode_runtime_handle(
        &mut self,
        runtime_id: RuntimeId,
    ) -> Result<Option<OpenCodeRuntimeHandle>, StateError> {
        self.observer_opencode_runtime_handle(runtime_id)
    }

    fn mark_ready(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: crate::domain::Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError> {
        self.mark_opencode_observer_ready(
            runtime_id,
            generation,
            expected_revision,
            observer_pid,
            observer_birth,
        )
        .map(|_| ())
    }

    fn mark_unknown(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        expected_revision: crate::domain::Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError> {
        self.mark_opencode_observer_unknown_exact(
            runtime_id,
            generation,
            expected_revision,
            observer_pid,
            observer_birth,
        )
    }

    fn apply_observation(
        &mut self,
        runtime_id: RuntimeId,
        observation: &OpenCodeLifecycleObservation,
    ) -> Result<(), StateError> {
        self.apply_opencode_lifecycle_observation(runtime_id, observation)
            .map(|_| ())
    }
}

/// Runs one exact `OpenCode` observer until its Runtime disappears or a
/// fail-closed ownership/identity check fails.
///
/// # Errors
///
/// Returns a bounded provider, state, or ownership error when corroboration
/// cannot be completed safely.
pub fn run_observer(
    root: &StateRoot,
    context: &OpenCodeObserverContext,
) -> Result<(), OpenCodeObserverError> {
    if !observer_context_valid(context) {
        return Err(OpenCodeObserverError::RuntimeProbeAmbiguous);
    }
    match context.mode {
        OpenCodeObserverMode::D16 => {
            let mut state = open_observer_transition(root)?;
            run_observer_with_authority(&mut state, context)
        }
        OpenCodeObserverMode::D17 => {
            let state = open_d17_current_only(root)?;
            let mut registry = state.into_d17_host_registry()?;
            run_observer_with_authority(&mut registry, context)
        }
        OpenCodeObserverMode::D16Standby => {
            Err(OpenCodeObserverError::StandbyActivationUnavailable)
        }
    }
}

fn run_observer_with_authority<A: ObserverAuthority>(
    authority: &mut A,
    context: &OpenCodeObserverContext,
) -> Result<(), OpenCodeObserverError> {
    let Some(record) = authority.runtime_by_id(context.runtime_id)? else {
        return Ok(());
    };
    if !observer_target_matches(&record, context) {
        return Ok(());
    }
    let pane_birth = record
        .process_birth
        .as_deref()
        .ok_or(OpenCodeObserverError::RuntimeProbeAmbiguous)?;
    let Some(handle) = authority.opencode_runtime_handle(context.runtime_id)? else {
        return Ok(());
    };
    if handle.runtime_generation != context.generation
        || handle.endpoint_port != context.endpoint.port
        || handle.native_session_id != context.session
    {
        mark_unknown(authority, context);
        return Ok(());
    }

    let client = OpenCodeClient::new(context.endpoint.clone());
    let deadline = Instant::now() + HEALTH_DEADLINE;
    loop {
        // Ownership is the first and final gate around provider observation.
        // When /proc evidence is ambiguous, do not query an endpoint that
        // could have been rebound; the existing bounded deadline remains the
        // only retry budget.
        if let Some(stream) = startup_readiness_step(
            || endpoint_owned_by_process(&context.endpoint, context.pane_pid, pane_birth),
            || {
                client
                    .health()
                    .is_ok_and(|health| health.version == handle.version)
                    && matches!(
                        client.session_status_with_root(&context.session, &context.cwd),
                        Ok(OpenCodeSessionStatus::Busy | OpenCodeSessionStatus::Idle)
                    )
            },
            || client.event_stream().ok(),
        ) {
            // `ready` is an action boundary, not merely process liveness. A
            // caller may submit native input as soon as start returns, so the
            // exact SSE stream must already be established before the handle
            // becomes actionable or the first turn can race past observation.
            let (observer_pid, observer_birth) = prepare(authority, context)?;
            let evidence = SupervisionEvidence {
                context,
                pane_birth,
                observer_pid,
                observer_birth: &observer_birth,
                provider_version: &handle.version,
            };
            return supervise(authority, &client, &evidence, Some(stream));
        }
        if Instant::now() >= deadline {
            mark_unknown(authority, context);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Orders one bounded startup readiness attempt. Exact endpoint ownership is
/// required before any provider HTTP request and rechecked immediately before
/// opening SSE. An ambiguous first sample therefore permits only a retry.
fn startup_readiness_step<Endpoint, Provider, Stream, T>(
    mut endpoint_owned: Endpoint,
    mut provider_ready: Provider,
    mut open_stream: Stream,
) -> Option<T>
where
    Endpoint: FnMut() -> bool,
    Provider: FnMut() -> bool,
    Stream: FnMut() -> Option<T>,
{
    if !endpoint_owned() || !provider_ready() || !endpoint_owned() {
        return None;
    }
    open_stream()
}

/// A process-local, content-free event buffer used while an observer is in
/// standby.  It retains only the already-parsed lifecycle envelope and
/// bounded assistant message identifier; raw SSE bytes are never retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StandbyEventBuffer {
    events: VecDeque<OpenCodeEvent>,
    metadata_bytes: usize,
}

impl StandbyEventBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            metadata_bytes: 0,
        }
    }

    /// Adds one parsed event without exceeding either in-memory bound.
    ///
    /// # Errors
    ///
    /// Returns [`OpenCodeObserverError::StandbyBufferFull`] when retaining the
    /// event would exceed the bounded entry or metadata budget.
    pub fn push(&mut self, event: OpenCodeEvent) -> Result<(), OpenCodeObserverError> {
        let metadata_bytes = event
            .candidate_message_id
            .as_deref()
            .map_or(0, str::len)
            .saturating_add(1);
        if self.events.len() >= STANDBY_MAX_EVENTS
            || self.metadata_bytes.saturating_add(metadata_bytes) > STANDBY_MAX_EVENT_METADATA_BYTES
        {
            return Err(OpenCodeObserverError::StandbyBufferFull);
        }
        self.metadata_bytes = self.metadata_bytes.saturating_add(metadata_bytes);
        self.events.push_back(event);
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn into_events(self) -> VecDeque<OpenCodeEvent> {
        self.events
    }

    /// Drops only the ordered prefix ending at the last buffered settlement
    /// already reflected by the frozen predecessor's exact binding. Events
    /// after that boundary remain eligible for replay after the handle CAS.
    fn reconcile_committed_prefix(mut self, last_settled_turn_id: Option<&str>) -> Self {
        let Some(last_settled_turn_id) = last_settled_turn_id else {
            return self;
        };
        let mut candidate_message_id = None;
        let mut committed_boundary = None;
        for (index, event) in self.events.iter().enumerate() {
            record_candidate(&mut candidate_message_id, event);
            if let Some(LifecycleHint::Settled { message_id }) = &event.hint {
                let settled = message_id.as_deref().or(candidate_message_id.as_deref());
                if settled == Some(last_settled_turn_id) {
                    committed_boundary = Some(index);
                }
                candidate_message_id = None;
            }
        }
        if let Some(committed_boundary) = committed_boundary {
            self.events.drain(..=committed_boundary);
            self.metadata_bytes = self
                .events
                .iter()
                .map(|event| {
                    event
                        .candidate_message_id
                        .as_deref()
                        .map_or(0, str::len)
                        .saturating_add(1)
                })
                .sum();
        }
        self
    }
}

impl Default for StandbyEventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs a D16 standby observer without opening or mutating host state.
///
/// The command establishes the exact endpoint health/status contract and
/// opens the global SSE stream before publishing a bounded readiness line.
/// Parsed events remain process-local until an exact `SIGUSR1` activation is
/// received. Activation then revalidates the assigned PID/birth/generation in
/// state, emits one bounded private acknowledgement, and only then enters
/// active supervision.
///
/// # Errors
///
/// Returns a bounded readiness, provider, activation, or observer-transition
/// error when the exact endpoint or post-CAS state assignment cannot be
/// established.
pub fn run_standby(
    root: &std::path::Path,
    context: &OpenCodeObserverContext,
) -> Result<(), OpenCodeObserverError> {
    if context.mode != OpenCodeObserverMode::D16Standby {
        return Err(OpenCodeObserverError::StandbyActivationUnavailable);
    }
    let expected_version = context
        .provider_version
        .as_deref()
        .ok_or(OpenCodeObserverError::StandbyReadinessFailed)?;
    if !observer_context_valid(context) || !bounded_standby_token(expected_version) {
        return Err(OpenCodeObserverError::StandbyReadinessFailed);
    }
    let observer_pid = std::process::id();
    let observer_birth = LinuxProcessProbe
        .process_birth(observer_pid)
        .ok_or(OpenCodeObserverError::StandbyReadinessFailed)?;
    let activation = activation_channel()?;
    let client = OpenCodeClient::new(context.endpoint.clone());
    let deadline = Instant::now() + STANDBY_READY_TIMEOUT;
    let mut stream = None;
    while Instant::now() < deadline {
        if activation.try_recv().is_ok() {
            // A signal before the endpoint is ready cannot authorize state
            // writes.  The parent must establish the stream first and retry
            // with a fresh exact standby.
            return Err(OpenCodeObserverError::StandbyReadinessFailed);
        }
        let health_matches = client
            .health()
            .is_ok_and(|health| health.version == expected_version);
        let status_matches = client
            .session_status_with_root(&context.session, &context.cwd)
            .is_ok_and(|status| {
                matches!(
                    status,
                    OpenCodeSessionStatus::Busy | OpenCodeSessionStatus::Idle
                )
            });
        if health_matches
            && status_matches
            && endpoint_owned_by_process(
                &context.endpoint,
                context.pane_pid,
                &context.provider_birth,
            )
            && let Ok(ready_stream) = client.event_stream()
        {
            stream = Some(ready_stream);
            break;
        }
        thread::sleep(STANDBY_POLL_INTERVAL);
    }
    let Some(mut stream) = stream else {
        return Err(OpenCodeObserverError::StandbyReadinessFailed);
    };
    publish_standby_ready(observer_pid, &observer_birth)?;

    let mut buffer = StandbyEventBuffer::new();
    loop {
        match activation.try_recv() {
            Ok(()) => {
                return activate_standby(
                    root,
                    context,
                    observer_pid,
                    &observer_birth,
                    buffer,
                    &client,
                    stream,
                );
            }
            Err(TryRecvError::Disconnected) => {
                return Err(OpenCodeObserverError::StandbyActivationUnavailable);
            }
            Err(TryRecvError::Empty) => {}
        }
        match stream.next_data() {
            Ok(Some(data)) => {
                match super::parse_event_strict(&data, &context.session, Some(&context.cwd)) {
                    Ok(Some(event)) => buffer.push(event)?,
                    Ok(None) => {}
                    Err(_) => return Err(OpenCodeObserverError::StandbyReadinessFailed),
                }
            }
            Err(OpenCodeError::IdleTimeout) => {}
            Ok(None) | Err(_) => return Err(OpenCodeObserverError::StandbyReadinessFailed),
        }
    }
}

fn bounded_standby_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.contains(['\0', '\n', '\r'])
}

fn observer_context_valid(context: &OpenCodeObserverContext) -> bool {
    context.session.provider() == crate::domain::ProviderKind::OpenCode
        && context.endpoint.host == LOOPBACK_HOST
        && context.endpoint.port != 0
        && context.pane_pid != 0
        && bounded_standby_token(&context.generation)
        && bounded_standby_token(&context.provider_birth)
        && context.cwd.is_absolute()
        && context
            .cwd
            .to_str()
            .is_some_and(|cwd| !cwd.is_empty() && cwd.len() <= 4096)
}

fn publish_standby_ready(pid: u32, birth: &str) -> Result<(), OpenCodeObserverError> {
    let line = format!("READY {pid} {birth}\n");
    if line.len() > STANDBY_READY_LINE_MAX_BYTES {
        return Err(OpenCodeObserverError::StandbyReadinessFailed);
    }
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|_| OpenCodeObserverError::StandbyReadinessFailed)
}

fn publish_standby_activation_ack(
    root: &std::path::Path,
    context: &OpenCodeObserverContext,
    observer_pid: u32,
    observer_birth: &str,
    revision: crate::domain::Revision,
) -> Result<(), OpenCodeObserverError> {
    let executable = std::env::current_exe()
        .map_err(|_| OpenCodeObserverError::StandbyReadinessFailed)?
        .into_os_string()
        .into_string()
        .map_err(|_| OpenCodeObserverError::StandbyReadinessFailed)?;
    write_observer_handover_activation_ack(
        root,
        &ObserverHandoverActivationAck {
            version: 1,
            runtime_id: context.runtime_id.to_string(),
            runtime_generation: context.generation.clone(),
            standby_observer: ObserverProcessIdentity {
                pid: observer_pid,
                birth: observer_birth.to_owned(),
                executable,
            },
            handle_revision: revision,
        },
    )?;
    let line = standby_activation_ack_line(context, observer_pid, observer_birth, revision)?;
    let mut stdout = std::io::stdout().lock();
    // The durable private acknowledgement is authoritative. Stdout is only a
    // latency optimization for the launcher that originally created this
    // standby and may have disappeared before recovery.
    let _ = stdout.write_all(&line).and_then(|()| stdout.flush());
    Ok(())
}

fn standby_activation_ack_line(
    context: &OpenCodeObserverContext,
    observer_pid: u32,
    observer_birth: &str,
    revision: crate::domain::Revision,
) -> Result<Vec<u8>, OpenCodeObserverError> {
    if observer_pid == 0
        || !bounded_standby_token(observer_birth)
        || !bounded_standby_token(&context.generation)
    {
        return Err(OpenCodeObserverError::StandbyReadinessFailed);
    }
    let line = format!(
        "ACTIVATED {observer_pid} {observer_birth} {} {} {}\n",
        context.runtime_id,
        revision.value(),
        context.generation,
    );
    if line.len() > STANDBY_ACTIVATION_ACK_LINE_MAX_BYTES {
        return Err(OpenCodeObserverError::StandbyReadinessFailed);
    }
    Ok(line.into_bytes())
}

fn activation_channel() -> Result<Receiver<()>, OpenCodeObserverError> {
    #[cfg(unix)]
    {
        let mut signals = nix::sys::signal::SigSet::empty();
        signals.add(nix::sys::signal::Signal::SIGUSR1);
        signals
            .thread_block()
            .map_err(|_| OpenCodeObserverError::StandbyActivationUnavailable)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            if signals.wait().is_ok() {
                let _ = sender.send(());
            }
        });
        Ok(receiver)
    }
    #[cfg(not(unix))]
    {
        Err(OpenCodeObserverError::StandbyActivationUnavailable)
    }
}

fn activate_standby(
    root: &std::path::Path,
    context: &OpenCodeObserverContext,
    observer_pid: u32,
    observer_birth: &str,
    buffer: StandbyEventBuffer,
    client: &OpenCodeClient,
    stream: OpenCodeEventStream,
) -> Result<(), OpenCodeObserverError> {
    let state_root = StateRoot::select(root);
    let mut state = open_observer_transition(&state_root)?;
    let handle = state
        .observer_opencode_runtime_handle(context.runtime_id)?
        .ok_or(OpenCodeObserverError::StandbyReadinessFailed)?;
    // This check is the process-side CAS barrier.  A signal sent too early,
    // or a stale/reused standby, has no state mutation authority.
    if handle.runtime_generation != context.generation
        || handle.endpoint_port != context.endpoint.port
        || handle.native_session_id != context.session
        || handle.observer_pid != Some(observer_pid)
        || handle.observer_birth.as_deref() != Some(observer_birth)
        || handle.observer_status != crate::state::OpenCodeObserverStatus::Ready
        || context
            .provider_version
            .as_deref()
            .is_some_and(|version| handle.version != version)
    {
        return Err(OpenCodeObserverError::StandbyReadinessFailed);
    }
    let evidence = SupervisionEvidence {
        context,
        pane_birth: &context.provider_birth,
        observer_pid,
        observer_birth,
        provider_version: &handle.version,
    };
    let last_settled_turn_id = state.observer_last_settled_turn_id(
        context.runtime_id,
        &context.generation,
        &context.session,
    )?;
    let buffer = buffer.reconcile_committed_prefix(last_settled_turn_id.as_deref());
    let mut candidate_message_id = None;
    for event in buffer.into_events() {
        let observer_evidence = ObserverEvidence {
            context,
            observer_pid,
            observer_birth,
        };
        if !apply_event(
            &mut state,
            &observer_evidence,
            &event,
            &mut candidate_message_id,
            client,
        )? {
            return Ok(());
        }
    }
    publish_standby_activation_ack(root, context, observer_pid, observer_birth, handle.revision)?;
    supervise(&mut state, client, &evidence, Some(stream))
}

fn observer_target_matches(
    record: &crate::state::RuntimeRecord,
    context: &OpenCodeObserverContext,
) -> bool {
    record.provider == crate::domain::ProviderKind::OpenCode
        && record.tmux_generation == context.generation
        && record.cwd == context.cwd
        && record.provider_pid == Some(context.pane_pid)
        && record.process_birth.as_deref() == Some(context.provider_birth.as_str())
}

fn prepare<A: ObserverAuthority>(
    authority: &mut A,
    context: &OpenCodeObserverContext,
) -> Result<(u32, String), OpenCodeObserverError> {
    let observer_pid = std::process::id();
    let observer_birth = LinuxProcessProbe
        .process_birth(observer_pid)
        .ok_or(OpenCodeObserverError::RuntimeProbeAmbiguous)?;
    let current_handle = authority
        .opencode_runtime_handle(context.runtime_id)?
        .ok_or(OpenCodeObserverError::RuntimeProbeAmbiguous)?;
    authority.mark_ready(
        context.runtime_id,
        &context.generation,
        current_handle.revision,
        observer_pid,
        &observer_birth,
    )?;
    // D17 deliberately retains `starting` until its presentation controller
    // commits the onboarding proof after this exact Ready handle.  D16 has no
    // such journal phase and preserves its existing immediate Started hint.
    if context.mode == OpenCodeObserverMode::D16
        && let Some(current) = authority.runtime_by_id(context.runtime_id)?
        && current.status == RuntimeStatus::Starting
    {
        let evidence = ObserverEvidence {
            context,
            observer_pid,
            observer_birth: &observer_birth,
        };
        if let Err(error) = apply_hint(authority, &evidence, LifecycleHint::Started) {
            mark_unknown(authority, context);
            return Err(error);
        }
    }
    Ok((observer_pid, observer_birth))
}

struct SupervisionEvidence<'a> {
    context: &'a OpenCodeObserverContext,
    pane_birth: &'a str,
    observer_pid: u32,
    observer_birth: &'a str,
    provider_version: &'a str,
}

fn supervise<A: ObserverAuthority>(
    authority: &mut A,
    client: &OpenCodeClient,
    supervision: &SupervisionEvidence<'_>,
    mut stream: Option<OpenCodeEventStream>,
) -> Result<(), OpenCodeObserverError> {
    let context = supervision.context;
    let mut reconnect_failures = 0_u8;
    let mut candidate_message_id = None;
    let mut last_status_poll = Instant::now();
    let mut last_supervision = Instant::now();
    let mut status_failures = 0_u8;
    loop {
        let Some(current) = authority.runtime_by_id(context.runtime_id)? else {
            return Ok(());
        };
        if current.tmux_generation != context.generation
            || current.provider_pid != Some(context.pane_pid)
            || current.process_birth.as_deref() != Some(context.provider_birth.as_str())
        {
            mark_unknown(authority, context);
            return Ok(());
        }

        // The event stream can deliver many updates per second. Health and
        // endpoint ownership remain fail-closed supervision evidence, but do
        // not need to contend with every provider event. Initial readiness
        // checks above already proved both before this loop became live.
        if supervision_due(last_supervision, Instant::now()) {
            if !client
                .health()
                .is_ok_and(|health| health.version == supervision.provider_version)
                || !endpoint_owned_by_process(
                    client.endpoint(),
                    context.pane_pid,
                    supervision.pane_birth,
                )
            {
                mark_unknown(authority, context);
                return Ok(());
            }
            last_supervision = Instant::now();
        }

        // Polling is corroboration, never a source of a message ID. It also
        // keeps a Ready observer from surviving an endpoint that no longer
        // proves the exact root session identity.
        if last_status_poll.elapsed() >= STATUS_POLL_INTERVAL {
            if !poll_supervision_status(
                authority,
                supervision,
                &mut candidate_message_id,
                client,
                &mut status_failures,
            )? {
                return Ok(());
            }
            last_status_poll = Instant::now();
        }

        if stream.is_none() {
            match client.event_stream() {
                Ok(new_stream) => stream = Some(new_stream),
                Err(_) => reconnect_failures = reconnect_failures.saturating_add(1),
            }
        }
        if let Some(event_stream) = stream.as_mut() {
            match event_stream.next_data() {
                Ok(Some(data)) => {
                    match super::parse_event_strict(&data, &context.session, Some(&context.cwd)) {
                        Ok(Some(event)) => {
                            let evidence = ObserverEvidence {
                                context,
                                observer_pid: supervision.observer_pid,
                                observer_birth: supervision.observer_birth,
                            };
                            match apply_event(
                                authority,
                                &evidence,
                                &event,
                                &mut candidate_message_id,
                                client,
                            ) {
                                Ok(true) => {}
                                Ok(false) => return Ok(()),
                                Err(error) => {
                                    mark_unknown(authority, context);
                                    return Err(error);
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            mark_unknown(authority, context);
                            return Ok(());
                        }
                    }
                    reconnect_failures = 0;
                }
                Err(OpenCodeError::IdleTimeout) => {}
                Ok(None) | Err(_) => {
                    stream = None;
                    reconnect_failures = reconnect_failures.saturating_add(1);
                }
            }
        }
        if reconnect_failures >= RECONNECT_LIMIT {
            mark_unknown(authority, context);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(
            100_u64.saturating_mul(1_u64 << reconnect_failures),
        ));
    }
}

fn poll_supervision_status<A: ObserverAuthority>(
    authority: &mut A,
    supervision: &SupervisionEvidence<'_>,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
    status_failures: &mut u8,
) -> Result<bool, OpenCodeObserverError> {
    let context = supervision.context;
    let evidence = ObserverEvidence {
        context,
        observer_pid: supervision.observer_pid,
        observer_birth: supervision.observer_birth,
    };
    match poll_root_status(
        authority,
        &evidence,
        candidate_message_id,
        client,
        status_failures,
    ) {
        Ok(true) => Ok(true),
        Ok(false) => {
            mark_unknown(authority, context);
            Ok(false)
        }
        Err(error) => {
            mark_unknown(authority, context);
            Err(error)
        }
    }
}

fn poll_root_status<A: ObserverAuthority>(
    authority: &mut A,
    evidence: &ObserverEvidence<'_>,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
    status_failures: &mut u8,
) -> Result<bool, OpenCodeObserverError> {
    let Ok(status) =
        client.session_status_with_root(&evidence.context.session, &evidence.context.cwd)
    else {
        *status_failures = status_failures.saturating_add(1);
        return Ok(*status_failures < STATUS_FAILURE_LIMIT);
    };
    match status {
        OpenCodeSessionStatus::Busy => {
            *status_failures = 0;
            let should_apply = authority
                .runtime_by_id(evidence.context.runtime_id)?
                .is_some_and(|runtime| runtime.status != RuntimeStatus::Working);
            if should_apply {
                apply_hint(authority, evidence, LifecycleHint::Working)?;
            }
        }
        OpenCodeSessionStatus::Idle => {
            *status_failures = 0;
            if candidate_message_id.is_some() {
                settle_if_idle(authority, evidence, candidate_message_id, client)
                    .map_err(OpenCodeObserverError::OpenCode)?;
            }
        }
        OpenCodeSessionStatus::Unknown => {
            *status_failures = status_failures.saturating_add(1);
        }
    }
    Ok(*status_failures < STATUS_FAILURE_LIMIT)
}

fn supervision_due(last_supervision: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_supervision) >= SUPERVISION_INTERVAL
}

struct ObserverEvidence<'a> {
    context: &'a OpenCodeObserverContext,
    observer_pid: u32,
    observer_birth: &'a str,
}

fn apply_event<A: ObserverAuthority>(
    authority: &mut A,
    evidence: &ObserverEvidence<'_>,
    event: &OpenCodeEvent,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
) -> Result<bool, OpenCodeObserverError> {
    record_candidate(candidate_message_id, event);
    match &event.hint {
        Some(LifecycleHint::Working) => {
            // An incomplete message update may trail the provider's completed
            // assistant update and idle transition. SSE is evidence, not
            // mutation authority: require the exact root session to still be
            // busy before moving an idle/attention Runtime back to working.
            // The regular status poll remains the bounded fallback if the
            // event arrives before the status map changes.
            let runtime_status = authority
                .runtime_by_id(evidence.context.runtime_id)?
                .map(|runtime| runtime.status);
            if should_apply_working_event(runtime_status, || {
                client
                    .session_status_with_root(&evidence.context.session, &evidence.context.cwd)
                    .ok()
            }) {
                apply_hint(authority, evidence, LifecycleHint::Working)?;
            }
        }
        Some(LifecycleHint::Settled { .. }) if candidate_message_id.is_some() => {
            settle_if_idle(authority, evidence, candidate_message_id, client)?;
        }
        Some(LifecycleHint::Started) => apply_hint(authority, evidence, LifecycleHint::Started)?,
        Some(LifecycleHint::Ended) => {
            apply_hint(authority, evidence, LifecycleHint::Ended)?;
            return Ok(false);
        }
        Some(LifecycleHint::Settled { .. }) | None => {}
    }
    Ok(true)
}

fn should_apply_working_event(
    runtime_status: Option<RuntimeStatus>,
    exact_status: impl FnOnce() -> Option<OpenCodeSessionStatus>,
) -> bool {
    match runtime_status {
        Some(RuntimeStatus::Working) | None => false,
        Some(_) => exact_status().is_some_and(|status| status == OpenCodeSessionStatus::Busy),
    }
}

fn record_candidate(candidate_message_id: &mut Option<String>, event: &OpenCodeEvent) {
    if let Some(candidate) = event.candidate_message_id.clone() {
        *candidate_message_id = Some(candidate);
    }
    if event.clears_candidate {
        *candidate_message_id = None;
    }
}

fn settle_if_idle<A: ObserverAuthority>(
    authority: &mut A,
    evidence: &ObserverEvidence<'_>,
    candidate_message_id: &mut Option<String>,
    client: &OpenCodeClient,
) -> Result<(), OpenCodeError> {
    let status =
        client.session_status_with_root(&evidence.context.session, &evidence.context.cwd)?;
    match status {
        OpenCodeSessionStatus::Idle => {
            let Some(message_id) = candidate_message_id.take() else {
                return Ok(());
            };
            apply_hint(
                authority,
                evidence,
                LifecycleHint::Settled {
                    message_id: Some(message_id),
                },
            )
            .map_err(|error| match error {
                OpenCodeObserverError::OpenCode(error) => error,
                OpenCodeObserverError::State(_)
                | OpenCodeObserverError::RuntimeProbeAmbiguous
                | OpenCodeObserverError::StandbyActivationUnavailable
                | OpenCodeObserverError::StandbyReadinessFailed
                | OpenCodeObserverError::StandbyBufferFull => OpenCodeError::MalformedResponse,
            })?;
        }
        OpenCodeSessionStatus::Busy => *candidate_message_id = None,
        OpenCodeSessionStatus::Unknown => {}
    }
    Ok(())
}

fn apply_hint<A: ObserverAuthority>(
    authority: &mut A,
    evidence: &ObserverEvidence<'_>,
    hint: LifecycleHint,
) -> Result<(), OpenCodeObserverError> {
    let runtime_revision = authority
        .runtime_by_id(evidence.context.runtime_id)?
        .ok_or(StateError::UnknownRuntime(evidence.context.runtime_id))?
        .revision;
    let observation = OpenCodeLifecycleObservation {
        generation: evidence.context.generation.clone(),
        cwd: evidence.context.cwd.clone(),
        runtime_revision,
        session: evidence.context.session.clone(),
        observer_pid: evidence.observer_pid,
        observer_birth: evidence.observer_birth.to_owned(),
        hint,
    };
    authority
        .apply_observation(evidence.context.runtime_id, &observation)
        .map_err(OpenCodeObserverError::State)
}

fn mark_unknown<A: ObserverAuthority>(authority: &mut A, context: &OpenCodeObserverContext) {
    if let Ok(Some(handle)) = authority.opencode_runtime_handle(context.runtime_id)
        && let (Some(observer_pid), Some(observer_birth)) =
            (handle.observer_pid, handle.observer_birth.as_deref())
    {
        let _ = authority.mark_unknown(
            context.runtime_id,
            &context.generation,
            handle.revision,
            observer_pid,
            observer_birth,
        );
    }
}

/// Marks an exact handle unknown for actions that cannot enter the observer.
pub fn mark_unknown_handle(
    registry: &mut HostRegistry,
    handle: &crate::state::OpenCodeRuntimeHandle,
    generation: &str,
) {
    if handle.runtime_generation != generation {
        return;
    }
    if let (Some(observer_pid), Some(observer_birth)) =
        (handle.observer_pid, handle.observer_birth.as_deref())
    {
        let _ = registry.mark_opencode_observer_unknown_exact(
            handle.runtime_id,
            generation,
            handle.revision,
            observer_pid,
            observer_birth,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_status_retains_a_completed_candidate_until_idle() {
        let mut candidate = Some("message-1".to_owned());
        record_candidate(
            &mut candidate,
            &OpenCodeEvent {
                hint: Some(LifecycleHint::Working),
                candidate_message_id: None,
                clears_candidate: false,
            },
        );
        assert_eq!(candidate.as_deref(), Some("message-1"));
    }

    #[test]
    fn incomplete_message_update_clears_a_completed_candidate() {
        let mut candidate = Some("message-1".to_owned());
        record_candidate(
            &mut candidate,
            &OpenCodeEvent {
                hint: Some(LifecycleHint::Working),
                candidate_message_id: None,
                clears_candidate: true,
            },
        );
        assert_eq!(candidate, None);
    }

    #[test]
    fn completed_candidate_is_retained_until_idle_evidence() {
        let mut candidate = None;
        record_candidate(
            &mut candidate,
            &OpenCodeEvent {
                hint: None,
                candidate_message_id: Some("message-1".to_owned()),
                clears_candidate: false,
            },
        );
        assert_eq!(candidate.as_deref(), Some("message-1"));
    }

    #[test]
    fn working_event_status_corroboration_is_lazy_and_exact() {
        let calls = std::cell::Cell::new(0);
        assert!(!should_apply_working_event(
            Some(RuntimeStatus::Working),
            || {
                calls.set(calls.get() + 1);
                Some(OpenCodeSessionStatus::Busy)
            }
        ));
        assert_eq!(calls.get(), 0);
        assert!(!should_apply_working_event(None, || {
            calls.set(calls.get() + 1);
            Some(OpenCodeSessionStatus::Busy)
        }));
        assert_eq!(calls.get(), 0);

        assert!(should_apply_working_event(
            Some(RuntimeStatus::Attention),
            || {
                calls.set(calls.get() + 1);
                Some(OpenCodeSessionStatus::Busy)
            }
        ));
        assert_eq!(calls.get(), 1);
        assert!(!should_apply_working_event(
            Some(RuntimeStatus::Attention),
            || {
                calls.set(calls.get() + 1);
                Some(OpenCodeSessionStatus::Idle)
            }
        ));
        assert_eq!(calls.get(), 2);
        assert!(!should_apply_working_event(
            Some(RuntimeStatus::Attention),
            || {
                calls.set(calls.get() + 1);
                Some(OpenCodeSessionStatus::Unknown)
            }
        ));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn supervision_is_cadence_gated() {
        let start = Instant::now();
        assert!(!supervision_due(start, start + Duration::from_millis(499)));
        assert!(supervision_due(start, start + SUPERVISION_INTERVAL));
    }

    #[test]
    fn startup_readiness_does_not_probe_http_or_sse_when_endpoint_is_unowned() {
        let calls = std::cell::RefCell::new(Vec::new());
        let result = startup_readiness_step(
            || {
                calls.borrow_mut().push("endpoint");
                false
            },
            || {
                calls.borrow_mut().push("provider");
                true
            },
            || {
                calls.borrow_mut().push("stream");
                Some(())
            },
        );

        assert_eq!(result, None);
        assert_eq!(*calls.borrow(), vec!["endpoint"]);
    }

    #[test]
    fn startup_readiness_retries_false_then_true_within_the_existing_loop() {
        let endpoint_samples = std::cell::Cell::new(0);
        let provider_calls = std::cell::Cell::new(0);
        let stream_calls = std::cell::Cell::new(0);
        let mut result = None;
        for _ in 0..3 {
            result = startup_readiness_step(
                || {
                    endpoint_samples.set(endpoint_samples.get() + 1);
                    endpoint_samples.get() >= 2
                },
                || {
                    provider_calls.set(provider_calls.get() + 1);
                    true
                },
                || {
                    stream_calls.set(stream_calls.get() + 1);
                    Some("stream")
                },
            );
            if result.is_some() {
                break;
            }
        }

        assert_eq!(result, Some("stream"));
        assert_eq!(endpoint_samples.get(), 3);
        assert_eq!(provider_calls.get(), 1);
        assert_eq!(stream_calls.get(), 1);
    }

    #[test]
    fn startup_readiness_persistent_endpoint_ambiguity_stays_bounded_and_passive() {
        let endpoint_samples = std::cell::Cell::new(0);
        let provider_calls = std::cell::Cell::new(0);
        let stream_calls = std::cell::Cell::new(0);
        for _ in 0..3 {
            assert_eq!(
                startup_readiness_step(
                    || {
                        endpoint_samples.set(endpoint_samples.get() + 1);
                        false
                    },
                    || {
                        provider_calls.set(provider_calls.get() + 1);
                        true
                    },
                    || {
                        stream_calls.set(stream_calls.get() + 1);
                        Some(())
                    },
                ),
                None
            );
        }

        assert_eq!(endpoint_samples.get(), 3);
        assert_eq!(provider_calls.get(), 0);
        assert_eq!(stream_calls.get(), 0);
    }

    #[test]
    fn startup_readiness_rechecks_ownership_immediately_before_sse() {
        let endpoint_samples = std::cell::Cell::new(0);
        let provider_calls = std::cell::Cell::new(0);
        let stream_calls = std::cell::Cell::new(0);
        let result = startup_readiness_step(
            || {
                endpoint_samples.set(endpoint_samples.get() + 1);
                endpoint_samples.get() == 1
            },
            || {
                provider_calls.set(provider_calls.get() + 1);
                true
            },
            || {
                stream_calls.set(stream_calls.get() + 1);
                Some(())
            },
        );

        assert_eq!(result, None);
        assert_eq!(endpoint_samples.get(), 2);
        assert_eq!(provider_calls.get(), 1);
        assert_eq!(stream_calls.get(), 0);
    }

    #[test]
    fn standby_buffer_rejects_the_bounded_entry_limit() {
        let mut buffer = StandbyEventBuffer::new();
        let event = OpenCodeEvent {
            hint: None,
            candidate_message_id: None,
            clears_candidate: false,
        };
        for _ in 0..STANDBY_MAX_EVENTS {
            buffer.push(event.clone()).unwrap();
        }
        assert!(matches!(
            buffer.push(event),
            Err(OpenCodeObserverError::StandbyBufferFull)
        ));
        assert_eq!(buffer.len(), STANDBY_MAX_EVENTS);
    }

    #[test]
    fn standby_buffer_rejects_unbounded_message_metadata() {
        let mut buffer = StandbyEventBuffer::new();
        let event = OpenCodeEvent {
            hint: None,
            candidate_message_id: Some("m".repeat(STANDBY_MAX_EVENT_METADATA_BYTES)),
            clears_candidate: false,
        };
        assert!(matches!(
            buffer.push(event),
            Err(OpenCodeObserverError::StandbyBufferFull)
        ));
        assert!(buffer.is_empty());
    }

    #[test]
    fn standby_buffer_replays_only_events_after_the_committed_settlement() {
        let candidate = |message_id: &str| OpenCodeEvent {
            hint: None,
            candidate_message_id: Some(message_id.to_owned()),
            clears_candidate: false,
        };
        let settled = OpenCodeEvent {
            hint: Some(LifecycleHint::Settled { message_id: None }),
            candidate_message_id: None,
            clears_candidate: false,
        };
        let mut buffer = StandbyEventBuffer::new();
        buffer.push(candidate("already-committed")).unwrap();
        buffer.push(settled.clone()).unwrap();
        buffer.push(candidate("after-freeze")).unwrap();
        buffer.push(settled.clone()).unwrap();

        let replay = buffer.reconcile_committed_prefix(Some("already-committed"));
        assert_eq!(
            replay.into_events(),
            VecDeque::from([candidate("after-freeze"), settled])
        );
    }

    #[test]
    fn standby_buffer_retains_every_event_when_baseline_is_not_buffered() {
        let event = OpenCodeEvent {
            hint: Some(LifecycleHint::Working),
            candidate_message_id: None,
            clears_candidate: false,
        };
        let mut buffer = StandbyEventBuffer::new();
        buffer.push(event.clone()).unwrap();
        let replay = buffer.reconcile_committed_prefix(Some("earlier-message"));
        assert_eq!(replay.into_events(), VecDeque::from([event]));
    }

    #[test]
    fn standby_activation_ack_is_bounded_and_binds_exact_handle_metadata() {
        let runtime_id = RuntimeId::new();
        let context = OpenCodeObserverContext {
            runtime_id,
            generation: "generation-a".to_owned(),
            endpoint: OpenCodeEndpoint::loopback(4312).unwrap(),
            session: ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "session-a")
                .unwrap(),
            pane_pid: 42,
            cwd: PathBuf::from("/disposable/project"),
            provider_birth: "provider-birth".to_owned(),
            provider_version: Some("1.0".to_owned()),
            mode: OpenCodeObserverMode::D16Standby,
        };
        let line = standby_activation_ack_line(
            &context,
            84,
            "observer-birth",
            crate::domain::Revision::INITIAL.next(),
        )
        .unwrap();
        assert_eq!(
            std::str::from_utf8(&line).unwrap(),
            format!("ACTIVATED 84 observer-birth {runtime_id} 2 generation-a\n")
        );

        let oversized = OpenCodeObserverContext {
            generation: "g".repeat(STANDBY_ACTIVATION_ACK_LINE_MAX_BYTES),
            ..context
        };
        assert!(matches!(
            standby_activation_ack_line(
                &oversized,
                84,
                "observer-birth",
                crate::domain::Revision::INITIAL.next(),
            ),
            Err(OpenCodeObserverError::StandbyReadinessFailed)
        ));
    }

    #[test]
    fn standby_rejects_invalid_identity_before_state_root_io() {
        let temporary = tempfile::tempdir().unwrap();
        let context = OpenCodeObserverContext {
            runtime_id: RuntimeId::new(),
            generation: "generation-a".to_owned(),
            endpoint: OpenCodeEndpoint::loopback(4312).unwrap(),
            session: ProviderSessionId::new(crate::domain::ProviderKind::OpenCode, "session-a")
                .unwrap(),
            pane_pid: 0,
            cwd: PathBuf::from("/disposable/project"),
            provider_birth: "provider-birth".to_owned(),
            provider_version: Some("1.0".to_owned()),
            mode: OpenCodeObserverMode::D16Standby,
        };
        assert!(matches!(
            run_standby(temporary.path(), &context),
            Err(OpenCodeObserverError::StandbyReadinessFailed)
        ));
        assert!(!temporary.path().join("host.sqlite").exists());
    }
}
