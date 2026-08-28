use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{HostId, LocationId, ProviderKind, Revision, WorkstreamId};
use crate::provider::codex::profile::{OBSERVER_PROFILE_NAME, ProfileOwnership};

use super::models::{
    CodexIntegration, ExternalWorkstream, HostIdentity, HostRegistry, IntegrationLifecycle,
    MAX_PROJECT_BROWSER_ENTRIES, ProjectDirectoriesResponse, ProjectDirectoryEntry, StateError,
};
use super::utils::{
    default_project_browser_root, integration_lifecycle_from_text, integration_lifecycle_text,
    project_browser_directory, project_browser_root_label, resolve_project_browser_root,
    safe_project_browser_entry_name, to_from_sql_error, validate_project_browser_relative_path,
    validate_project_display_name, validate_remote_identity_display,
    validate_repository_fingerprint,
};
use super::workstream::next_activity_sequence;

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

    /// Lists bounded direct child directories beneath this host's configured
    /// browser root. Paths stay host-private; the DTO contains only a safe
    /// root label, a relative cursor, and child names.
    ///
    /// # Errors
    ///
    /// Returns an error if the configured root is unavailable, the relative
    /// cursor is unsafe, or a bounded directory read cannot complete.
    pub fn project_directories(
        &self,
        relative_path: &str,
        include_hidden: bool,
    ) -> Result<ProjectDirectoriesResponse, StateError> {
        let root = self.project_browser_root()?;
        let current = self.project_browser_directory(relative_path)?;
        let mut entries = fs::read_dir(&current)
            .map_err(|_| StateError::ProjectBrowserRootUnavailable)?
            .take(MAX_PROJECT_BROWSER_ENTRIES + 1)
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                if !safe_project_browser_entry_name(&name, include_hidden) {
                    return None;
                }
                let path = fs::canonicalize(entry.path()).ok()?;
                if !path.starts_with(&root) || !path.is_dir() {
                    return None;
                }
                Some(ProjectDirectoryEntry {
                    is_git_repository: path.join(".git").exists(),
                    name,
                })
            })
            .collect::<Vec<_>>();
        entries.sort_by_cached_key(|entry| {
            let repository_group = u8::from(!entry.is_git_repository);
            let hidden_group = u8::from(!(include_hidden && entry.name.starts_with('.')));
            (
                repository_group,
                hidden_group,
                entry.name.to_lowercase(),
                entry.name.clone(),
            )
        });
        entries.truncate(MAX_PROJECT_BROWSER_ENTRIES);
        Ok(ProjectDirectoriesResponse {
            root_label: project_browser_root_label(&root),
            relative_path: relative_path.to_owned(),
            include_hidden,
            entries,
        })
    }

    /// Resolves one host-private browser cursor to a directory beneath the
    /// configured root. This is deliberately not exposed through snapshots;
    /// it exists only for local host-side registration.
    ///
    /// # Errors
    ///
    /// Returns an error if the root or the requested child is unavailable, or
    /// if the cursor could escape the configured browser root.
    pub fn project_browser_directory(&self, relative_path: &str) -> Result<PathBuf, StateError> {
        validate_project_browser_relative_path(relative_path)?;
        let root = self.project_browser_root()?;
        project_browser_directory(&root, relative_path)
    }

    /// Sets this host's private project-browser root. `~/…` resolves only on
    /// the selected host and no absolute path is returned through the protocol.
    ///
    /// # Errors
    ///
    /// Returns an error if the supplied root is unsafe, unavailable, or cannot
    /// be atomically persisted.
    pub fn set_project_browser_root(&mut self, root_path: &str) -> Result<(), StateError> {
        let root = resolve_project_browser_root(root_path)?;
        let root = fs::canonicalize(root).map_err(|_| StateError::ProjectBrowserRootUnavailable)?;
        if !root.is_dir() {
            return Err(StateError::ProjectBrowserRootUnavailable);
        }
        let root = root.to_str().ok_or(StateError::InvalidProjectBrowserRoot)?;
        self.connection
            .execute(
                "INSERT INTO project_browser_settings (singleton, root_path, revision)
                 VALUES (1, ?1, 1)
                 ON CONFLICT(singleton) DO UPDATE SET
                   root_path = excluded.root_path,
                   revision = project_browser_settings.revision + 1",
                [root],
            )
            .map_err(StateError::Sqlite)?;
        Ok(())
    }

    pub(in crate::state) fn project_browser_root(&self) -> Result<PathBuf, StateError> {
        let configured: Option<String> = self
            .connection
            .query_row(
                "SELECT root_path FROM project_browser_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let root = match configured {
            Some(path) => PathBuf::from(path),
            None => default_project_browser_root()?,
        };
        let root = fs::canonicalize(root).map_err(|_| StateError::ProjectBrowserRootUnavailable)?;
        root.is_dir()
            .then_some(root)
            .ok_or(StateError::ProjectBrowserRootUnavailable)
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

    /// Registers one existing Git project root as an external initial Workstream.
    ///
    /// # Errors
    ///
    /// Returns an error if an input field is unsafe, the project path already
    /// exists in registry state, or the transaction cannot be committed.
    pub fn register_project_root(
        &mut self,
        project_root: &Path,
        provider: ProviderKind,
    ) -> Result<ExternalWorkstream, StateError> {
        let display_name = project_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("local project")
            .to_owned();
        self.register_project_root_with_metadata(project_root, &display_name, None, None, provider)
    }

    /// Registers a project root with separately discovered project-level
    /// repository metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if an input field is unsafe, the project path already
    /// exists in registry state, or the transaction cannot be committed.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn register_project_root_with_metadata(
        &mut self,
        project_root: &Path,
        repository_display_name: &str,
        remote_identity_fingerprint: Option<&str>,
        remote_identity_display: Option<&str>,
        provider: ProviderKind,
    ) -> Result<ExternalWorkstream, StateError> {
        validate_project_display_name(repository_display_name)?;
        validate_repository_fingerprint(remote_identity_fingerprint)?;
        validate_remote_identity_display(remote_identity_display)?;
        let location_id = LocationId::new();
        let registration = ExternalWorkstream {
            location_id,
            workstream_id: WorkstreamId::new(),
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let activity_sequence = next_activity_sequence(&transaction)?;
        // Schema 13 requires every Location to belong to exactly one Project.
        // Use the same two-phase Location/Project insertion as the D16 state
        // seam: the nullable foreign key is filled only after the Project row
        // can refer back to its label Location.
        transaction
            .execute(
                "INSERT INTO project_locations (
                    location_id, repository_path,
                    repository_display_name, remote_identity_fingerprint,
                    remote_identity_display, revision, project_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, NULL)",
                params![
                    registration.location_id.to_string(),
                    project_root.to_string_lossy(),
                    repository_display_name,
                    remote_identity_fingerprint.unwrap_or(""),
                    remote_identity_display.unwrap_or(""),
                ],
            )
            .map_err(StateError::Sqlite)?;
        let project_id: String = if let Some(fingerprint) =
            remote_identity_fingerprint.filter(|value| !value.is_empty())
        {
            let existing = transaction
                .query_row(
                    "SELECT project_id FROM projects
                     WHERE repository_fingerprint = ?1",
                    [fingerprint],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(StateError::Sqlite)?;
            if let Some(project_id) = existing {
                transaction
                    .execute(
                        "UPDATE projects SET revision = revision + 1
                         WHERE project_id = ?1",
                        [&project_id],
                    )
                    .map_err(StateError::Sqlite)?;
                project_id
            } else {
                let project_id = Uuid::new_v4().to_string();
                transaction
                    .execute(
                        "INSERT INTO projects (
                            project_id, label_location_id, display_name,
                            repository_fingerprint, revision
                         ) VALUES (?1, ?2, ?3, ?4, 1)",
                        params![
                            project_id,
                            registration.location_id.to_string(),
                            repository_display_name,
                            fingerprint,
                        ],
                    )
                    .map_err(StateError::Sqlite)?;
                project_id
            }
        } else {
            let project_id = Uuid::new_v4().to_string();
            transaction
                .execute(
                    "INSERT INTO projects (
                        project_id, label_location_id, display_name,
                        repository_fingerprint, revision
                     ) VALUES (?1, ?2, ?3, NULL, 1)",
                    params![
                        project_id,
                        registration.location_id.to_string(),
                        repository_display_name,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            project_id
        };
        transaction
            .execute(
                "UPDATE project_locations SET project_id = ?1
                 WHERE location_id = ?2",
                params![project_id, registration.location_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "INSERT INTO workstreams (
                    workstream_id, location_id, provider, origin, source_workstream_id,
                    lifecycle, last_activity_sequence,
                    last_activity_at_millis, revision
                 ) VALUES (?1, ?2, ?3, 'external', NULL, 'open', ?4, ?5, 1)",
                params![
                    registration.workstream_id.to_string(),
                    registration.location_id.to_string(),
                    provider.as_str(),
                    activity_sequence,
                    0_i64,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(registration)
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
