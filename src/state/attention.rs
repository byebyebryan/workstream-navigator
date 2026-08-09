use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::domain::{
    AttentionState, DomainError, ProviderKind, ProviderSessionId, Revision, WorkstreamId,
};

use super::models::{HostRegistry, StateError};
use super::utils::{provider_kind_from_text, to_from_sql_error};

impl HostRegistry {
    /// Records a settled provider result and leaves prior unseen result attention sticky.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream is unknown, its persisted provider
    /// differs from the session provider, or the state transaction fails.
    pub fn mark_result_attention(
        &mut self,
        workstream_id: WorkstreamId,
        session_id: ProviderSessionId,
        turn_id: String,
    ) -> Result<AttentionState, StateError> {
        self.update_attention_with_provider(
            workstream_id,
            Some(session_id.provider()),
            |attention| attention.mark_result(session_id, turn_id),
        )
    }

    /// Records a recovery-required attention condition.
    ///
    /// # Errors
    ///
    /// Returns an error when the state transaction cannot be completed.
    pub fn mark_recovery_attention(
        &mut self,
        workstream_id: WorkstreamId,
    ) -> Result<AttentionState, StateError> {
        self.update_attention(workstream_id, |attention| {
            attention.mark_recovery_required();
            Ok(())
        })
    }

    /// Clears result attention only at the caller's observed revision.
    ///
    /// # Errors
    ///
    /// Returns an error when a newer attention update exists or the state
    /// transaction cannot be completed.
    pub fn acknowledge_result_attention(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    ) -> Result<AttentionState, StateError> {
        self.update_attention(workstream_id, |attention| {
            attention.acknowledge_result(expected_revision)
        })
    }

    /// Reads the durable attention state for one workstream.
    ///
    /// # Errors
    ///
    /// Returns an error when the state cannot be queried or contains invalid
    /// persisted data.
    pub fn attention(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<Option<AttentionState>, StateError> {
        load_attention_from_connection(&self.connection, workstream_id)
    }

    fn update_attention(
        &mut self,
        workstream_id: WorkstreamId,
        update: impl FnOnce(&mut AttentionState) -> Result<(), DomainError>,
    ) -> Result<AttentionState, StateError> {
        self.update_attention_with_provider(workstream_id, None, update)
    }

    fn update_attention_with_provider(
        &mut self,
        workstream_id: WorkstreamId,
        expected_provider: Option<ProviderKind>,
        update: impl FnOnce(&mut AttentionState) -> Result<(), DomainError>,
    ) -> Result<AttentionState, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        if let Some(expected_provider) = expected_provider {
            let stored_provider = transaction
                .query_row(
                    "SELECT provider FROM workstreams WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(StateError::Sqlite)?
                .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
            let stored_provider = provider_kind_from_text(&stored_provider)?;
            if stored_provider != expected_provider {
                return Err(StateError::ProviderIdentityMismatch);
            }
        }
        let mut attention = load_attention_from_transaction(&transaction, workstream_id)?
            .unwrap_or_else(|| AttentionState::new(workstream_id));
        let prior_revision = attention.revision;
        update(&mut attention)?;
        let changed = transaction
            .execute(
                "INSERT INTO attention_states (
                    workstream_id, result_unseen_since_revision,
                    recovery_unseen_since_revision, latest_native_session_id,
                    latest_native_session_provider,
                    latest_turn_id, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(workstream_id) DO UPDATE SET
                    result_unseen_since_revision = excluded.result_unseen_since_revision,
                    recovery_unseen_since_revision = excluded.recovery_unseen_since_revision,
                    latest_native_session_id = excluded.latest_native_session_id,
                    latest_native_session_provider = excluded.latest_native_session_provider,
                    latest_turn_id = excluded.latest_turn_id,
                    revision = excluded.revision
                 WHERE attention_states.revision = ?8",
                params![
                    attention.workstream_id.to_string(),
                    attention.result_unseen_since_revision.map(Revision::value),
                    attention
                        .recovery_unseen_since_revision
                        .map(Revision::value),
                    attention
                        .latest_native_session_id
                        .as_ref()
                        .map(ProviderSessionId::native_id),
                    attention
                        .latest_native_session_id
                        .as_ref()
                        .map(|session| session.provider().as_str()),
                    attention.latest_turn_id,
                    attention.revision.value(),
                    prior_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(attention)
    }
}

pub(in crate::state) fn ensure_recovery_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let mut attention = load_attention_from_transaction(transaction, workstream_id)?
        .unwrap_or_else(|| AttentionState::new(workstream_id));
    if attention.recovery_unseen_since_revision.is_some() {
        return Ok(());
    }
    let prior_revision = attention.revision;
    attention.mark_recovery_required();
    save_attention_in_transaction(transaction, &attention, prior_revision)
}

pub(in crate::state) fn clear_recovery_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let mut attention = load_attention_from_transaction(transaction, workstream_id)?
        .ok_or(StateError::HookEvidenceMismatch)?;
    if attention.recovery_unseen_since_revision.is_none() {
        return Err(StateError::HookEvidenceMismatch);
    }
    let prior_revision = attention.revision;
    attention.clear_recovery_required();
    save_attention_in_transaction(transaction, &attention, prior_revision)
}

pub(in crate::state) fn save_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    attention: &AttentionState,
    prior_revision: Revision,
) -> Result<(), StateError> {
    let changed = transaction
        .execute(
            "INSERT INTO attention_states (
            workstream_id, result_unseen_since_revision,
            recovery_unseen_since_revision, latest_native_session_id,
            latest_native_session_provider, latest_turn_id, revision
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(workstream_id) DO UPDATE SET
            result_unseen_since_revision = excluded.result_unseen_since_revision,
            recovery_unseen_since_revision = excluded.recovery_unseen_since_revision,
            latest_native_session_id = excluded.latest_native_session_id,
            latest_native_session_provider = excluded.latest_native_session_provider,
            latest_turn_id = excluded.latest_turn_id,
            revision = excluded.revision
         WHERE attention_states.revision = ?8",
            params![
                attention.workstream_id.to_string(),
                attention.result_unseen_since_revision.map(Revision::value),
                attention
                    .recovery_unseen_since_revision
                    .map(Revision::value),
                attention
                    .latest_native_session_id
                    .as_ref()
                    .map(ProviderSessionId::native_id),
                attention
                    .latest_native_session_id
                    .as_ref()
                    .map(|session| session.provider().as_str()),
                attention.latest_turn_id,
                attention.revision.value(),
                prior_revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

pub(in crate::state) fn mark_result_attention_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
    session_id: ProviderSessionId,
    turn_id: String,
) -> Result<(), StateError> {
    let current = load_attention_from_transaction(transaction, workstream_id)?;
    let mut attention = current.unwrap_or_else(|| AttentionState::new(workstream_id));
    let prior_revision = attention.revision;
    attention.mark_result(session_id, turn_id)?;
    save_attention_in_transaction(transaction, &attention, prior_revision)
}

pub(in crate::state) fn load_attention_from_connection(
    connection: &Connection,
    workstream_id: WorkstreamId,
) -> Result<Option<AttentionState>, StateError> {
    let attention = connection
        .query_row(
            "SELECT result_unseen_since_revision, recovery_unseen_since_revision,
                    latest_native_session_id, latest_native_session_provider,
                    latest_turn_id, revision
             FROM attention_states WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| row_to_attention(row, workstream_id),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(attention)
}

pub(in crate::state) fn load_attention_from_transaction(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<Option<AttentionState>, StateError> {
    let attention = transaction
        .query_row(
            "SELECT result_unseen_since_revision, recovery_unseen_since_revision,
                    latest_native_session_id, latest_native_session_provider,
                    latest_turn_id, revision
             FROM attention_states WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| row_to_attention(row, workstream_id),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(attention)
}

pub(in crate::state) fn row_to_attention(
    row: &rusqlite::Row<'_>,
    workstream_id: WorkstreamId,
) -> rusqlite::Result<AttentionState> {
    let result: Option<i64> = row.get(0)?;
    let recovery: Option<i64> = row.get(1)?;
    let native_session_id: Option<String> = row.get(2)?;
    let provider: Option<String> = row.get(3)?;
    let latest_native_session_id = match (native_session_id, provider) {
        (None, None) => None,
        (Some(native_session_id), Some(provider)) => {
            let provider = provider_kind_from_text(&provider).map_err(to_from_sql_error)?;
            Some(ProviderSessionId::new(provider, native_session_id).map_err(to_from_sql_error)?)
        }
        _ => {
            return Err(to_from_sql_error(StateError::ProviderIdentityMismatch));
        }
    };
    Ok(AttentionState {
        workstream_id,
        result_unseen_since_revision: result
            .map(Revision::try_from)
            .transpose()
            .map_err(to_from_sql_error)?,
        recovery_unseen_since_revision: recovery
            .map(Revision::try_from)
            .transpose()
            .map_err(to_from_sql_error)?,
        latest_native_session_id,
        latest_turn_id: row.get(4)?,
        revision: Revision::try_from(row.get::<_, i64>(5)?).map_err(to_from_sql_error)?,
    })
}
