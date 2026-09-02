use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{
    Clock, ProviderKind, ProviderSessionId, Revision, RuntimeId, SystemClock, WorkstreamId,
    WorkstreamLifecycle,
};
use crate::provider::lifecycle::{LifecycleEvent, LifecycleHint, LifecycleObservation};

use super::models::{
    HostRegistry, OpenCodeLifecycleObservation, OpenCodeObserverStatus, ProviderBinding, StateError,
};
use super::runtime::load_binding;
use super::utils::{
    provider_kind_from_text, validate_provider_metadata, workstream_lifecycle_from_text,
};
use super::workstream::{reopen_recovery_workstream, touch_workstream};

impl HostRegistry {
    /// Applies one already-authorized lifecycle observation to its exact runtime.
    ///
    /// Hooks supply evidence only: an initial session can bind solely while the
    /// runtime is `starting`. The one proven native same-TUI replacement is a
    /// distinct `SessionStart(source=clear)` after an idle or attention state;
    /// all other replacement claims fail closed. A settled result and its
    /// exact provider binding commit in the same `SQLite` transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime generation, cwd, binding, lifecycle, or
    /// revision evidence is ambiguous or does not match a managed runtime.
    pub fn apply_lifecycle_observation(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        observation: LifecycleObservation,
    ) -> Result<(), StateError> {
        let activity_at_millis = match observation.event {
            LifecycleEvent::UserPromptSubmit | LifecycleEvent::Stop => {
                Some(SystemClock.now_millis()?)
            }
            LifecycleEvent::SessionStart | LifecycleEvent::SessionEnd => None,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let runtime = transaction
            .query_row(
                "SELECT runtimes.workstream_id, runtimes.provider, runtimes.tmux_generation,
                        runtimes.cwd, runtimes.lifecycle, runtimes.revision,
                        workstreams.provider, workstreams.lifecycle
                 FROM runtimes JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1",
                [runtime_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownRuntime(runtime_id))?;
        let workstream_id = Uuid::parse_str(&runtime.0)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let provider = provider_kind_from_text(&runtime.1)?;
        let workstream_provider = provider_kind_from_text(&runtime.6)?;
        if provider != workstream_provider {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let revision = Revision::try_from(runtime.5)?;
        if runtime.2 != generation || runtime.3 != observation.cwd {
            return Err(StateError::HookEvidenceMismatch);
        }
        let existing = load_binding(&transaction, runtime_id)?;
        if provider != ProviderKind::Codex {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let observed_session =
            ProviderSessionId::new(provider, observation.native_session_id.clone())?;
        apply_lifecycle_event(
            LifecycleEventContext {
                transaction: &transaction,
                runtime_id,
                provider,
                runtime_status: &runtime.4,
                runtime_revision: revision,
                generation,
                workstream_id,
                workstream_lifecycle: workstream_lifecycle_from_text(&runtime.7)?,
                existing,
                observed_session,
            },
            &observation,
        )?;
        touch_workstream(&transaction, &runtime.0, activity_at_millis)?;
        let result = transaction.commit().map_err(StateError::Sqlite);
        drop(observation);
        result
    }
}

pub(in crate::state) fn validate_opencode_observation(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    observation: &OpenCodeLifecycleObservation,
) -> Result<(String, WorkstreamId), StateError> {
    let runtime = transaction
        .query_row(
            "SELECT provider, tmux_generation, cwd, lifecycle, revision, workstream_id
             FROM runtimes WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::UnknownRuntime(runtime_id))?;
    if provider_kind_from_text(&runtime.0)? != ProviderKind::OpenCode
        || runtime.1 != observation.generation
        || runtime.2 != observation.cwd.to_string_lossy()
        || Revision::try_from(runtime.4)? != observation.runtime_revision
    {
        return Err(StateError::HookEvidenceMismatch);
    }
    let handle = transaction
        .query_row(
            "SELECT observer_pid, observer_birth, observer_status
             FROM opencode_runtime_handles
             WHERE runtime_id = ?1 AND runtime_generation = ?2",
            params![runtime_id.to_string(), observation.generation],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::HookEvidenceMismatch)?;
    if handle.0 != Some(i64::from(observation.observer_pid))
        || handle.1.as_deref() != Some(&observation.observer_birth)
        || handle.2 != OpenCodeObserverStatus::Ready.as_str()
    {
        return Err(StateError::HookEvidenceMismatch);
    }
    let binding = load_binding(transaction, runtime_id)?.ok_or(StateError::HookEvidenceMismatch)?;
    if binding.provider != ProviderKind::OpenCode
        || binding.native_session_id != observation.session
    {
        return Err(StateError::ProviderIdentityMismatch);
    }
    if binding.runtime_generation != observation.generation {
        return Err(StateError::HookEvidenceMismatch);
    }
    let workstream_id = Uuid::parse_str(&runtime.5)
        .map(WorkstreamId::from)
        .map_err(StateError::InvalidPersistedUuid)?;
    Ok((runtime.3, workstream_id))
}

pub(in crate::state) fn apply_opencode_lifecycle_transition(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    runtime_revision: Revision,
    lifecycle: &str,
    workstream_id: WorkstreamId,
    observation: &OpenCodeLifecycleObservation,
) -> Result<bool, StateError> {
    match &observation.hint {
        LifecycleHint::Started => {
            if lifecycle != "starting" {
                return Err(StateError::HookEvidenceMismatch);
            }
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "idle")?;
            let workstream_lifecycle: String = transaction
                .query_row(
                    "SELECT lifecycle FROM workstreams WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(StateError::Sqlite)?;
            if workstream_lifecycle == "recovery_required" {
                let binding = load_binding(transaction, runtime_id)?
                    .ok_or(StateError::HookEvidenceMismatch)?;
                if binding.provider != ProviderKind::OpenCode
                    || binding.native_session_id != observation.session
                    || binding.start_source != "resume"
                {
                    return Err(StateError::HookEvidenceMismatch);
                }
                reopen_recovery_workstream(transaction, workstream_id)?;
            }
            Ok(true)
        }
        LifecycleHint::Working => {
            if !matches!(lifecycle, "starting" | "idle" | "working" | "attention") {
                return Err(StateError::HookEvidenceMismatch);
            }
            if lifecycle == "working" {
                return Ok(false);
            }
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "working")?;
            Ok(true)
        }
        LifecycleHint::Settled { message_id } => {
            if !matches!(lifecycle, "starting" | "idle" | "working" | "attention") {
                return Err(StateError::HookEvidenceMismatch);
            }
            let message_id = message_id
                .as_deref()
                .ok_or(StateError::HookEvidenceMismatch)?;
            validate_provider_metadata(message_id)?;
            if !record_opencode_settled_message(transaction, runtime_id, observation, message_id)? {
                // A delayed duplicate must not touch the binding, Runtime,
                // Workstream activity or provider binding.
                return Ok(false);
            }
            let changed = transaction
                .execute(
                    "UPDATE provider_bindings SET last_settled_turn_id = ?1,
                        revision = revision + 1
                     WHERE runtime_id = ?2 AND provider = 'opencode'
                       AND native_session_id = ?3 AND runtime_generation = ?4",
                    params![
                        message_id,
                        runtime_id.to_string(),
                        observation.session.native_id(),
                        observation.generation,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            if changed != 1 {
                return Err(StateError::ConcurrentWrite);
            }
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "attention")?;
            Ok(true)
        }
        LifecycleHint::Ended => {
            if !matches!(lifecycle, "starting" | "idle" | "working" | "attention") {
                return Err(StateError::HookEvidenceMismatch);
            }
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "stopped")?;
            Ok(true)
        }
    }
}

/// Persists one exact `OpenCode` settled-message identity before applying its
/// lifecycle effects. The current schema records the complete retained identity set,
/// so reconnect/retry delivery remains idempotent for the lifetime of the
/// Runtime generation/session. This table is the current idempotency boundary;
/// a repeated latest settled event remains a safe no-op.
fn record_opencode_settled_message(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    observation: &OpenCodeLifecycleObservation,
    message_id: &str,
) -> Result<bool, StateError> {
    let schema_version: i64 = transaction
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StateError::Sqlite)?;
    if schema_version != super::schema::HOST_SCHEMA_VERSION {
        return Err(StateError::MalformedHostSchema);
    }
    let previous = transaction
        .query_row(
            "SELECT last_settled_turn_id
             FROM provider_bindings
             WHERE runtime_id = ?1 AND provider = 'opencode'
               AND native_session_id = ?2 AND runtime_generation = ?3",
            params![
                runtime_id.to_string(),
                observation.session.native_id(),
                observation.generation,
            ],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::HookEvidenceMismatch)?;
    if previous.as_deref() == Some(message_id) {
        return Ok(false);
    }
    let changed = transaction
        .execute(
            "INSERT OR IGNORE INTO opencode_settled_messages (
                runtime_id, runtime_generation, native_session_id, message_id
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                runtime_id.to_string(),
                observation.generation,
                observation.session.native_id(),
                message_id,
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 0 {
        return Ok(false);
    }
    Ok(true)
}

pub(in crate::state) struct LifecycleEventContext<'tx, 'db> {
    pub(in crate::state) transaction: &'tx rusqlite::Transaction<'db>,
    pub(in crate::state) runtime_id: RuntimeId,
    pub(in crate::state) provider: ProviderKind,
    pub(in crate::state) runtime_status: &'tx str,
    pub(in crate::state) runtime_revision: Revision,
    pub(in crate::state) generation: &'tx str,
    pub(in crate::state) workstream_id: WorkstreamId,
    pub(in crate::state) workstream_lifecycle: WorkstreamLifecycle,
    pub(in crate::state) existing: Option<ProviderBinding>,
    pub(in crate::state) observed_session: ProviderSessionId,
}

pub(in crate::state) fn apply_lifecycle_event(
    context: LifecycleEventContext<'_, '_>,
    observation: &LifecycleObservation,
) -> Result<(), StateError> {
    let LifecycleEventContext {
        transaction,
        runtime_id,
        provider,
        runtime_status,
        runtime_revision,
        generation,
        workstream_id,
        workstream_lifecycle,
        existing,
        observed_session,
    } = context;
    match observation.event {
        LifecycleEvent::SessionStart => apply_session_start(
            transaction,
            &SessionStartContext {
                runtime_id,
                provider,
                runtime_status,
                runtime_revision,
                generation,
                workstream_id,
                workstream_lifecycle,
            },
            existing,
            observed_session.native_id(),
            observation.source.as_deref(),
        ),
        LifecycleEvent::UserPromptSubmit => {
            require_matching_binding(
                existing.as_ref(),
                &observation.native_session_id,
                generation,
            )?;
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "working")
        }
        LifecycleEvent::Stop => {
            let turn_id = observation
                .turn_id
                .clone()
                .ok_or(StateError::HookEvidenceMismatch)?;
            require_matching_binding(
                existing.as_ref(),
                &observation.native_session_id,
                generation,
            )?;
            validate_provider_metadata(&turn_id)?;
            let changed = transaction
                .execute(
                    "UPDATE provider_bindings SET last_settled_turn_id = ?1, revision = revision + 1
                     WHERE runtime_id = ?2 AND runtime_generation = ?3",
                    params![turn_id, runtime_id.to_string(), generation],
                )
                .map_err(StateError::Sqlite)?;
            if changed != 1 {
                return Err(StateError::ConcurrentWrite);
            }
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "attention")?;
            Ok(())
        }
        LifecycleEvent::SessionEnd => {
            require_matching_binding(
                existing.as_ref(),
                &observation.native_session_id,
                generation,
            )?;
            update_runtime_lifecycle(transaction, runtime_id, runtime_revision, "stopped")
        }
    }
}

pub(in crate::state) struct SessionStartContext<'a> {
    runtime_id: RuntimeId,
    provider: ProviderKind,
    runtime_status: &'a str,
    runtime_revision: Revision,
    generation: &'a str,
    workstream_id: WorkstreamId,
    workstream_lifecycle: WorkstreamLifecycle,
}

pub(in crate::state) fn apply_session_start(
    transaction: &rusqlite::Transaction<'_>,
    context: &SessionStartContext<'_>,
    existing: Option<ProviderBinding>,
    session_id: &str,
    source: Option<&str>,
) -> Result<(), StateError> {
    let session_id = ProviderSessionId::new(context.provider, session_id)?;
    let Some(binding) = existing else {
        return insert_initial_binding(transaction, context, session_id.native_id(), source);
    };
    if binding.provider != context.provider || binding.native_session_id == session_id {
        if binding.provider != context.provider {
            return Err(StateError::ProviderIdentityMismatch);
        }
        // A persisted binding appears at `starting` only when an exact parked
        // session is resumed in a fresh private tmux generation. Repeated live
        // SessionStart evidence must not mark a working turn idle.
        if context.runtime_status != "starting" {
            return Err(StateError::HookEvidenceMismatch);
        }
        if context.workstream_lifecycle == WorkstreamLifecycle::RecoveryRequired
            && source != Some("resume")
        {
            return Err(StateError::HookEvidenceMismatch);
        }
        return complete_session_start(transaction, context);
    }
    if binding.runtime_generation != context.generation {
        return Err(StateError::HookEvidenceMismatch);
    }
    if source != Some("clear") || !matches!(context.runtime_status, "idle" | "attention") {
        return Err(StateError::HookEvidenceMismatch);
    }
    let changed = transaction
        .execute(
            "UPDATE provider_bindings SET
                native_session_id = ?1,
                start_source = 'clear',
                last_settled_turn_id = NULL,
                observed_thread_name = NULL,
                name_state = 'unavailable',
                name_observed_at = NULL,
                predecessor_native_session_id = ?2,
                predecessor_effective_name = ?3,
                revision = revision + 1
             WHERE runtime_id = ?4 AND native_session_id = ?2 AND revision = ?5",
            params![
                session_id.native_id(),
                binding.native_session_id.native_id(),
                binding.observed_thread_name,
                context.runtime_id.to_string(),
                binding.revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    update_runtime_lifecycle(
        transaction,
        context.runtime_id,
        context.runtime_revision,
        "idle",
    )
}

pub(in crate::state) fn insert_initial_binding(
    transaction: &rusqlite::Transaction<'_>,
    context: &SessionStartContext<'_>,
    session_id: &str,
    source: Option<&str>,
) -> Result<(), StateError> {
    if context.runtime_status != "starting" || !matches!(source, Some("startup" | "resume")) {
        return Err(StateError::HookEvidenceMismatch);
    }
    if context.workstream_lifecycle == WorkstreamLifecycle::RecoveryRequired
        && source != Some("resume")
    {
        return Err(StateError::HookEvidenceMismatch);
    }
    let session_id = ProviderSessionId::new(context.provider, session_id)?;
    transaction
        .execute(
            "INSERT INTO provider_bindings (
                binding_id, runtime_id, provider, native_session_id, start_source,
                last_settled_turn_id, observed_thread_name, name_state,
                name_observed_at, predecessor_native_session_id,
                predecessor_effective_name, runtime_generation, revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, 'unavailable', NULL,
                NULL, NULL, ?6, 1)",
            params![
                Uuid::new_v4().to_string(),
                context.runtime_id.to_string(),
                context.provider.as_str(),
                session_id.native_id(),
                source.unwrap_or("startup"),
                context.generation,
            ],
        )
        .map_err(StateError::Sqlite)?;
    complete_session_start(transaction, context)
}

pub(in crate::state) fn complete_session_start(
    transaction: &rusqlite::Transaction<'_>,
    context: &SessionStartContext<'_>,
) -> Result<(), StateError> {
    let binding = transaction
        .query_row(
            "SELECT runtime_generation, revision FROM provider_bindings
             WHERE runtime_id = ?1",
            [context.runtime_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::HookEvidenceMismatch)?;
    if binding.0 != context.generation {
        let changed = transaction
            .execute(
                "UPDATE provider_bindings SET runtime_generation = ?1,
                    revision = revision + 1
                 WHERE runtime_id = ?2 AND runtime_generation = ?3 AND revision = ?4",
                params![
                    context.generation,
                    context.runtime_id.to_string(),
                    binding.0,
                    binding.1,
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
    }
    update_runtime_lifecycle(
        transaction,
        context.runtime_id,
        context.runtime_revision,
        "idle",
    )?;
    if context.workstream_lifecycle == WorkstreamLifecycle::RecoveryRequired {
        reopen_recovery_workstream(transaction, context.workstream_id)?;
    }
    Ok(())
}

pub(in crate::state) fn require_matching_binding(
    binding: Option<&ProviderBinding>,
    session_id: &str,
    generation: &str,
) -> Result<(), StateError> {
    let session_id = ProviderSessionId::codex(session_id)?;
    if binding.is_some_and(|binding| {
        binding.provider == ProviderKind::Codex
            && binding.native_session_id == session_id
            && binding.runtime_generation == generation
    }) {
        Ok(())
    } else {
        Err(StateError::HookEvidenceMismatch)
    }
}

pub(in crate::state) fn update_runtime_lifecycle(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    expected_revision: Revision,
    lifecycle: &'static str,
) -> Result<(), StateError> {
    let updated = transaction
        .execute(
            "UPDATE runtimes SET lifecycle = ?1, revision = revision + 1
             WHERE runtime_id = ?2 AND revision = ?3",
            params![lifecycle, runtime_id.to_string(), expected_revision.value()],
        )
        .map_err(StateError::Sqlite)?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}
