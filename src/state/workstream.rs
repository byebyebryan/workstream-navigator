use std::path::PathBuf;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{
    DomainError, LocationId, OperationKind, OperationPhase, ProviderKind, Revision, RuntimeId,
    RuntimeStatus, WorkstreamId,
};

use super::current::PARKED_RECOVERY_RESOLVED_OUTCOME;
use super::models::{
    CatalogAuthorization, HostRegistry, PersistedOpenCodeSessionCreationPlan,
    PersistedWorkstreamOverview, StateError, WorkstreamOverview, WorkstreamOverviewPage,
};
use super::schema::{MAX_NAVIGATOR_WORKSTREAMS, validate_foreign_keys};
use super::utils::{
    operation_kind_from_text, operation_phase_from_text, provider_kind_from_text,
    workstream_lifecycle_from_text,
};

impl HostRegistry {
    /// Returns the bounded state needed by one local navigator snapshot.
    /// Provider content, terminal captures, and hook payloads are not queried
    /// or returned.
    ///
    /// # Errors
    ///
    /// Returns an error when a persisted identity, lifecycle, or revision is
    /// malformed, or when the registry cannot be queried.
    pub fn workstream_overviews(&self) -> Result<Vec<WorkstreamOverview>, StateError> {
        let bases = {
            // Keep OFFSET pages on one SQLite read snapshot so concurrent
            // lifecycle activity cannot reorder a row between page reads.
            // Hydration follows after the read transaction because the
            // existing binding readers own their own exact transactions.
            let _read_snapshot = self
                .connection
                .is_autocommit()
                .then(|| self.connection.unchecked_transaction())
                .transpose()
                .map_err(StateError::Sqlite)?;
            let mut bases = Vec::new();
            let mut cursor = 0;
            loop {
                let (page, next_cursor) =
                    self.persisted_workstream_overview_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
                bases.extend(page);
                let Some(next_cursor) = next_cursor else {
                    break bases;
                };
                cursor = next_cursor;
            }
        };
        bases
            .into_iter()
            .map(|base| self.hydrate_workstream_overview(base))
            .collect()
    }

    /// Returns one deterministic bounded Workstream page ordered by latest
    /// activity, project root, and opaque Workstream identity.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid page size, cursor overflow, malformed
    /// persisted state, or an unavailable registry.
    pub fn workstream_overview_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<WorkstreamOverviewPage, StateError> {
        let (bases, next_cursor) = self.persisted_workstream_overview_page(cursor, page_size)?;
        let workstreams = bases
            .into_iter()
            .map(|base| self.hydrate_workstream_overview(base))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkstreamOverviewPage {
            workstreams,
            next_cursor,
        })
    }

    fn persisted_workstream_overview_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<(Vec<PersistedWorkstreamOverview>, Option<u32>), StateError> {
        if page_size == 0 || page_size > MAX_NAVIGATOR_WORKSTREAMS {
            return Err(StateError::InvalidNavigatorPageSize);
        }
        let query_limit =
            i64::try_from(page_size + 1).map_err(|_| StateError::InvalidNavigatorPageSize)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT workstreams.workstream_id, workstreams.location_id,
                        workstreams.provider,
                        project_locations.repository_path,
                        project_locations.repository_display_name,
                        project_locations.remote_identity_fingerprint,
                        project_locations.remote_identity_display,
                        workstreams.lifecycle,
                        workstreams.archived_at_millis,
                        workstreams.last_activity_sequence,
                        workstreams.last_activity_at_millis, workstreams.revision
                 FROM workstreams
                 JOIN project_locations
                   ON project_locations.location_id = workstreams.location_id
                 ORDER BY workstreams.last_activity_sequence DESC,
                          project_locations.repository_path, workstreams.workstream_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(StateError::Sqlite)?;
        let mut bases = statement
            .query_map(params![query_limit, i64::from(cursor)], |row| {
                Ok(PersistedWorkstreamOverview {
                    workstream_id: row.get(0)?,
                    location_id: row.get(1)?,
                    provider: row.get(2)?,
                    project_repository_path: row.get(3)?,
                    project_display_name: row.get(4)?,
                    remote_identity_fingerprint: row.get(5)?,
                    remote_identity_display: row.get(6)?,
                    lifecycle: row.get(7)?,
                    archived_at_millis: row.get(8)?,
                    activity_sequence: row.get(9)?,
                    activity_at_millis: row.get(10)?,
                    revision: row.get(11)?,
                })
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        let has_more = bases.len() > page_size;
        bases.truncate(page_size);
        let page_len =
            u32::try_from(bases.len()).map_err(|_| StateError::NavigatorCursorOverflow)?;
        let next_cursor = has_more
            .then(|| {
                cursor
                    .checked_add(page_len)
                    .ok_or(StateError::NavigatorCursorOverflow)
            })
            .transpose()?;
        Ok((bases, next_cursor))
    }

    /// Hides one exact Workstream from the active navigator scope without
    /// deleting its Runtime, provider binding, project files, or
    /// lineage. The caller is responsible for any necessary exact Runtime
    /// stop before this durable visibility transition.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream is missing, already archived, its
    /// revision is stale, the timestamp is invalid, or the transaction fails.
    pub fn archive_workstream(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        archived_at_millis: i64,
    ) -> Result<Revision, StateError> {
        if archived_at_millis < 0 {
            return Err(StateError::InvalidRegistryField("archive timestamp"));
        }
        self.transition_workstream_archive(
            workstream_id,
            expected_revision,
            Some(archived_at_millis),
        )
    }

    /// Returns one archived Workstream to the active navigator scope without
    /// starting or resuming a provider Runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream is missing, not archived, its
    /// revision is stale, or the transaction fails.
    pub fn restore_workstream(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    ) -> Result<Revision, StateError> {
        self.transition_workstream_archive(workstream_id, expected_revision, None)
    }

    /// Performs the read-only ownership/revision checks required before an
    /// exact Runtime stop for [`Self::forget_workstream`].
    pub(crate) fn validate_forget_workstream(
        &self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    ) -> Result<(), StateError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let existing: Option<(i64, Option<i64>)> = transaction
            .query_row(
                "SELECT revision, archived_at_millis
                 FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((revision, archived_at_millis)) = existing else {
            return Err(StateError::UnknownOpenWorkstream(workstream_id));
        };
        let revision = Revision::try_from(revision)?;
        if revision != expected_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: revision,
            }));
        }
        if archived_at_millis.is_none() {
            return Err(StateError::WorkstreamNotArchived(workstream_id));
        }
        let runtime_id = transaction
            .query_row(
                "SELECT runtime_id FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .map(|value| {
                Uuid::parse_str(&value)
                    .map(RuntimeId::from)
                    .map_err(|_| StateError::MalformedHostSchema)
            })
            .transpose()?;
        let _ = forget_operation_ids(&transaction, workstream_id, runtime_id)?;
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Permanently removes one archived Workstream from the `WSNav` catalog.
    ///
    /// This is deliberately narrower than a general record purge.  The
    /// caller must have already completed any exact Runtime stop; this one
    /// transaction then revalidates the archived/revision boundary, refuses
    /// unresolved or shared provider-effect journal rows, removes only the
    /// selected Workstream's WSNav-owned graph, and severs nullable child
    /// lineage. Provider-native history, Project/Location rows, and child
    /// Workstreams remain untouched.
    ///
    /// # Errors
    ///
    /// Returns an error when the Workstream is unknown, stale, active, or has
    /// ambiguous/unresolved operation ownership. In every error case the
    /// transaction is rolled back and the Workstream is retained.
    #[allow(
        clippy::too_many_lines,
        reason = "destructive graph ownership checks and deletes stay in one auditable transaction"
    )]
    pub fn forget_workstream(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
    ) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let existing: Option<(i64, Option<i64>)> = transaction
            .query_row(
                "SELECT revision, archived_at_millis
                 FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((revision, archived_at_millis)) = existing else {
            return Err(StateError::UnknownOpenWorkstream(workstream_id));
        };
        let revision = Revision::try_from(revision)?;
        if revision != expected_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: revision,
            }));
        }
        if archived_at_millis.is_none() {
            return Err(StateError::WorkstreamNotArchived(workstream_id));
        }

        let runtime_id = transaction
            .query_row(
                "SELECT runtime_id FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .map(|value| {
                Uuid::parse_str(&value)
                    .map(RuntimeId::from)
                    .map_err(|_| StateError::MalformedHostSchema)
            })
            .transpose()?;
        let operation_ids = forget_operation_ids(&transaction, workstream_id, runtime_id)?;

        // A child may outlive its source Workstream.  Remove only the nullable
        // lineage edge, and advance that child's revision so stale snapshots
        // cannot silently act on the changed provenance.
        transaction
            .execute(
                "UPDATE workstreams SET source_workstream_id = NULL,
                        revision = revision + 1
                 WHERE source_workstream_id = ?1",
                [workstream_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;

        if let Some(runtime_id) = runtime_id {
            transaction
                .execute(
                    "DELETE FROM opencode_settled_messages WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            transaction
                .execute(
                    "DELETE FROM opencode_runtime_handles WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            transaction
                .execute(
                    "DELETE FROM provider_bindings WHERE runtime_id = ?1",
                    [runtime_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            let deleted = transaction
                .execute(
                    "DELETE FROM runtimes
                     WHERE runtime_id = ?1 AND workstream_id = ?2",
                    params![runtime_id.to_string(), workstream_id.to_string()],
                )
                .map_err(StateError::Sqlite)?;
            if deleted != 1 {
                return Err(StateError::WorkstreamForgetRefused);
            }
        }

        transaction
            .execute(
                "DELETE FROM attention_states WHERE workstream_id = ?1",
                [workstream_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;

        // The selected row's own creation request is catalog metadata. Any
        // requests that named it as a source must also be removed because the
        // schema-15 source FK is non-null; their child lineage was severed
        // above, while the child Workstreams themselves remain.
        transaction
            .execute(
                "DELETE FROM independent_creation_requests
                 WHERE workstream_id = ?1 OR source_workstream_id = ?1",
                [workstream_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;

        for operation_id in operation_ids {
            transaction
                .execute(
                    "DELETE FROM onboarding_exec_targets WHERE operation_id = ?1",
                    [operation_id.as_str()],
                )
                .map_err(StateError::Sqlite)?;
            transaction
                .execute(
                    "DELETE FROM compound_operations WHERE operation_id = ?1",
                    [operation_id.as_str()],
                )
                .map_err(StateError::Sqlite)?;
        }

        let deleted = transaction
            .execute(
                "DELETE FROM workstreams
                 WHERE workstream_id = ?1 AND archived_at_millis IS NOT NULL
                   AND revision = ?2",
                params![workstream_id.to_string(), expected_revision.value()],
            )
            .map_err(StateError::Sqlite)?;
        if deleted != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        validate_foreign_keys(&transaction)?;
        transaction.commit().map_err(StateError::Sqlite)
    }

    fn transition_workstream_archive(
        &mut self,
        workstream_id: WorkstreamId,
        expected_revision: Revision,
        archived_at_millis: Option<i64>,
    ) -> Result<Revision, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let existing = transaction
            .query_row(
                "SELECT revision, archived_at_millis, lifecycle
                 FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
        let current_revision = Revision::try_from(existing.0)?;
        if current_revision != expected_revision {
            return Err(StateError::Domain(DomainError::RevisionConflict {
                expected: expected_revision,
                current: current_revision,
            }));
        }
        match (existing.1, archived_at_millis) {
            (Some(_), Some(_)) => return Err(StateError::WorkstreamAlreadyArchived(workstream_id)),
            (None, None) => return Err(StateError::WorkstreamNotArchived(workstream_id)),
            (None, Some(_)) | (Some(_), None) => {}
        }
        let next_revision = current_revision.next();
        let lifecycle = if archived_at_millis.is_none() && existing.2 == "parked" {
            "open"
        } else {
            existing.2.as_str()
        };
        let updated = transaction
            .execute(
                "UPDATE workstreams
                 SET archived_at_millis = ?1,
                     lifecycle = ?2,
                     revision = ?3
                 WHERE workstream_id = ?4 AND revision = ?5",
                params![
                    archived_at_millis,
                    lifecycle,
                    next_revision.value(),
                    workstream_id.to_string(),
                    current_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if updated != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(next_revision)
    }

    fn hydrate_workstream_overview(
        &self,
        base: PersistedWorkstreamOverview,
    ) -> Result<WorkstreamOverview, StateError> {
        let workstream_id = Uuid::parse_str(&base.workstream_id)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let location_id = Uuid::parse_str(&base.location_id)
            .map(LocationId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let lifecycle = workstream_lifecycle_from_text(&base.lifecycle)?;
        let provider = provider_kind_from_text(&base.provider)?;
        let revision = Revision::try_from(base.revision)?;
        let runtime = self.runtime_for_workstream(workstream_id)?;
        let binding = match runtime.as_ref() {
            None => None,
            Some(runtime)
                if runtime.provider == ProviderKind::Codex
                    && matches!(
                        runtime.status,
                        RuntimeStatus::Stopped | RuntimeStatus::Unknown
                    ) =>
            {
                self.retained_codex_binding_for_runtime(runtime.runtime_id)?
            }
            Some(runtime) => match self.binding_for_runtime(runtime.runtime_id) {
                Ok(binding) => binding,
                // A resumed Runtime deliberately retains its old exact
                // binding until exact live confirmation or the matching
                // SessionStart corroborates the new generation. Do not
                // project that stale binding into a snapshot while the
                // Runtime is still starting.
                Err(StateError::HookEvidenceMismatch)
                    if runtime.status == RuntimeStatus::Starting =>
                {
                    None
                }
                Err(error) => return Err(error),
            },
        };
        Ok(WorkstreamOverview {
            workstream_id,
            location_id,
            provider,
            project_repository_path: PathBuf::from(base.project_repository_path),
            project_display_name: base.project_display_name,
            remote_identity_fingerprint: base
                .remote_identity_fingerprint
                .filter(|fingerprint| !fingerprint.is_empty()),
            remote_identity_display: base
                .remote_identity_display
                .filter(|display| !display.is_empty()),
            lifecycle,
            archived_at_millis: base.archived_at_millis,
            last_activity_sequence: base.activity_sequence,
            last_activity_at_millis: (base.activity_at_millis != 0)
                .then_some(base.activity_at_millis),
            revision,
            runtime,
            binding,
        })
    }
}

/// Finds every operation whose exact durable plan belongs to the selected
/// Workstream.  The operation payloads remain private; this helper only
/// returns opaque operation IDs to the deletion transaction.  Any malformed,
/// unresolved, or cross-Workstream Runtime relationship is a closed refusal.
fn forget_operation_ids(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
    runtime_id: Option<RuntimeId>,
) -> Result<Vec<String>, StateError> {
    let mut statement = transaction
        .prepare(
            "SELECT operation_id, kind, phase, expected_revisions_json,
                    effect_watermark, outcome_json
             FROM compound_operations ORDER BY operation_id",
        )
        .map_err(StateError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    drop(statement);

    let mut operation_ids = Vec::new();
    for (
        operation_id,
        kind_text,
        phase_text,
        expected_revisions_json,
        effect_watermark,
        outcome_json,
    ) in rows
    {
        let operation_id =
            Uuid::parse_str(&operation_id).map_err(|_| StateError::MalformedHostSchema)?;
        let phase = operation_phase_from_text(&phase_text)?;
        // Schema 15 retains only completed/failed Fork rows as inert history.
        // Any other retired Fork phase is unresolved provider-effect evidence,
        // so retain the selected Workstream rather than guessing ownership.
        let kind = match kind_text.as_str() {
            "onboard" | "start" => operation_kind_from_text(&kind_text)?,
            "fork" if matches!(phase, OperationPhase::Committed | OperationPhase::Failed) => {
                continue;
            }
            _ => return Err(StateError::WorkstreamForgetRefused),
        };
        let (operation_workstream_id, operation_runtime_id) = match kind {
            OperationKind::Onboard => {
                let intent: serde_json::Value = serde_json::from_str(&expected_revisions_json)
                    .map_err(|_| StateError::MalformedHostSchema)?;
                let operation_workstream_id = WorkstreamId::from(persisted_uuid_field(
                    &intent,
                    "workstream_id",
                    StateError::MalformedHostSchema,
                )?);
                let operation_runtime_id = RuntimeId::from(persisted_uuid_field(
                    &intent,
                    "candidate_runtime_id",
                    StateError::MalformedHostSchema,
                )?);
                (operation_workstream_id, operation_runtime_id)
            }
            OperationKind::Start => {
                let plan =
                    PersistedOpenCodeSessionCreationPlan::decode(effect_watermark.as_deref())?;
                (plan.workstream_id, plan.runtime_id)
            }
        };

        let targets_workstream = operation_workstream_id == workstream_id;
        let targets_runtime = runtime_id.is_some_and(|runtime| runtime == operation_runtime_id);
        if targets_runtime && !targets_workstream {
            // A Runtime ID shared by a different Workstream would make the
            // provider effect ownership ambiguous.  Do not delete anything.
            return Err(StateError::WorkstreamForgetRefused);
        }
        if !targets_workstream {
            continue;
        }

        // A stale operation plan must never be treated as selected merely by
        // matching its Workstream field when its Runtime has been reassigned.
        let runtime_owner: Option<String> = transaction
            .query_row(
                "SELECT workstream_id FROM runtimes WHERE runtime_id = ?1",
                [operation_runtime_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        if runtime_owner.is_some_and(|owner| owner != workstream_id.to_string()) {
            return Err(StateError::WorkstreamForgetRefused);
        }
        if !forget_operation_is_terminal(kind, phase, outcome_json.as_deref()) {
            return Err(StateError::WorkstreamForgetRefused);
        }
        operation_ids.push(operation_id.to_string());
    }
    Ok(operation_ids)
}

fn persisted_uuid_field(
    value: &serde_json::Value,
    field: &str,
    error: StateError,
) -> Result<Uuid, StateError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or(error)
        .and_then(|value| Uuid::parse_str(value).map_err(|_| StateError::MalformedHostSchema))
}

fn forget_operation_is_terminal(
    kind: OperationKind,
    phase: OperationPhase,
    outcome_json: Option<&str>,
) -> bool {
    match kind {
        OperationKind::Onboard => match phase {
            OperationPhase::Committed => {
                // `Committed` is reserved for the exact parked-recovery
                // resolution. A generic committed onboarding row would leave
                // provider-effect ownership ambiguous, so refuse it.
                outcome_json == Some(PARKED_RECOVERY_RESOLVED_OUTCOME)
            }
            OperationPhase::ProviderExecProven | OperationPhase::RolledBack => true,
            _ => false,
        },
        OperationKind::Start => match phase {
            OperationPhase::Committed => true,
            // OpenCode's known-absent pre-effect failure is safe to remove;
            // crossed-boundary unknown outcomes are deliberately refused.
            OperationPhase::Failed => {
                outcome_json
                    .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                    .and_then(|value| {
                        value
                            .get("code")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .as_deref()
                    == Some("provider_effect_not_started")
            }
            _ => false,
        },
    }
}

pub(in crate::state) fn open_workstream_project_root(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
    authorization: CatalogAuthorization,
) -> Result<(String, String, Option<i64>), StateError> {
    let row = transaction
        .query_row(
            "SELECT project_locations.repository_path,
                    workstreams.lifecycle, workstreams.archived_at_millis
             FROM workstreams
             JOIN project_locations
               ON project_locations.location_id = workstreams.location_id
             WHERE workstreams.workstream_id = ?1
               AND workstreams.lifecycle IN ('open', 'parked')",
            [workstream_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, Option<i64>>(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::UnknownOpenWorkstream(workstream_id))?;
    if row.2.is_some() && !authorization.permits_archived() {
        return Err(StateError::WorkstreamArchived(workstream_id));
    }
    Ok(row)
}

pub(in crate::state) fn next_activity_sequence(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<i64, StateError> {
    transaction
        .query_row(
            "SELECT COALESCE(MAX(last_activity_sequence), 0) + 1 FROM workstreams",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

/// Records meaningful runtime or provider lifecycle activity for ordering.
pub(in crate::state) fn touch_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: &str,
    activity_at_millis: Option<i64>,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET last_activity_sequence = ?1,
             last_activity_at_millis = COALESCE(?2, last_activity_at_millis),
             revision = revision + 1
             WHERE workstream_id = ?3",
            params![activity_sequence, activity_at_millis, workstream_id],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

pub(in crate::state) fn reopen_parked_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET lifecycle = 'open', last_activity_sequence = ?1,
             revision = revision + 1
             WHERE workstream_id = ?2 AND lifecycle = 'parked'",
            params![activity_sequence, workstream_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}

pub(in crate::state) fn reopen_recovery_workstream(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(), StateError> {
    let activity_sequence = next_activity_sequence(transaction)?;
    let changed = transaction
        .execute(
            "UPDATE workstreams SET lifecycle = 'open', last_activity_sequence = ?1,
             revision = revision + 1
             WHERE workstream_id = ?2 AND lifecycle = 'recovery_required'",
            params![activity_sequence, workstream_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StateError::ConcurrentWrite)
    }
}
