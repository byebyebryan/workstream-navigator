//! Current registry reads and transactional state access.
//!
//! This module is the narrow boundary for host registry projections and
//! revision-fenced reads. Mutations that span provider onboarding live in the
//! onboarding module; presentation owns its own topology.

use super::onboarding::load_exec_proof_target;
use super::{
    BTreeMap, BTreeSet, CurrentState, HostRegistry, MAX_NAVIGATOR_WORKSTREAMS, OnboardingMarkerRow,
    OnboardingOperationInventory, OnboardingOperationInventoryPage, OnboardingPhase,
    OnboardingProviderExecTarget, OnboardingVisibility, OnboardingWorkstreamProjection,
    OperationId, OptionalExtension, PARKED_RECOVERY_RESOLVED_OUTCOME, Path,
    PersistedOnboardingIntent, ProjectProjection, RuntimeId, RuntimePaths, RuntimePathsPage,
    StateError, StateMode, Uuid, WorkstreamId, ensure_current_mode, load_project_projections,
    operation_phase_from_text, page_parameters, schema_version, validate_schema15,
};

#[cfg(test)]
use super::{
    create_project, next_activity_sequence, validate_project_display_name,
    validate_project_membership_transaction,
};
#[cfg(test)]
use crate::domain::{IdGenerator, LocationId, ProviderKind};
#[cfg(test)]
use rusqlite::{TransactionBehavior, params};

impl CurrentState {
    #[must_use]
    pub const fn mode(&self) -> StateMode {
        self.mode
    }

    /// Returns the exact state-root spelling retained by this handle.  The
    /// path is used only for revalidation; it is never included
    /// in public projections or provider-facing state.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads `PRAGMA user_version` without changing the database.
    pub fn schema_version(&self) -> Result<i64, StateError> {
        schema_version(&self.connection)
    }

    /// Seeds one complete current Project/Location/Workstream graph for
    /// in-crate lifecycle fixtures. Production onboarding uses the broker
    /// transaction below, so tests must not exercise a public arbitrary
    /// registration API.
    #[cfg(test)]
    pub(crate) fn seed_test_workstream(
        &mut self,
        repository_path: &Path,
        display_name: &str,
        provider: ProviderKind,
        id_generator: &dyn IdGenerator,
    ) -> Result<(LocationId, WorkstreamId), StateError> {
        ensure_current_mode(self.mode)?;
        validate_schema15(&self.connection)?;
        validate_project_display_name(display_name)?;
        let repository_path = repository_path
            .to_str()
            .ok_or(StateError::InvalidPersistedValue(
                "repository path is not UTF-8".to_owned(),
            ))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let location_id = LocationId::from(id_generator.uuid());
        transaction
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_path, repository_display_name,
                    remote_identity_fingerprint, remote_identity_display,
                    revision, project_id
                 ) VALUES (?1, ?2, ?3, NULL, '', 1, NULL)",
                params![location_id.to_string(), repository_path, display_name,],
            )
            .map_err(StateError::Sqlite)?;
        let project = create_project(&transaction, location_id, display_name, None, id_generator)?;
        transaction
            .execute(
                "UPDATE project_locations SET project_id = ?1 WHERE location_id = ?2",
                params![project.project_id.to_string(), location_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        let workstream_id = WorkstreamId::from(id_generator.uuid());
        let activity_sequence = next_activity_sequence(&transaction)?;
        transaction
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin,
                    source_workstream_id, lifecycle, archived_at_millis,
                    last_activity_sequence, last_activity_at_millis, revision
                 ) VALUES (?1, ?2, ?3, 'external', NULL, 'open', NULL, ?4, 0, 1)",
                params![
                    workstream_id.to_string(),
                    location_id.to_string(),
                    provider.as_str(),
                    activity_sequence,
                ],
            )
            .map_err(StateError::Sqlite)?;
        validate_project_membership_transaction(&transaction)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok((location_id, workstream_id))
    }

    /// Converts an exact current schema-15 handle into the retained lifecycle
    /// registry. The handle exposes no legacy-schema behavior.
    pub(crate) fn into_host_registry(self) -> Result<HostRegistry, StateError> {
        if self.mode != StateMode::Current {
            return Err(StateError::MalformedHostSchema);
        }
        validate_schema15(&self.connection)?;
        let Self { connection, .. } = self;
        Ok(HostRegistry { connection })
    }

    /// Returns the retained Project/Location display projection for the
    /// Workstreams surface. No repository or Git inspection occurs here.
    pub(crate) fn project_projections(&self) -> Result<Vec<ProjectProjection>, StateError> {
        ensure_current_mode(self.mode)?;
        validate_schema15(&self.connection)?;
        load_project_projections(&self.connection)
    }

    /// Projects the only onboarding states that change Workstreams
    /// visibility or action authority. The journal is read as a bounded,
    /// exact Runtime relationship: a malformed or duplicate relationship
    /// refuses the whole snapshot instead of exposing a possibly unowned
    /// Runtime card.
    pub(crate) fn onboarding_workstream_projections(
        &self,
    ) -> Result<Vec<OnboardingWorkstreamProjection>, StateError> {
        let operations = self.onboarding_operation_inventory()?;
        let mut projections = BTreeMap::new();
        for operation in operations {
            let visibility = match operation.phase {
                OnboardingPhase::CapabilityIssued => OnboardingVisibility::Reserved,
                OnboardingPhase::RuntimeOwnedLaunching
                | OnboardingPhase::ProviderPreparation
                | OnboardingPhase::ProviderExternalEffectStarted
                | OnboardingPhase::ProviderExecStarted
                | OnboardingPhase::KnownAbsentExec => OnboardingVisibility::ActionFenced,
                OnboardingPhase::RecoveryRequired => OnboardingVisibility::RecoveryRequired,
                OnboardingPhase::ProviderExecProven | OnboardingPhase::RolledBack => continue,
                OnboardingPhase::Prepared => return Err(StateError::MalformedHostSchema),
            };
            if projections
                .insert(
                    operation.workstream_id,
                    OnboardingWorkstreamProjection {
                        workstream_id: operation.workstream_id,
                        runtime_id: operation.runtime_id,
                        visibility,
                    },
                )
                .is_some()
            {
                return Err(StateError::MalformedHostSchema);
            }
        }
        Ok(projections.into_values().collect())
    }

    /// Returns the bounded exact journal inventory required to reconcile
    /// a presentation-private provisional marker. It validates each durable
    /// operation against the Runtime it claims before returning any entry.
    #[allow(
        clippy::too_many_lines,
        reason = "the bounded journal inventory validates every retained onboarding phase and its exact Runtime relationship together"
    )]
    pub(crate) fn onboarding_operation_inventory(
        &self,
    ) -> Result<Vec<OnboardingOperationInventory>, StateError> {
        ensure_current_mode(self.mode)?;
        validate_schema15(&self.connection)?;
        let _read_snapshot = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let mut cursor = 0;
        let mut seen_workstreams = BTreeSet::new();
        let mut inventory = Vec::new();
        loop {
            let page =
                self.onboarding_operation_inventory_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
            for workstream_id in page.workstream_ids {
                if !seen_workstreams.insert(workstream_id) {
                    return Err(StateError::MalformedHostSchema);
                }
            }
            inventory.extend(page.operations);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(inventory);
            };
            cursor = next_cursor;
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one page validates every retained onboarding phase and exact Runtime relationship together"
    )]
    fn onboarding_operation_inventory_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<OnboardingOperationInventoryPage, StateError> {
        let (query_limit, cursor_step) = page_parameters(page_size)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT operation_id, phase, expected_revisions_json, outcome_json
                 FROM compound_operations
                 WHERE kind = 'onboard'
                 ORDER BY operation_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(StateError::Sqlite)?;
        let operations = statement
            .query_map([query_limit, i64::from(cursor)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        drop(statement);
        let has_more = operations.len() > page_size;
        let mut inventory = Vec::with_capacity(operations.len());
        let mut workstream_ids = Vec::with_capacity(operations.len());
        for (operation_id, phase, encoded_intent, outcome_json) in
            operations.into_iter().take(page_size)
        {
            let operation_id = Uuid::parse_str(&operation_id)
                .map(OperationId::from)
                .map_err(StateError::InvalidPersistedUuid)?;
            let operation_phase =
                operation_phase_from_text(&phase).map_err(|_| StateError::MalformedHostSchema)?;
            let intent: PersistedOnboardingIntent = serde_json::from_str(&encoded_intent)
                .map_err(|_| StateError::MalformedHostSchema)?;
            if intent.version != 1 {
                return Err(StateError::MalformedHostSchema);
            }
            workstream_ids.push(intent.workstream_id);
            // Archive may terminally resolve a recovery-required Runtime after
            // exact-stop cleanup. That commits the onboarding journal without
            // claiming its original native exec was proven. The exact, bounded
            // outcome is durable evidence of that decision; its Runtime may
            // subsequently be resumed, so current stopped/parked lifecycle is
            // not required.
            if operation_phase == crate::domain::OperationPhase::Committed {
                let retained_runtime: Option<(String, String)> = self
                    .connection
                    .query_row(
                        "SELECT workstream_id, provider
                         FROM runtimes
                         WHERE runtimes.runtime_id = ?1",
                        [intent.candidate_runtime_id.to_string()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(StateError::Sqlite)?;
                if outcome_json.as_deref() != Some(PARKED_RECOVERY_RESOLVED_OUTCOME)
                    || retained_runtime
                        != Some((
                            intent.workstream_id.to_string(),
                            intent.provider.as_str().to_owned(),
                        ))
                {
                    return Err(StateError::MalformedHostSchema);
                }
                continue;
            }
            let phase = OnboardingPhase::from_operation_phase(operation_phase)
                .ok_or(StateError::MalformedHostSchema)?;

            let runtime: Option<(String, String)> = self
                .connection
                .query_row(
                    "SELECT workstream_id, provider
                     FROM runtimes WHERE runtime_id = ?1",
                    [intent.candidate_runtime_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(StateError::Sqlite)?;
            if phase == OnboardingPhase::RolledBack {
                if runtime.is_some() {
                    return Err(StateError::MalformedHostSchema);
                }
                inventory.push(OnboardingOperationInventory {
                    operation_id,
                    workstream_id: intent.workstream_id,
                    runtime_id: intent.candidate_runtime_id,
                    phase,
                });
                continue;
            }
            let Some((workstream_id, provider)) = runtime else {
                return Err(StateError::MalformedHostSchema);
            };
            if workstream_id != intent.workstream_id.to_string()
                || provider != intent.provider.as_str()
            {
                return Err(StateError::MalformedHostSchema);
            }
            inventory.push(OnboardingOperationInventory {
                operation_id,
                workstream_id: intent.workstream_id,
                runtime_id: intent.candidate_runtime_id,
                phase,
            });
        }
        let next_cursor = if has_more {
            Some(
                cursor
                    .checked_add(cursor_step)
                    .ok_or(StateError::NavigatorCursorOverflow)?,
            )
        } else {
            None
        };
        Ok(OnboardingOperationInventoryPage {
            operations: inventory,
            workstream_ids,
            next_cursor,
        })
    }

    pub(crate) fn onboarding_marker_operation_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<(Vec<OnboardingMarkerRow>, Option<u32>), StateError> {
        let (query_limit, cursor_step) = page_parameters(page_size)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT operation_id, phase, expected_revisions_json
                 FROM compound_operations
                 WHERE kind = 'onboard'
                 ORDER BY operation_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(StateError::Sqlite)?;
        let rows = statement
            .query_map([query_limit, i64::from(cursor)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        let has_more = rows.len() > page_size;
        let next_cursor = if has_more {
            Some(
                cursor
                    .checked_add(cursor_step)
                    .ok_or(StateError::NavigatorCursorOverflow)?,
            )
        } else {
            None
        };
        Ok((rows.into_iter().take(page_size).collect(), next_cursor))
    }

    /// Lists the exact private path set for every retained Runtime. This is
    /// classifier input only; neither paths nor session names enter a
    /// navigator snapshot or provider command.
    pub(crate) fn registered_runtime_paths(&self) -> Result<Vec<RuntimePaths>, StateError> {
        ensure_current_mode(self.mode)?;
        validate_schema15(&self.connection)?;
        let _read_snapshot = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let mut cursor = 0;
        let mut paths = Vec::new();
        loop {
            let page = self.registered_runtime_paths_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
            paths.extend(page.paths);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(paths);
            };
            cursor = next_cursor;
        }
    }

    fn registered_runtime_paths_page(
        &self,
        cursor: u32,
        page_size: usize,
    ) -> Result<RuntimePathsPage, StateError> {
        let (query_limit, cursor_step) = page_parameters(page_size)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT runtime_id, tmux_session
                 FROM runtimes
                 ORDER BY runtime_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(StateError::Sqlite)?;
        let runtimes = statement
            .query_map([query_limit, i64::from(cursor)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        drop(statement);
        let has_more = runtimes.len() > page_size;
        let paths = runtimes
            .into_iter()
            .take(page_size)
            .map(|(runtime_id, session_name)| {
                let runtime_id = Uuid::parse_str(&runtime_id)
                    .map(RuntimeId::from)
                    .map_err(StateError::InvalidPersistedUuid)?;
                RuntimePaths::for_record(&self.root, runtime_id, &session_name)
                    .map_err(|_| StateError::MalformedHostSchema)
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
        Ok(RuntimePathsPage { paths, next_cursor })
    }
}

impl HostRegistry {
    /// Loads the one durable onboarding proof that can authorize the shell
    /// promotion exit path for an exact Runtime.  This is intentionally a
    /// registry read rather than a marker read: the original presentation may
    /// already be gone by the time a retained provider pane is reconciled.
    /// Every onboarding journal is scanned in bounded pages so a long-lived
    /// host cannot turn this proof into an unbounded query.
    pub(crate) fn onboarding_exec_proven_target_for_runtime(
        &self,
        state_root: &Path,
        workstream_id: WorkstreamId,
        runtime_id: RuntimeId,
        runtime_generation: &str,
    ) -> Result<Option<OnboardingProviderExecTarget>, StateError> {
        if runtime_generation.is_empty() {
            return Err(StateError::MalformedHostSchema);
        }
        validate_schema15(&self.connection)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let (query_limit, cursor_step) = page_parameters(MAX_NAVIGATOR_WORKSTREAMS)?;
        let mut cursor = 0_u32;
        let mut matching_operation = None;
        let mut matching_non_proven = false;
        loop {
            let rows = {
                let mut statement = transaction
                    .prepare(
                        "SELECT operation_id, phase, expected_revisions_json
                         FROM compound_operations
                         WHERE kind = 'onboard'
                         ORDER BY operation_id
                         LIMIT ?1 OFFSET ?2",
                    )
                    .map_err(StateError::Sqlite)?;
                let rows = statement
                    .query_map([query_limit, i64::from(cursor)], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(StateError::Sqlite)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(StateError::Sqlite)?;
                drop(statement);
                rows
            };
            let has_more = rows.len() > MAX_NAVIGATOR_WORKSTREAMS;
            for (operation_id, phase, encoded_intent) in
                rows.into_iter().take(MAX_NAVIGATOR_WORKSTREAMS)
            {
                let operation_id = operation_id
                    .parse::<OperationId>()
                    .map_err(|_| StateError::MalformedHostSchema)?;
                let operation_phase = operation_phase_from_text(&phase)
                    .map_err(|_| StateError::MalformedHostSchema)?;
                let intent: PersistedOnboardingIntent = serde_json::from_str(&encoded_intent)
                    .map_err(|_| StateError::MalformedHostSchema)?;
                if intent.version != 1 {
                    return Err(StateError::MalformedHostSchema);
                }
                if intent.candidate_runtime_id != runtime_id {
                    continue;
                }
                if intent.workstream_id != workstream_id {
                    return Err(StateError::OperationRequestMismatch);
                }
                // Runtime IDs are retained across launches. Older proven
                // onboarding journals for the same Runtime therefore remain
                // in the bounded audit log, but cannot authorize a later
                // generation. Only an exact current generation is eligible
                // for the promoted-cwd exception below.
                if intent.runtime_generation != runtime_generation {
                    continue;
                }
                if operation_phase != crate::domain::OperationPhase::ProviderExecProven {
                    if matching_non_proven || matching_operation.is_some() {
                        return Err(StateError::MalformedHostSchema);
                    }
                    matching_non_proven = true;
                    continue;
                }
                if matching_non_proven || matching_operation.replace(operation_id).is_some() {
                    return Err(StateError::MalformedHostSchema);
                }
            }
            if !has_more {
                break;
            }
            cursor = cursor
                .checked_add(cursor_step)
                .ok_or(StateError::NavigatorCursorOverflow)?;
        }

        let target = matching_operation
            .map(|operation_id| {
                load_exec_proof_target(
                    &transaction,
                    state_root,
                    operation_id,
                    OnboardingPhase::ProviderExecProven,
                )
            })
            .transpose()?;
        transaction.commit().map_err(StateError::Sqlite)?;
        if target.as_ref().is_some_and(|target| {
            target.ownership().workstream_id != workstream_id
                || target.ownership().runtime_id != runtime_id
        }) {
            return Err(StateError::OnboardingOperationUnavailable);
        }
        Ok(target)
    }
}
