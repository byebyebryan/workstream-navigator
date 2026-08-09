use std::path::PathBuf;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{DomainError, LocationId, Revision, RuntimeStatus, WorkstreamId};

use super::models::{
    HostRegistry, PersistedWorkstreamOverview, StateError, WorkstreamOverview,
    WorkstreamOverviewPage,
};
use super::schema::MAX_NAVIGATOR_WORKSTREAMS;
use super::utils::{provider_kind_from_text, workstream_lifecycle_from_text};

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
        let mut workstreams = Vec::new();
        let mut cursor = 0;
        loop {
            let page = self.workstream_overview_page(cursor, MAX_NAVIGATOR_WORKSTREAMS)?;
            workstreams.extend(page.workstreams);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(workstreams);
            };
            cursor = next_cursor;
        }
    }

    /// Hides one exact Workstream from the active navigator scope without
    /// deleting its Runtime, provider binding, attention, project files, or
    /// lineage. The caller is responsible for any necessary Runtime park
    /// before this durable visibility transition.
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
                "SELECT revision, archived_at_millis FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
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
        let updated = transaction
            .execute(
                "UPDATE workstreams SET archived_at_millis = ?1, revision = ?2
                 WHERE workstream_id = ?3 AND revision = ?4",
                params![
                    archived_at_millis,
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
        let workstreams = bases
            .into_iter()
            .map(|base| self.hydrate_workstream_overview(base))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkstreamOverviewPage {
            workstreams,
            next_cursor,
        })
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
            Some(runtime) => match self.binding_for_runtime(runtime.runtime_id) {
                Ok(binding) => binding,
                // A resumed Runtime deliberately retains its old exact
                // binding until the matching SessionStart corroborates the
                // new generation. Do not project that stale binding into a
                // snapshot while the Runtime is still starting.
                Err(StateError::HookEvidenceMismatch)
                    if runtime.status == RuntimeStatus::Starting =>
                {
                    None
                }
                Err(error) => return Err(error),
            },
        };
        let attention = self.attention(workstream_id)?;
        if attention
            .as_ref()
            .and_then(|state| state.latest_native_session_id.as_ref())
            .is_some_and(|session| session.provider() != provider)
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
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
            attention,
        })
    }
}

pub(in crate::state) fn open_workstream_project_root(
    transaction: &rusqlite::Transaction<'_>,
    workstream_id: WorkstreamId,
) -> Result<(String, String, Option<i64>), StateError> {
    transaction
        .query_row(
            "SELECT project_locations.repository_path,
                    workstreams.lifecycle, workstreams.archived_at_millis
             FROM workstreams
             JOIN project_locations
               ON project_locations.location_id = workstreams.location_id
             WHERE workstreams.workstream_id = ?1
               AND workstreams.lifecycle IN ('open', 'parked')",
            [workstream_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?
        .ok_or(StateError::UnknownOpenWorkstream(workstream_id))
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
