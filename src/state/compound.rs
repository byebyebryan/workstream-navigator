use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{
    CompoundOperation, DomainError, IdGenerator, LocationId, OperationId, OperationKind,
    OperationPhase, ProjectId, ProviderKind, ProviderSessionId, RandomIdGenerator, Revision,
    RuntimeId, WorkstreamId, WorkstreamOrigin,
};

use super::models::{
    CreatedWorkstream, HostRegistry, OPENCODE_SESSION_CREATION_CLEANUP_UNKNOWN_CODE,
    OPENCODE_SESSION_CREATION_PLAN_SCHEMA_VERSION, OPENCODE_SESSION_CREATION_UNKNOWN_CODE,
    OpenCodeSessionCreationOperation, OperationOverview, OperationOverviewPage,
    PersistedOpenCodeSessionCreationPlan, ProviderBinding, StateError,
};
use super::runtime::load_binding;
use super::schema::MAX_NAVIGATOR_WORKSTREAMS;
use super::utils::{
    operation_kind_from_text, operation_kind_text, operation_phase_from_text, operation_phase_text,
    provider_kind_from_text, to_from_sql_error, validate_registry_text,
};
use super::workstream::next_activity_sequence;

impl HostRegistry {
    /// Creates a fresh Workstream at the source Project's registered root.
    /// The destination provider is explicit and may differ from the source;
    /// replaying a request with a different provider is rejected.
    /// The request key deduplicates an interrupted host-local request without
    /// creating a branch, worktree, or repository side effect.
    ///
    /// An archived source is still a retained `ProjectLocation` and may seed a
    /// new independent Workstream without restoring or resuming the source.
    ///
    /// # Errors
    ///
    /// Returns an error when the source is unknown or stale, request-key reuse
    /// conflicts, or the atomic state change cannot commit.
    pub fn create_independent_workstream(
        &mut self,
        request_key: &str,
        source_workstream_id: WorkstreamId,
        expected_source_revision: Revision,
        provider: ProviderKind,
    ) -> Result<CreatedWorkstream, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        if let Some(existing) = transaction
            .query_row(
                "SELECT source_workstream_id, source_revision, workstream_id
                 FROM independent_creation_requests WHERE request_key = ?1",
                [request_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?
        {
            let source = Uuid::parse_str(&existing.0)
                .map(WorkstreamId::from)
                .map_err(StateError::InvalidPersistedUuid)?;
            if source != source_workstream_id
                || Revision::try_from(existing.1)? != expected_source_revision
            {
                return Err(StateError::OperationRequestMismatch);
            }
            let created = created_workstream_from_record(
                &transaction,
                Uuid::parse_str(&existing.2)
                    .map(WorkstreamId::from)
                    .map_err(StateError::InvalidPersistedUuid)?,
            )?;
            if created.provider != provider {
                return Err(StateError::OperationRequestMismatch);
            }
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(created);
        }

        let (source_location_id, source_revision) =
            load_source_workstream(&transaction, source_workstream_id, true)?;
        if source_revision != expected_source_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_source_revision,
                current: source_revision,
            }));
        }
        let workstream_id = WorkstreamId::new();
        let activity_sequence = next_activity_sequence(&transaction)?;
        transaction
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin, source_workstream_id,
                    lifecycle, last_activity_sequence, last_activity_at_millis, revision
                 ) VALUES (?1, ?2, ?3, 'independent', ?4, 'open', ?5, 0, 1)",
                params![
                    workstream_id.to_string(),
                    source_location_id.to_string(),
                    provider.as_str(),
                    source_workstream_id.to_string(),
                    activity_sequence,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO independent_creation_requests (
                    request_key, source_workstream_id, source_revision, workstream_id
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    request_key,
                    source_workstream_id.to_string(),
                    expected_source_revision.value(),
                    workstream_id.to_string(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        let created = CreatedWorkstream {
            workstream_id,
            location_id: source_location_id,
            provider,
            origin: WorkstreamOrigin::Independent,
            source_workstream_id,
            revision: Revision::INITIAL,
        };
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(created)
    }

    /// Creates an independent Workstream at one exact schema-15 Location.
    ///
    /// Registration creates exactly one external Workstream for a Location;
    /// that retained row is the stable source anchor even after it is
    /// archived. Project and Location revisions are revalidated in the same
    /// transaction that records the independent Workstream and its replay
    /// key. An already-recorded request returns its original result even when
    /// later presentation metadata has changed.
    ///
    /// # Errors
    ///
    /// Returns an error for stale Project or Location evidence, a missing or
    /// ambiguous external source anchor, conflicting request-key reuse, or a
    /// failed atomic write.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the exact Project, Location, revisions, request key, and provider are independent authority inputs"
    )]
    pub fn create_independent_workstream_at_location(
        &mut self,
        project_id: ProjectId,
        location_id: LocationId,
        expected_project_revision: Revision,
        expected_location_revision: Revision,
        request_key: &str,
        provider: ProviderKind,
    ) -> Result<CreatedWorkstream, StateError> {
        validate_registry_text("request key", request_key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;

        if let Some(existing) = transaction
            .query_row(
                "SELECT request.source_workstream_id, request.source_revision,
                        request.workstream_id, source.location_id, source.origin,
                        locations.project_id, destination.location_id, destination.provider
                 FROM independent_creation_requests AS request
                 JOIN workstreams AS source
                   ON source.workstream_id = request.source_workstream_id
                 JOIN project_locations AS locations
                   ON locations.location_id = source.location_id
                 JOIN workstreams AS destination
                   ON destination.workstream_id = request.workstream_id
                 WHERE request.request_key = ?1",
                [request_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?
        {
            let source_workstream_id = Uuid::parse_str(&existing.0)
                .map(WorkstreamId::from)
                .map_err(StateError::InvalidPersistedUuid)?;
            let source_revision = Revision::try_from(existing.1)?;
            let existing_location_id = Uuid::parse_str(&existing.3)
                .map(LocationId::from)
                .map_err(StateError::InvalidPersistedUuid)?;
            let existing_project_id = existing
                .5
                .as_deref()
                .ok_or(StateError::MalformedHostSchema)?
                .parse::<ProjectId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            let destination_location_id = Uuid::parse_str(&existing.6)
                .map(LocationId::from)
                .map_err(StateError::InvalidPersistedUuid)?;
            let destination_provider = provider_kind_from_text(&existing.7)?;
            if existing.4 != "external"
                || existing_location_id != location_id
                || destination_location_id != location_id
                || existing_project_id != project_id
                || destination_provider != provider
            {
                return Err(StateError::OperationRequestMismatch);
            }
            let created = created_workstream_from_record(
                &transaction,
                Uuid::parse_str(&existing.2)
                    .map(WorkstreamId::from)
                    .map_err(StateError::InvalidPersistedUuid)?,
            )?;
            if created.source_workstream_id != source_workstream_id
                || created.provider != provider
                || created.location_id != location_id
                || created.origin != WorkstreamOrigin::Independent
            {
                return Err(StateError::OperationRequestMismatch);
            }
            let _ = source_revision;
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(created);
        }

        let revisions = transaction
            .query_row(
                "SELECT projects.revision, project_locations.revision
                 FROM project_locations
                 JOIN projects ON projects.project_id = project_locations.project_id
                 WHERE projects.project_id = ?1 AND project_locations.location_id = ?2",
                params![project_id.to_string(), location_id.to_string()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        if Revision::try_from(revisions.0)? != expected_project_revision
            || Revision::try_from(revisions.1)? != expected_location_revision
        {
            return Err(StateError::ConcurrentWrite);
        }

        let mut statement = transaction
            .prepare(
                "SELECT workstream_id, revision FROM workstreams
                 WHERE location_id = ?1 AND origin = 'external'
                 ORDER BY workstream_id LIMIT 2",
            )
            .map_err(StateError::Sqlite)?;
        let anchors = statement
            .query_map([location_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        drop(statement);
        let [(source_workstream, source_revision)] = anchors.as_slice() else {
            return Err(StateError::MalformedHostSchema);
        };
        let source_workstream_id = Uuid::parse_str(source_workstream)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let source_revision = Revision::try_from(*source_revision)?;
        let workstream_id = WorkstreamId::new();
        let activity_sequence = next_activity_sequence(&transaction)?;
        transaction
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin, source_workstream_id,
                    lifecycle, archived_at_millis, last_activity_sequence,
                    last_activity_at_millis, revision
                 ) VALUES (?1, ?2, ?3, 'independent', ?4, 'open', NULL, ?5, 0, 1)",
                params![
                    workstream_id.to_string(),
                    location_id.to_string(),
                    provider.as_str(),
                    source_workstream_id.to_string(),
                    activity_sequence,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO independent_creation_requests (
                    request_key, source_workstream_id, source_revision, workstream_id
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    request_key,
                    source_workstream_id.to_string(),
                    source_revision.value(),
                    workstream_id.to_string(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        let created = CreatedWorkstream {
            workstream_id,
            location_id,
            provider,
            origin: WorkstreamOrigin::Independent,
            source_workstream_id,
            revision: Revision::INITIAL,
        };
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(created)
    }

    /// Lists only durable non-onboarding creation operations that still
    /// require an explicit operator decision for the public operations
    /// diagnostic. This is presentation metadata, not Navigator recovery or
    /// provider/project-root discovery.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded operation projection cannot be read
    /// or contains an invalid persisted identity, kind, phase, or revision.
    pub fn unresolved_operation_overviews(&self) -> Result<Vec<OperationOverview>, StateError> {
        // Retain one SQLite snapshot across every bounded page so a concurrent
        // operation transition cannot move a row across OFFSET boundaries.
        let _read_snapshot = self
            .connection
            .is_autocommit()
            .then(|| self.connection.unchecked_transaction())
            .transpose()
            .map_err(StateError::Sqlite)?;
        let mut cursor = 0;
        let mut operations = Vec::new();
        loop {
            let page =
                self.unresolved_operation_overview_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
            operations.extend(page.operations);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(operations);
            };
            cursor = next_cursor;
        }
    }

    /// Reads one deterministic bounded page of unresolved non-onboarding
    /// operations.  The complete-list API above deliberately hides this
    /// cursor from callers while retaining the database I/O bound.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid page size, cursor overflow, malformed
    /// persisted operation state, or an unavailable registry.
    pub fn unresolved_operation_overview_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<OperationOverviewPage, StateError> {
        if page_size == 0 || page_size > MAX_NAVIGATOR_WORKSTREAMS {
            return Err(StateError::InvalidNavigatorPageSize);
        }
        let query_limit =
            i64::try_from(page_size).map_err(|_| StateError::InvalidNavigatorPageSize)?;
        let cursor_step =
            u32::try_from(page_size).map_err(|_| StateError::InvalidNavigatorPageSize)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT operation_id, kind, phase, effect_watermark, revision
                 FROM compound_operations
                 WHERE kind != 'onboard'
                   AND phase IN ('external_effect_started', 'awaiting_reconciliation', 'recovery_required')
                 ORDER BY operation_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(StateError::Sqlite)?;
        let rows = statement
            .query_map([query_limit + 1, i64::from(cursor)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        let has_more = rows.len() > page_size;
        let operations = rows
            .into_iter()
            .take(page_size)
            .map(|(operation_id, kind, phase, effect_watermark, revision)| {
                let kind = operation_kind_from_text(&kind)?;
                let provider = match kind {
                    OperationKind::Onboard => {
                        return Err(StateError::OnboardingOperationUnavailable);
                    }
                    OperationKind::Start => {
                        PersistedOpenCodeSessionCreationPlan::decode(effect_watermark.as_deref())?
                            .provider
                    }
                };
                Ok(OperationOverview {
                    operation_id: Uuid::parse_str(&operation_id)
                        .map(OperationId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    kind,
                    provider,
                    phase: operation_phase_from_text(&phase)?,
                    revision: Revision::try_from(revision)?,
                })
            })
            .collect::<Result<Vec<_>, StateError>>()?;
        let next_cursor = if has_more {
            Some(
                cursor
                    .checked_add(cursor_step)
                    .ok_or(StateError::NavigatorCursorOverflow)?,
            )
        } else {
            None
        };
        Ok(OperationOverviewPage {
            operations,
            next_cursor,
        })
    }

    /// Creates a durable operation or returns the operation for the request key.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation is invalid, state cannot be read or
    /// written, or a previous request key cannot be resolved.
    pub fn create_or_get_operation(
        &mut self,
        request_key: String,
        kind: OperationKind,
        expected_revisions_json: String,
    ) -> Result<(CompoundOperation, bool), StateError> {
        self.create_or_get_operation_with_id_generator(
            request_key,
            kind,
            expected_revisions_json,
            &RandomIdGenerator,
        )
    }

    /// Creates or gets an operation with an injected identity source.
    ///
    /// This is a deterministic seam for recovery fixtures. Production callers
    /// should use [`Self::create_or_get_operation`].
    ///
    /// # Errors
    ///
    /// Returns an error if the operation is invalid, state cannot be read or
    /// written, or a previous request key cannot be resolved.
    pub fn create_or_get_operation_with_id_generator(
        &mut self,
        request_key: String,
        kind: OperationKind,
        expected_revisions_json: String,
        id_generator: &dyn IdGenerator,
    ) -> Result<(CompoundOperation, bool), StateError> {
        let candidate = CompoundOperation::with_id(
            OperationId::from(id_generator.uuid()),
            request_key,
            kind,
            expected_revisions_json,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;

        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO compound_operations (
                    operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    candidate.id.to_string(),
                    candidate.request_key,
                    operation_kind_text(candidate.kind),
                    operation_phase_text(candidate.phase),
                    candidate.expected_revisions_json,
                    candidate.effect_watermark,
                    candidate.outcome_json,
                    candidate.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;

        let operation = if inserted == 1 {
            candidate
        } else {
            load_operation_by_request_key(&transaction, &candidate.request_key)?
                .ok_or_else(|| StateError::MissingOperation(candidate.request_key.clone()))?
        };
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok((operation, inserted == 1))
    }

    /// Journals one exact `OpenCode` blank-session creation before any
    /// provider request is made. Reusing the same Runtime generation returns
    /// the original operation and never creates a second journal entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the Runtime is unknown, not an `OpenCode`
    /// `starting` Runtime, stale for the supplied generation, or state cannot
    /// be committed.
    pub fn prepare_opencode_session_creation(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        validate_registry_text("runtime generation", expected_generation)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (workstream_id, provider, generation, lifecycle): (String, String, String, String) =
            transaction
                .query_row(
                    "SELECT workstream_id, provider, tmux_generation, lifecycle
                     FROM runtimes WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(StateError::Sqlite)?
                .ok_or(StateError::UnknownRuntime(runtime_id))?;
        if provider_kind_from_text(&provider)? != ProviderKind::OpenCode
            || generation != expected_generation
            || lifecycle != "starting"
        {
            return Err(StateError::HookEvidenceMismatch);
        }
        let workstream_id = Uuid::parse_str(&workstream_id)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let request_key = opencode_session_creation_request_key(runtime_id, expected_generation);
        if let Some(operation) = load_operation_by_request_key(&transaction, &request_key)? {
            let plan = PersistedOpenCodeSessionCreationPlan::decode(
                operation.effect_watermark.as_deref(),
            )?;
            validate_opencode_session_creation_operation_identity(
                &operation,
                &plan,
                runtime_id,
                workstream_id,
                expected_generation,
            )?;
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(plan.public_plan(operation));
        }
        let plan = PersistedOpenCodeSessionCreationPlan {
            schema_version: OPENCODE_SESSION_CREATION_PLAN_SCHEMA_VERSION,
            provider: ProviderKind::OpenCode,
            runtime_id,
            workstream_id,
            runtime_generation: expected_generation.to_owned(),
            native_session_id: None,
        };
        let mut operation = CompoundOperation::new(
            request_key,
            OperationKind::Start,
            serde_json::json!({
                "runtime_id": runtime_id,
                "runtime_generation": expected_generation,
            })
            .to_string(),
        )?;
        operation.effect_watermark = Some(plan.encode()?);
        transaction
            .execute(
                "INSERT INTO compound_operations (
                    operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                params![
                    operation.id.to_string(),
                    operation.request_key,
                    operation_kind_text(operation.kind),
                    operation_phase_text(operation.phase),
                    operation.expected_revisions_json,
                    operation.effect_watermark,
                    operation.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(plan.public_plan(operation))
    }

    /// Atomically marks the exact prepared operation immediately before the
    /// provider's non-idempotent `POST /session` boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the operation is stale, already crossed the
    /// provider boundary, or the exact Runtime generation cannot be validated.
    pub fn begin_opencode_session_creation(
        &mut self,
        prepared: &OpenCodeSessionCreationOperation,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (mut operation, plan) = load_exact_opencode_session_creation(&transaction, prepared)?;
        if operation.phase != OperationPhase::Prepared || plan.native_session_id.is_some() {
            return Err(StateError::OpenCodeSessionCreationUnavailable);
        }
        validate_opencode_session_creation_runtime(&transaction, &plan)?;
        let effect_watermark = Some(plan.encode()?);
        operation.transition(
            OperationPhase::ExternalEffectStarted,
            effect_watermark,
            None,
        )?;
        update_operation_in_transaction(&transaction, &operation, prepared.operation.revision)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(plan.public_plan(operation))
    }

    /// Atomically commits the verified native session ID and the journal's
    /// terminal `Committed` phase. The Runtime generation and operation
    /// revision are checked in the same transaction as the binding insert.
    ///
    /// # Errors
    ///
    /// Returns an error when the session provider, operation, Runtime
    /// generation, or optimistic revision does not match the prepared plan.
    pub fn commit_opencode_session_creation(
        &mut self,
        prepared: &OpenCodeSessionCreationOperation,
        session: &ProviderSessionId,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        if session.provider() != ProviderKind::OpenCode {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (mut operation, mut plan) =
            load_exact_opencode_session_creation(&transaction, prepared)?;
        if operation.phase != OperationPhase::ExternalEffectStarted
            || plan.native_session_id.is_some()
        {
            return Err(StateError::OpenCodeSessionCreationUnavailable);
        }
        validate_opencode_session_creation_runtime(&transaction, &plan)?;
        bind_opencode_session_in_transaction(
            &transaction,
            plan.runtime_id,
            &plan.runtime_generation,
            session,
            "new",
        )?;
        plan.native_session_id = Some(session.clone());
        operation.transition(OperationPhase::Committed, Some(plan.encode()?), None)?;
        update_operation_in_transaction(&transaction, &operation, prepared.operation.revision)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(plan.public_plan(operation))
    }

    /// Terminally fails an exact prepared operation before the provider
    /// boundary. The bounded outcome code is diagnostic metadata only; it
    /// never stores a provider error payload.
    ///
    /// # Errors
    ///
    /// Returns an error when the outcome code is invalid, the operation is
    /// stale or already crossed the boundary, or state cannot be committed.
    pub fn fail_opencode_session_creation(
        &mut self,
        prepared: &OpenCodeSessionCreationOperation,
        outcome_code: &str,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        validate_operation_outcome_code(outcome_code)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (mut operation, plan) = load_exact_opencode_session_creation(&transaction, prepared)?;
        if operation.phase != OperationPhase::Prepared {
            return Err(StateError::OpenCodeSessionCreationUnavailable);
        }
        validate_opencode_session_creation_runtime(&transaction, &plan)?;
        let outcome = serde_json::json!({"code": outcome_code}).to_string();
        operation.transition(OperationPhase::Failed, Some(plan.encode()?), Some(outcome))?;
        update_operation_in_transaction(&transaction, &operation, prepared.operation.revision)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(plan.public_plan(operation))
    }

    /// Terminally records that the crossed provider boundary is unknown. The
    /// exact Starting Runtime is moved to `unknown`, its Workstream becomes
    /// recovery-required, and the operation is no longer
    /// eligible for retry. All state changes commit in one `SQLite` transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the operation is stale, has not crossed the
    /// provider boundary, or the exact Runtime transition cannot be committed.
    pub fn mark_opencode_session_creation_unknown(
        &mut self,
        prepared: &OpenCodeSessionCreationOperation,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        self.mark_opencode_session_creation_recovery(
            prepared,
            OperationPhase::ExternalEffectStarted,
            OPENCODE_SESSION_CREATION_UNKNOWN_CODE,
        )
    }

    /// Terminally records that the short-lived provider helper could not be
    /// cleaned up before the non-idempotent boundary. The exact Starting
    /// Runtime is moved to `unknown` so cleanup ambiguity is never retryable.
    ///
    /// # Errors
    ///
    /// Returns an error when the prepared operation is stale or the exact
    /// Runtime transition cannot be committed.
    pub fn mark_opencode_session_creation_cleanup_unknown(
        &mut self,
        prepared: &OpenCodeSessionCreationOperation,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        self.mark_opencode_session_creation_recovery(
            prepared,
            OperationPhase::Prepared,
            OPENCODE_SESSION_CREATION_CLEANUP_UNKNOWN_CODE,
        )
    }

    fn mark_opencode_session_creation_recovery(
        &mut self,
        prepared: &OpenCodeSessionCreationOperation,
        expected_phase: OperationPhase,
        outcome_code: &str,
    ) -> Result<OpenCodeSessionCreationOperation, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (mut operation, plan) = load_exact_opencode_session_creation(&transaction, prepared)?;
        if operation.phase != expected_phase {
            return Err(StateError::OpenCodeSessionCreationUnavailable);
        }
        validate_opencode_session_creation_runtime(&transaction, &plan)?;
        let runtime_changed = transaction
            .execute(
                "UPDATE runtimes SET lifecycle = 'unknown', revision = revision + 1
                 WHERE runtime_id = ?1 AND tmux_generation = ?2 AND lifecycle = 'starting'",
                params![plan.runtime_id.to_string(), &plan.runtime_generation],
            )
            .map_err(StateError::Sqlite)?;
        if runtime_changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        let activity_sequence = next_activity_sequence(&transaction)?;
        let workstream_changed = transaction
            .execute(
                "UPDATE workstreams SET lifecycle = 'recovery_required',
                 last_activity_sequence = ?1, revision = revision + 1
                 WHERE workstream_id = ?2 AND lifecycle IN ('open', 'parked')",
                params![activity_sequence, plan.workstream_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        if workstream_changed == 0 {
            let lifecycle: String = transaction
                .query_row(
                    "SELECT lifecycle FROM workstreams WHERE workstream_id = ?1",
                    [plan.workstream_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(StateError::Sqlite)?
                .ok_or(StateError::UnknownOpenWorkstream(plan.workstream_id))?;
            if lifecycle != "recovery_required" {
                return Err(StateError::ConcurrentWrite);
            }
        }
        operation.transition(
            OperationPhase::Failed,
            Some(plan.encode()?),
            Some(serde_json::json!({"code": outcome_code}).to_string()),
        )?;
        update_operation_in_transaction(&transaction, &operation, prepared.operation.revision)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(plan.public_plan(operation))
    }

    /// Reads the exact journal for one Runtime generation without adopting a
    /// different operation or provider session.
    ///
    /// # Errors
    ///
    /// Returns an error when the generation is invalid, journal identity is
    /// inconsistent, or state cannot be read.
    pub fn opencode_session_creation_for_runtime(
        &self,
        runtime_id: RuntimeId,
        expected_generation: &str,
    ) -> Result<Option<OpenCodeSessionCreationOperation>, StateError> {
        validate_registry_text("runtime generation", expected_generation)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let request_key = opencode_session_creation_request_key(runtime_id, expected_generation);
        let operation = load_operation_by_request_key(&transaction, &request_key)?;
        let result = operation
            .map(
                |operation| -> Result<OpenCodeSessionCreationOperation, StateError> {
                    let plan = PersistedOpenCodeSessionCreationPlan::decode(
                        operation.effect_watermark.as_deref(),
                    )?;
                    validate_opencode_session_creation_operation_identity(
                        &operation,
                        &plan,
                        runtime_id,
                        runtime_workstream_id(&transaction, runtime_id)?,
                        expected_generation,
                    )?;
                    Ok(plan.public_plan(operation))
                },
            )
            .transpose()?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(result)
    }

    /// Returns whether an exact Runtime generation has an unresolved blank
    /// session creation operation. `Prepared` is unresolved here even though
    /// it has not crossed the provider boundary: if the action disappears
    /// before its normal cleanup path, the host has no proof that the
    /// short-lived provider helper is gone. Only terminal `Failed` and
    /// `Committed` operations are settled and eligible for a fresh Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the generation is invalid, journal identity is
    /// inconsistent, or state cannot be read.
    pub fn has_unresolved_opencode_session_creation(
        &self,
        runtime_id: RuntimeId,
        expected_generation: &str,
    ) -> Result<bool, StateError> {
        Ok(self
            .opencode_session_creation_for_runtime(runtime_id, expected_generation)?
            .is_some_and(|operation| {
                matches!(
                    operation.operation.phase,
                    OperationPhase::Prepared
                        | OperationPhase::ExternalEffectStarted
                        | OperationPhase::AwaitingReconciliation
                        | OperationPhase::RecoveryRequired
                )
            }))
    }

    /// Advances an operation with an optimistic revision guard.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale revision, an invalid transition, missing
    /// operation, or failed state transaction.
    pub fn transition_operation(
        &mut self,
        operation_id: OperationId,
        expected_revision: Revision,
        next_phase: OperationPhase,
        effect_watermark: Option<String>,
        outcome_json: Option<String>,
    ) -> Result<CompoundOperation, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let mut operation = load_operation_by_id(&transaction, operation_id)?
            .ok_or(StateError::UnknownOperation(operation_id))?;
        if operation.revision != expected_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: operation.revision,
            }));
        }
        operation.transition(next_phase, effect_watermark, outcome_json)?;
        let updated = transaction
            .execute(
                "UPDATE compound_operations
                 SET phase = ?1, effect_watermark = ?2, outcome_json = ?3, revision = ?4
                 WHERE operation_id = ?5 AND revision = ?6",
                params![
                    operation_phase_text(operation.phase),
                    operation.effect_watermark,
                    operation.outcome_json,
                    operation.revision.value(),
                    operation.id.to_string(),
                    expected_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(operation)
    }
}

fn load_source_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
    include_archived: bool,
) -> Result<(LocationId, Revision), StateError> {
    let row = transaction
        .query_row(
            "SELECT location_id, archived_at_millis, revision
             FROM workstreams WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
    if row.1.is_some() && !include_archived {
        return Err(StateError::WorkstreamArchived(workstream_id));
    }
    Ok((
        Uuid::parse_str(&row.0)
            .map(LocationId::from)
            .map_err(StateError::InvalidPersistedUuid)?,
        Revision::try_from(row.2)?,
    ))
}

pub(in crate::state) fn created_workstream_from_record(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<CreatedWorkstream, StateError> {
    let record = transaction
        .query_row(
            "SELECT location_id, provider, origin, source_workstream_id, revision
             FROM workstreams WHERE workstream_id = ?1",
            [workstream_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::MalformedHostSchema)?;
    let location_id = Uuid::parse_str(&record.0)
        .map(LocationId::from)
        .map_err(StateError::InvalidPersistedUuid)?;
    let provider = provider_kind_from_text(&record.1)?;
    let source_workstream_id = record
        .3
        .as_deref()
        .ok_or(StateError::MalformedHostSchema)
        .and_then(|value| {
            Uuid::parse_str(value)
                .map(WorkstreamId::from)
                .map_err(StateError::InvalidPersistedUuid)
        })?;
    let origin = match record.2.as_str() {
        "independent" => WorkstreamOrigin::Independent,
        // Retained solely so historical Fork-origin records continue to
        // decode as inert provenance. No Fork operation can be created.
        "fork" => WorkstreamOrigin::Fork,
        _ => return Err(StateError::MalformedHostSchema),
    };
    Ok(CreatedWorkstream {
        workstream_id,
        location_id,
        provider,
        origin,
        source_workstream_id,
        revision: Revision::try_from(record.4)?,
    })
}
pub(in crate::state) fn load_operation_by_request_key(
    transaction: &rusqlite::Transaction<'_>,
    request_key: &str,
) -> Result<Option<CompoundOperation>, StateError> {
    let operation = transaction
        .query_row(
            "SELECT operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
             FROM compound_operations WHERE request_key = ?1",
            [request_key],
            row_to_operation,
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(operation)
}

pub(in crate::state) fn load_operation_by_id(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
) -> Result<Option<CompoundOperation>, StateError> {
    let operation = transaction
        .query_row(
            "SELECT operation_id, request_key, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json, revision
             FROM compound_operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            row_to_operation,
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    Ok(operation)
}

pub(in crate::state) fn opencode_session_creation_request_key(
    runtime_id: RuntimeId,
    generation: &str,
) -> String {
    format!("opencode-session:{runtime_id}:{generation}")
}

pub(in crate::state) fn validate_opencode_session_creation_operation_identity(
    operation: &CompoundOperation,
    plan: &PersistedOpenCodeSessionCreationPlan,
    runtime_id: RuntimeId,
    workstream_id: WorkstreamId,
    generation: &str,
) -> Result<(), StateError> {
    if operation.kind != OperationKind::Start
        || operation.request_key != opencode_session_creation_request_key(runtime_id, generation)
        || plan.runtime_id != runtime_id
        || plan.workstream_id != workstream_id
        || plan.runtime_generation != generation
    {
        return Err(StateError::OpenCodeSessionCreationUnavailable);
    }
    Ok(())
}

pub(in crate::state) fn runtime_workstream_id(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<WorkstreamId, StateError> {
    let value: String = transaction
        .query_row(
            "SELECT workstream_id FROM runtimes WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    Uuid::parse_str(&value)
        .map(WorkstreamId::from)
        .map_err(StateError::InvalidPersistedUuid)
}

pub(in crate::state) fn load_exact_opencode_session_creation(
    transaction: &rusqlite::Transaction<'_>,
    expected: &OpenCodeSessionCreationOperation,
) -> Result<(CompoundOperation, PersistedOpenCodeSessionCreationPlan), StateError> {
    let operation = load_operation_by_id(transaction, expected.operation.id)?
        .ok_or(StateError::UnknownOperation(expected.operation.id))?;
    if operation.revision != expected.operation.revision {
        return Err(StateError::Domain(DomainError::RevisionConflict {
            expected: expected.operation.revision,
            current: operation.revision,
        }));
    }
    let plan = PersistedOpenCodeSessionCreationPlan::decode(operation.effect_watermark.as_deref())?;
    if operation != expected.operation
        || plan.public_plan(operation.clone()) != *expected
        || operation.kind != OperationKind::Start
    {
        return Err(StateError::OpenCodeSessionCreationUnavailable);
    }
    validate_opencode_session_creation_operation_identity(
        &operation,
        &plan,
        expected.runtime_id,
        expected.workstream_id,
        &expected.runtime_generation,
    )?;
    Ok((operation, plan))
}

pub(in crate::state) fn validate_opencode_session_creation_runtime(
    transaction: &rusqlite::Transaction<'_>,
    plan: &PersistedOpenCodeSessionCreationPlan,
) -> Result<(), StateError> {
    let (workstream_id, provider, generation, lifecycle): (String, String, String, String) =
        transaction
            .query_row(
                "SELECT workstream_id, provider, tmux_generation, lifecycle
                 FROM runtimes WHERE runtime_id = ?1",
                [plan.runtime_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownRuntime(plan.runtime_id))?;
    if provider_kind_from_text(&provider)? != ProviderKind::OpenCode
        || generation != plan.runtime_generation
        || lifecycle != "starting"
    {
        return Err(StateError::HookEvidenceMismatch);
    }
    let workstream_id = Uuid::parse_str(&workstream_id)
        .map(WorkstreamId::from)
        .map_err(StateError::InvalidPersistedUuid)?;
    if workstream_id != plan.workstream_id {
        return Err(StateError::ProviderIdentityMismatch);
    }
    Ok(())
}

pub(in crate::state) fn update_operation_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    operation: &CompoundOperation,
    expected_revision: Revision,
) -> Result<(), StateError> {
    let updated = transaction
        .execute(
            "UPDATE compound_operations
             SET phase = ?1, effect_watermark = ?2, outcome_json = ?3, revision = ?4
             WHERE operation_id = ?5 AND revision = ?6",
            params![
                operation_phase_text(operation.phase),
                operation.effect_watermark,
                operation.outcome_json,
                operation.revision.value(),
                operation.id.to_string(),
                expected_revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

pub(in crate::state) fn validate_operation_outcome_code(value: &str) -> Result<(), StateError> {
    validate_registry_text("operation outcome", value)
}

pub(in crate::state) fn bind_opencode_session_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    expected_generation: &str,
    session: &ProviderSessionId,
    start_source: &str,
) -> Result<ProviderBinding, StateError> {
    if session.provider() != ProviderKind::OpenCode || !matches!(start_source, "new" | "resume") {
        return Err(StateError::ProviderIdentityMismatch);
    }
    validate_registry_text("runtime generation", expected_generation)?;
    validate_registry_text("start source", start_source)?;
    let (provider, generation, lifecycle): (String, String, String) = transaction
        .query_row(
            "SELECT provider, tmux_generation, lifecycle FROM runtimes WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::UnknownRuntime(runtime_id))?;
    if provider_kind_from_text(&provider)? != ProviderKind::OpenCode
        || generation != expected_generation
        || lifecycle != "starting"
    {
        return Err(StateError::HookEvidenceMismatch);
    }
    let existing = load_binding(transaction, runtime_id)?;
    let binding = if let Some(existing) = existing {
        if existing.provider != ProviderKind::OpenCode || existing.native_session_id != *session {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let changed = transaction
            .execute(
                "UPDATE provider_bindings SET runtime_generation = ?1,
                    start_source = ?2, revision = revision + 1
                 WHERE runtime_id = ?3 AND runtime_generation != ?1",
                params![expected_generation, start_source, runtime_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 0 {
            existing
        } else {
            load_binding(transaction, runtime_id)?.ok_or(StateError::ConcurrentWrite)?
        }
    } else {
        transaction
            .execute(
                "INSERT INTO provider_bindings (
                    binding_id, runtime_id, provider, native_session_id, start_source,
                    last_settled_turn_id, observed_thread_name, name_state,
                    name_observed_at, predecessor_native_session_id,
                    predecessor_effective_name, runtime_generation, revision
                 ) VALUES (?1, ?2, 'opencode', ?3, ?4, NULL, NULL,
                    'unavailable', NULL, NULL, NULL, ?5, 1)",
                params![
                    Uuid::new_v4().to_string(),
                    runtime_id.to_string(),
                    session.native_id(),
                    start_source,
                    expected_generation,
                ],
            )
            .map_err(StateError::Sqlite)?;
        load_binding(transaction, runtime_id)?.ok_or(StateError::ConcurrentWrite)?
    };
    Ok(binding)
}

pub(in crate::state) fn row_to_operation(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CompoundOperation> {
    let id: String = row.get(0)?;
    let kind: String = row.get(2)?;
    let phase: String = row.get(3)?;
    let revision: i64 = row.get(7)?;
    Ok(CompoundOperation {
        id: Uuid::parse_str(&id)
            .map(OperationId::from)
            .map_err(to_from_sql_error)?,
        request_key: row.get(1)?,
        kind: operation_kind_from_text(&kind).map_err(to_from_sql_error)?,
        phase: operation_phase_from_text(&phase).map_err(to_from_sql_error)?,
        expected_revisions_json: row.get(4)?,
        launch_token_id: None,
        launch_token_verifier: None,
        launch_token_expiry_monotonic: None,
        launch_claims_digest: None,
        effect_watermark: row.get(5)?,
        outcome_json: row.get(6)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}
