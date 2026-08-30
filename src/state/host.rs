use std::path::PathBuf;

use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::domain::{HostId, Revision};
use crate::provider::codex::profile::{OBSERVER_PROFILE_NAME, ProfileOwnership};

use super::models::{
    CodexIntegration, HostIdentity, HostRegistry, IntegrationLifecycle, StateError,
};
use super::utils::{
    integration_lifecycle_from_text, integration_lifecycle_text, to_from_sql_error,
};

impl HostRegistry {
    /// Returns the stable identity and generation of this host registry.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity record is missing, malformed, or
    /// cannot be queried.
    pub fn identity(&self) -> Result<HostIdentity, StateError> {
        self.connection
            .query_row(
                "SELECT host_id, registry_generation FROM host_identity WHERE singleton = 1",
                [],
                |row| {
                    let host_id: String = row.get(0)?;
                    let registry_generation: String = row.get(1)?;
                    Ok((host_id, registry_generation))
                },
            )
            .map_err(StateError::Sqlite)
            .and_then(|(host_id, registry_generation)| {
                Uuid::parse_str(&host_id)
                    .map(HostId::from)
                    .map(|host_id| HostIdentity {
                        host_id,
                        registry_generation,
                    })
                    .map_err(StateError::InvalidPersistedUuid)
            })
    }

    /// Returns the host schema version recorded by `SQLite`.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read the schema version.
    pub fn schema_version(&self) -> Result<i64, StateError> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(StateError::Sqlite)
    }

    /// Reads the single `wsnav-observer` ownership record, if it exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be queried or contains invalid
    /// persisted state.
    pub fn codex_integration(&self) -> Result<Option<CodexIntegration>, StateError> {
        self.connection
            .query_row(
                "SELECT canonical_profile_path, owner_id, profile_schema_version,
                    hook_executable_path, generated_content_hash, lifecycle, revision
                 FROM codex_integrations WHERE profile_name = ?1",
                [OBSERVER_PROFILE_NAME],
                row_to_integration,
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Stores an exactly-owned observer profile after an explicit setup action.
    ///
    /// # Errors
    ///
    /// Returns an error if a different ownership record already exists or the
    /// private transaction cannot be committed.
    pub fn record_codex_integration(
        &mut self,
        ownership: ProfileOwnership,
        lifecycle: IntegrationLifecycle,
    ) -> Result<CodexIntegration, StateError> {
        let existing = self.codex_integration()?;
        if let Some(existing) = &existing
            && existing.ownership != ownership
        {
            return Err(StateError::IntegrationOwnershipMismatch);
        }
        let revision = existing
            .as_ref()
            .map_or(Revision::INITIAL, |record| record.revision.next());
        self.connection
            .execute(
                "INSERT INTO codex_integrations (
                integration_id, profile_name, canonical_profile_path, owner_id,
                profile_schema_version, hook_executable_path, generated_content_hash,
                lifecycle, revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(profile_name) DO UPDATE SET
                profile_schema_version = excluded.profile_schema_version,
                lifecycle = excluded.lifecycle, revision = excluded.revision",
                params![
                    Uuid::new_v4().to_string(),
                    OBSERVER_PROFILE_NAME,
                    ownership.canonical_path.to_string_lossy(),
                    ownership.owner_id,
                    i64::from(ownership.profile_schema_version),
                    ownership.hook_executable.to_string_lossy(),
                    ownership.content_hash,
                    integration_lifecycle_text(lifecycle),
                    revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        Ok(CodexIntegration {
            ownership,
            lifecycle,
            revision,
        })
    }

    /// Replaces an already verified observer declaration after an explicit
    /// update. This is the sole state path that may change the recorded hook
    /// executable or declaration hash; the replacement returns to native trust
    /// pending before any managed Runtime can start.
    ///
    /// # Errors
    ///
    /// Returns an error when the expected old ownership is absent or stale, or
    /// the replacement cannot commit atomically.
    pub fn replace_codex_integration(
        &mut self,
        expected: &ProfileOwnership,
        replacement: ProfileOwnership,
        lifecycle: IntegrationLifecycle,
    ) -> Result<CodexIntegration, StateError> {
        let current = self
            .codex_integration()?
            .ok_or(StateError::IntegrationOwnershipMismatch)?;
        if current.ownership != *expected {
            return Err(StateError::IntegrationOwnershipMismatch);
        }
        let revision = current.revision.next();
        let changed = self
            .connection
            .execute(
                "UPDATE codex_integrations SET canonical_profile_path = ?1, owner_id = ?2,
                profile_schema_version = ?3, hook_executable_path = ?4,
                generated_content_hash = ?5, lifecycle = ?6, revision = ?7
             WHERE profile_name = ?8 AND generated_content_hash = ?9 AND revision = ?10",
                params![
                    replacement.canonical_path.to_string_lossy(),
                    replacement.owner_id,
                    i64::from(replacement.profile_schema_version),
                    replacement.hook_executable.to_string_lossy(),
                    replacement.content_hash,
                    integration_lifecycle_text(lifecycle),
                    revision.value(),
                    OBSERVER_PROFILE_NAME,
                    expected.content_hash,
                    current.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        Ok(CodexIntegration {
            ownership: replacement,
            lifecycle,
            revision,
        })
    }

    /// Updates only the lifecycle of an exactly captured observer row.  The
    /// ownership fields and revision form an optimistic fence so native trust
    /// finalization cannot silently advance a row that another actor changed
    /// while the review process was running.
    ///
    /// # Errors
    ///
    /// Returns an error when the captured ownership/revision no longer names
    /// the current integration row or the state write cannot commit.
    pub fn set_codex_integration_lifecycle(
        &mut self,
        expected: &CodexIntegration,
        lifecycle: IntegrationLifecycle,
    ) -> Result<CodexIntegration, StateError> {
        let revision = expected.revision.next();
        let changed = self
            .connection
            .execute(
                "UPDATE codex_integrations SET lifecycle = ?1, revision = ?2
                 WHERE profile_name = ?3 AND canonical_profile_path = ?4
                   AND owner_id = ?5 AND profile_schema_version = ?6
                   AND hook_executable_path = ?7 AND generated_content_hash = ?8
                   AND revision = ?9",
                params![
                    integration_lifecycle_text(lifecycle),
                    revision.value(),
                    OBSERVER_PROFILE_NAME,
                    expected.ownership.canonical_path.to_string_lossy(),
                    expected.ownership.owner_id,
                    i64::from(expected.ownership.profile_schema_version),
                    expected.ownership.hook_executable.to_string_lossy(),
                    expected.ownership.content_hash,
                    expected.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        Ok(CodexIntegration {
            ownership: expected.ownership.clone(),
            lifecycle,
            revision,
        })
    }

    /// Returns whether any managed runtime is not durably stopped.
    ///
    /// # Errors
    ///
    /// Returns an error when runtime state cannot be queried.
    pub fn has_live_runtime(&self) -> Result<bool, StateError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runtimes WHERE lifecycle != 'stopped')",
                [],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)
    }

    /// Removes the observer ownership row after the exact profile file is removed.
    ///
    /// # Errors
    ///
    /// Returns an error for an absent/mismatched record or a failed state mutation.
    pub fn remove_codex_integration(
        &mut self,
        ownership: &ProfileOwnership,
    ) -> Result<(), StateError> {
        let current = self
            .codex_integration()?
            .ok_or(StateError::IntegrationOwnershipMismatch)?;
        if current.ownership != *ownership {
            return Err(StateError::IntegrationOwnershipMismatch);
        }
        let deleted = self.connection.execute(
            "DELETE FROM codex_integrations WHERE profile_name = ?1 AND generated_content_hash = ?2",
            params![OBSERVER_PROFILE_NAME, ownership.content_hash],
        ).map_err(StateError::Sqlite)?;
        if deleted == 1 {
            Ok(())
        } else {
            Err(StateError::ConcurrentWrite)
        }
    }
}

pub(in crate::state) fn row_to_integration(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CodexIntegration> {
    let profile_schema_version = u8::try_from(row.get::<_, i64>(2)?).map_err(to_from_sql_error)?;
    let lifecycle: String = row.get(5)?;
    let revision: i64 = row.get(6)?;
    Ok(CodexIntegration {
        ownership: ProfileOwnership {
            canonical_path: PathBuf::from(row.get::<_, String>(0)?),
            owner_id: row.get(1)?,
            profile_schema_version,
            hook_executable: PathBuf::from(row.get::<_, String>(3)?),
            content_hash: row.get(4)?,
        },
        lifecycle: integration_lifecycle_from_text(&lifecycle).map_err(to_from_sql_error)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}
