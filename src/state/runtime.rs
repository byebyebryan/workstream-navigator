use std::path::PathBuf;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{
    Clock, ProviderKind, ProviderSessionId, Revision, RuntimeId, RuntimeStatus, SystemClock,
    WorkstreamId, WorkstreamLifecycle,
};

use super::attention::ensure_recovery_attention_in_transaction;
use super::compound::bind_opencode_session_in_transaction;
use super::lifecycle::{apply_opencode_lifecycle_transition, validate_opencode_observation};
use super::models::{
    HostRegistry, OpenCodeLifecycleObservation, OpenCodeObserverStatus, OpenCodeRuntimeHandle,
    ProviderBinding, RuntimeRecord, StateError,
};
use super::utils::{
    name_state_from_text, provider_kind_from_text, runtime_status_from_text, to_from_sql_error,
    validate_provider_metadata, validate_registry_text, workstream_lifecycle_from_text,
};
use super::workstream::{
    next_activity_sequence, open_workstream_project_root, reopen_parked_workstream,
    touch_workstream,
};

impl HostRegistry {
    #[allow(clippy::too_many_lines)]
    /// Reserves the single Runtime record for an open workstream before launch.
    ///
    /// # Errors
    ///
    /// Returns an error when the workstream is unknown, not open, already live,
    /// or durable state cannot be changed.
    pub fn reserve_runtime_with_provider(
        &mut self,
        workstream_id: WorkstreamId,
        provider: ProviderKind,
    ) -> Result<RuntimeRecord, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (project_root, workstream_lifecycle, archived_at_millis) =
            open_workstream_project_root(&transaction, workstream_id)?;
        let workstream_provider: String = transaction
            .query_row(
                "SELECT provider FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        let workstream_provider = provider_kind_from_text(&workstream_provider)?;
        if workstream_provider != provider {
            return Err(StateError::ProviderIdentityMismatch);
        }
        if archived_at_millis.is_some() {
            return Err(StateError::WorkstreamArchived(workstream_id));
        }
        let current: Option<RuntimeRecord> = transaction
            .query_row(
                "SELECT runtime_id, provider, tmux_generation, tmux_session, cwd, provider_pid, process_birth, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let generation = Uuid::new_v4().to_string();
        let record = if let Some(current) = current {
            if current.provider != workstream_provider {
                return Err(StateError::ProviderIdentityMismatch);
            }
            if !matches!(
                current.status,
                RuntimeStatus::Stopped | RuntimeStatus::Unknown
            ) {
                return Err(StateError::RuntimeAlreadyLive(workstream_id));
            }
            let next = RuntimeRecord {
                tmux_generation: generation,
                tmux_session: format!("wsnav-{}", current.runtime_id),
                cwd: PathBuf::from(&project_root),
                provider_pid: None,
                process_birth: None,
                status: RuntimeStatus::Starting,
                revision: current.revision.next(),
                ..current
            };
            transaction
                .execute(
                    "UPDATE runtimes SET tmux_generation = ?1, tmux_session = ?2, cwd = ?3,
                     provider_pid = NULL, process_birth = NULL, lifecycle = 'starting', revision = ?4
                     WHERE runtime_id = ?5 AND revision = ?6",
                    params![
                        next.tmux_generation,
                        next.tmux_session,
                        next.cwd.to_string_lossy(),
                        next.revision.value(),
                        next.runtime_id.to_string(),
                        current.revision.value()
                    ],
                )
                .map_err(StateError::Sqlite)?;
            next
        } else {
            let runtime_id = RuntimeId::new();
            let record = RuntimeRecord {
                runtime_id,
                workstream_id,
                provider: workstream_provider,
                tmux_generation: generation,
                tmux_session: format!("wsnav-{runtime_id}"),
                cwd: PathBuf::from(project_root),
                provider_pid: None,
                process_birth: None,
                status: RuntimeStatus::Starting,
                revision: Revision::INITIAL,
            };
            transaction
                .execute(
                    "INSERT INTO runtimes (
                    runtime_id, workstream_id, provider, tmux_generation, tmux_session,
                    cwd, provider_pid, process_birth, lifecycle, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, 'starting', 1)",
                    params![
                        record.runtime_id.to_string(),
                        workstream_id.to_string(),
                        record.provider.as_str(),
                        record.tmux_generation,
                        record.tmux_session,
                        record.cwd.to_string_lossy()
                    ],
                )
                .map_err(StateError::Sqlite)?;
            record
        };
        if workstream_lifecycle == "parked" {
            reopen_parked_workstream(&transaction, workstream_id)?;
        } else {
            touch_workstream(&transaction, &workstream_id.to_string(), None)?;
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(record)
    }

    #[cfg(test)]
    #[allow(clippy::missing_errors_doc)]
    pub fn reserve_runtime(
        &mut self,
        workstream_id: WorkstreamId,
    ) -> Result<RuntimeRecord, StateError> {
        let provider = self.workstream_provider(workstream_id)?;
        self.reserve_runtime_with_provider(workstream_id, provider)
    }

    /// Reserves a new private tmux generation for an explicitly recovering
    /// Workstream. The Workstream remains `recovery_required` until a verified
    /// native `SessionStart(source=resume)` binds the launched Codex process.
    ///
    /// # Errors
    ///
    /// Returns an error unless this Workstream has one runtime in the exact
    /// `unknown` state established by [`Self::mark_runtime_recovery_required`].
    pub fn reserve_runtime_recovery_with_provider(
        &mut self,
        workstream_id: WorkstreamId,
        provider: ProviderKind,
    ) -> Result<RuntimeRecord, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (project_root, archived_at_millis, workstream_provider): (String, Option<i64>, String) =
            transaction
                .query_row(
                    "SELECT project_locations.repository_path, workstreams.archived_at_millis,
                        workstreams.provider
                 FROM workstreams
                 JOIN project_locations
                   ON project_locations.location_id = workstreams.location_id
                 WHERE workstreams.workstream_id = ?1
                   AND workstreams.lifecycle = 'recovery_required'",
                    [workstream_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(StateError::Sqlite)?
                .ok_or(StateError::RecoveryUnavailable(workstream_id))?;
        if archived_at_millis.is_some() {
            return Err(StateError::WorkstreamArchived(workstream_id));
        }
        let workstream_provider = provider_kind_from_text(&workstream_provider)?;
        if workstream_provider != provider {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let current: RuntimeRecord = transaction
            .query_row(
                "SELECT runtime_id, provider, tmux_generation, tmux_session, cwd, provider_pid, process_birth, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1 AND lifecycle = 'unknown'",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::RecoveryUnavailable(workstream_id))?;
        if current.provider != workstream_provider {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let next = RuntimeRecord {
            tmux_generation: Uuid::new_v4().to_string(),
            tmux_session: format!("wsnav-{}", current.runtime_id),
            cwd: PathBuf::from(project_root),
            provider_pid: None,
            process_birth: None,
            status: RuntimeStatus::Starting,
            revision: current.revision.next(),
            ..current
        };
        let changed = transaction
            .execute(
                "UPDATE runtimes SET tmux_generation = ?1, tmux_session = ?2, cwd = ?3,
                 provider_pid = NULL, process_birth = NULL, lifecycle = 'starting', revision = ?4
                 WHERE runtime_id = ?5 AND revision = ?6 AND lifecycle = 'unknown'",
                params![
                    next.tmux_generation,
                    next.tmux_session,
                    next.cwd.to_string_lossy(),
                    next.revision.value(),
                    next.runtime_id.to_string(),
                    current.revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        touch_workstream(&transaction, &workstream_id.to_string(), None)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(next)
    }

    #[cfg(test)]
    #[allow(clippy::missing_errors_doc)]
    pub fn reserve_runtime_recovery(
        &mut self,
        workstream_id: WorkstreamId,
    ) -> Result<RuntimeRecord, StateError> {
        let provider = self.workstream_provider(workstream_id)?;
        self.reserve_runtime_recovery_with_provider(workstream_id, provider)
    }

    /// Reads the single persisted runtime record for a workstream.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains invalid
    /// persisted runtime data.
    pub fn runtime_for_workstream(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        let runtime = self
            .connection
            .query_row(
            "SELECT runtime_id, provider, tmux_generation, tmux_session, cwd, provider_pid, process_birth, lifecycle, revision
                 FROM runtimes WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row_to_runtime(row, workstream_id),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        if let Some(runtime) = &runtime {
            let provider = self.workstream_provider(workstream_id)?;
            if provider != runtime.provider {
                return Err(StateError::ProviderIdentityMismatch);
            }
        }
        Ok(runtime)
    }

    pub(in crate::state) fn workstream_provider(
        &self,
        workstream_id: WorkstreamId,
    ) -> Result<ProviderKind, StateError> {
        let value: String = self
            .connection
            .query_row(
                "SELECT provider FROM workstreams WHERE workstream_id = ?1",
                [workstream_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        provider_kind_from_text(&value)
    }

    /// Reads one exact persisted Runtime by its opaque identity.
    ///
    /// This is used only to validate an explicit native terminal attachment
    /// or another exact host-local lifecycle transition.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains an
    /// invalid persisted Runtime record.
    pub fn runtime_by_id(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        let runtime = self
            .connection
            .query_row(
                "SELECT workstream_id, provider, tmux_generation, tmux_session, cwd,
                        provider_pid, process_birth, lifecycle, revision
                 FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| {
                    let workstream_id: String = row.get(0)?;
                    let workstream_id = Uuid::parse_str(&workstream_id)
                        .map(WorkstreamId::from)
                        .map_err(to_from_sql_error)?;
                    row_to_runtime_with_id(row, runtime_id, workstream_id)
                },
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        if let Some(runtime) = &runtime {
            let provider = self.workstream_provider(runtime.workstream_id)?;
            if provider != runtime.provider {
                return Err(StateError::ProviderIdentityMismatch);
            }
        }
        Ok(runtime)
    }

    /// D17 observer-only spelling for the already-open schema-14 registry.
    /// It avoids granting the observer any root-opening authority.
    pub(crate) fn observer_runtime_by_id(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<RuntimeRecord>, StateError> {
        self.runtime_by_id(runtime_id)
    }

    /// Returns only current, process-fingerprinted private Runtimes that may
    /// corroborate a passive Codex hook. This is host-local evidence; callers
    /// must still probe the exact private tmux pane and require one match.
    ///
    /// # Errors
    ///
    /// Returns an error when persisted Runtime identity is malformed or the
    /// private registry cannot be queried.
    pub fn hook_runtime_candidates(&self) -> Result<Vec<RuntimeRecord>, StateError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT runtime_id, workstream_id, tmux_generation, tmux_session,
                        provider, cwd, provider_pid, process_birth, lifecycle, revision
                 FROM runtimes
                 WHERE lifecycle IN ('starting', 'idle', 'working', 'attention')
                   AND provider_pid IS NOT NULL AND process_birth IS NOT NULL",
            )
            .map_err(StateError::Sqlite)?;
        statement
            .query_map([], |row| {
                let runtime_id: String = row.get(0)?;
                let workstream_id: String = row.get(1)?;
                let tmux_generation: String = row.get(2)?;
                let tmux_session: String = row.get(3)?;
                let provider: String = row.get(4)?;
                let cwd: String = row.get(5)?;
                let provider_pid: Option<i64> = row.get(6)?;
                let process_birth: Option<String> = row.get(7)?;
                let lifecycle: String = row.get(8)?;
                let revision: i64 = row.get(9)?;
                Ok((
                    runtime_id,
                    workstream_id,
                    tmux_generation,
                    tmux_session,
                    provider,
                    cwd,
                    provider_pid,
                    process_birth,
                    lifecycle,
                    revision,
                ))
            })
            .map_err(StateError::Sqlite)?
            .map(|row| {
                let (
                    runtime_id,
                    workstream_id,
                    tmux_generation,
                    tmux_session,
                    provider,
                    cwd,
                    provider_pid,
                    process_birth,
                    lifecycle,
                    revision,
                ) = row.map_err(StateError::Sqlite)?;
                Ok(RuntimeRecord {
                    runtime_id: Uuid::parse_str(&runtime_id)
                        .map(RuntimeId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    workstream_id: Uuid::parse_str(&workstream_id)
                        .map(WorkstreamId::from)
                        .map_err(StateError::InvalidPersistedUuid)?,
                    provider: provider_kind_from_text(&provider)?,
                    tmux_generation,
                    tmux_session,
                    cwd: PathBuf::from(cwd),
                    provider_pid: provider_pid
                        .map(|pid| {
                            u32::try_from(pid).map_err(|_| {
                                StateError::InvalidPersistedValue("provider PID".to_owned())
                            })
                        })
                        .transpose()?,
                    process_birth,
                    status: runtime_status_from_text(&lifecycle)?,
                    revision: Revision::try_from(revision)?,
                })
            })
            .collect()
    }

    /// Confirms that one exact Runtime ended through the explicit park action.
    ///
    /// This is intentionally stricter than a stopped runtime alone: an
    /// unexpected native-process exit also leaves a Runtime stopped, but does
    /// not park its Workstream. Attachment helpers use this distinction after
    /// their private tmux client exits unexpectedly.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains an
    /// invalid persisted lifecycle value.
    pub fn runtime_is_deliberately_parked(
        &self,
        runtime_id: RuntimeId,
        workstream_id: WorkstreamId,
    ) -> Result<bool, StateError> {
        let lifecycle: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT runtimes.lifecycle, workstreams.lifecycle
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.runtime_id = ?1 AND runtimes.workstream_id = ?2",
                params![runtime_id.to_string(), workstream_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((runtime_lifecycle, workstream_lifecycle)) = lifecycle else {
            return Ok(false);
        };
        Ok(
            runtime_status_from_text(&runtime_lifecycle)? == RuntimeStatus::Stopped
                && workstream_lifecycle_from_text(&workstream_lifecycle)?
                    == WorkstreamLifecycle::Parked,
        )
    }

    /// Reads the current exact native-session binding for one runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry cannot be queried or contains invalid
    /// persisted binding data.
    pub fn binding_for_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<ProviderBinding>, StateError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let binding = load_current_binding(&transaction, runtime_id)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(binding)
    }

    /// Reads a previously corroborated Codex session for an inactive Runtime.
    ///
    /// An exact current-generation binding is returned for any Runtime status.
    /// A binding from an older generation is returned only for a persisted
    /// `stopped` or `unknown` Runtime, where it is resume history rather than
    /// hook or mutation authority. `OpenCode` deliberately has no retained
    /// binding path.
    ///
    /// # Errors
    ///
    /// Returns an error when the Runtime, Workstream, or binding provider
    /// identities disagree, when the persisted binding is malformed, or when
    /// a stale binding belongs to an active Runtime.
    pub(crate) fn retained_codex_binding_for_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<ProviderBinding>, StateError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let (runtime_provider, workstream_provider, generation, lifecycle): (
            String,
            String,
            String,
            String,
        ) = transaction
            .query_row(
                "SELECT runtimes.provider, workstreams.provider,
                            runtimes.tmux_generation, runtimes.lifecycle
                     FROM runtimes
                     JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                     WHERE runtimes.runtime_id = ?1",
                [runtime_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownRuntime(runtime_id))?;
        let runtime_provider = provider_kind_from_text(&runtime_provider)?;
        let workstream_provider = provider_kind_from_text(&workstream_provider)?;
        validate_registry_text("runtime generation", &generation)?;
        let runtime_status = runtime_status_from_text(&lifecycle)?;
        if runtime_provider != workstream_provider || runtime_provider != ProviderKind::Codex {
            return Err(StateError::ProviderIdentityMismatch);
        }
        let binding = load_binding(&transaction, runtime_id)?;
        let Some(binding) = binding else {
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(None);
        };
        if binding.provider != ProviderKind::Codex
            || binding.native_session_id.provider() != ProviderKind::Codex
            || binding
                .predecessor_native_session_id
                .as_ref()
                .is_some_and(|session| session.provider() != ProviderKind::Codex)
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
        if binding.runtime_generation != generation
            && !matches!(
                runtime_status,
                RuntimeStatus::Stopped | RuntimeStatus::Unknown
            )
        {
            return Err(StateError::HookEvidenceMismatch);
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(Some(binding))
    }

    /// Loads the host-private `OpenCode` endpoint and observer identity for one
    /// exact Runtime.  It refuses to return a handle whose provider or
    /// generation no longer matches the current Runtime and binding.
    #[allow(clippy::missing_errors_doc)]
    pub fn opencode_runtime_handle(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<OpenCodeRuntimeHandle>, StateError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StateError::Sqlite)?;
        let handle = load_opencode_handle(&transaction, runtime_id)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(handle)
    }

    /// D17 observer-only spelling for a handle read from an already-open
    /// schema-14 registry.
    pub(crate) fn observer_opencode_runtime_handle(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Option<OpenCodeRuntimeHandle>, StateError> {
        self.opencode_runtime_handle(runtime_id)
    }

    /// Binds an exact `OpenCode` session before native launch.  A pre-existing
    /// binding may only be reused for the same provider/session; a stale
    /// generation is updated transactionally and never adopted.
    #[allow(clippy::missing_errors_doc)]
    pub fn bind_opencode_session(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
        session: &ProviderSessionId,
        start_source: &str,
    ) -> Result<ProviderBinding, StateError> {
        if session.provider() != ProviderKind::OpenCode || !matches!(start_source, "new" | "resume")
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("runtime generation", expected_generation)?;
        validate_registry_text("start source", start_source)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let binding = bind_opencode_session_in_transaction(
            &transaction,
            runtime_id,
            expected_generation,
            session,
            start_source,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(binding)
    }

    /// Records the exact loopback handle for a prepared `OpenCode` generation.
    #[allow(clippy::missing_errors_doc)]
    pub fn record_opencode_runtime_handle(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
        endpoint_port: u16,
        version: &str,
        session: &ProviderSessionId,
    ) -> Result<OpenCodeRuntimeHandle, StateError> {
        if endpoint_port == 0 || session.provider() != ProviderKind::OpenCode {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("runtime generation", expected_generation)?;
        validate_provider_metadata(version)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
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
        let binding =
            load_binding(&transaction, runtime_id)?.ok_or(StateError::HookEvidenceMismatch)?;
        if binding.provider != ProviderKind::OpenCode || binding.native_session_id != *session {
            return Err(StateError::ProviderIdentityMismatch);
        }
        if binding.runtime_generation != expected_generation {
            return Err(StateError::HookEvidenceMismatch);
        }
        transaction
            .execute(
                "INSERT INTO opencode_runtime_handles (
                    runtime_id, runtime_generation, endpoint_host, endpoint_port,
                    version, native_session_id, observer_pid, observer_birth,
                    observer_status, revision
                 ) VALUES (?1, ?2, '127.0.0.1', ?3, ?4, ?5, NULL, NULL, 'starting', 1)
                 ON CONFLICT(runtime_id) DO UPDATE SET
                    runtime_generation = excluded.runtime_generation,
                    endpoint_host = excluded.endpoint_host,
                    endpoint_port = excluded.endpoint_port,
                    version = excluded.version,
                    native_session_id = excluded.native_session_id,
                    observer_pid = NULL,
                    observer_birth = NULL,
                    observer_status = 'starting',
                    revision = opencode_runtime_handles.revision + 1",
                params![
                    runtime_id.to_string(),
                    expected_generation,
                    i64::from(endpoint_port),
                    version,
                    session.native_id(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        let handle =
            load_opencode_handle(&transaction, runtime_id)?.ok_or(StateError::ConcurrentWrite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(handle)
    }

    /// Records the exact spawned observer process while its handle remains in
    /// `Starting`.  The handle revision prevents an old helper from claiming
    /// a newly reserved generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the handle identity or revision is stale, or the
    /// observer process identity is invalid.
    pub fn record_opencode_observer_started(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
        expected_handle_revision: Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<OpenCodeRuntimeHandle, StateError> {
        if observer_pid == 0 {
            return Err(StateError::InvalidRegistryField("observer PID"));
        }
        validate_registry_text("observer birth", observer_birth)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE opencode_runtime_handles SET observer_pid = ?1,
                    observer_birth = ?2, observer_status = 'starting', revision = revision + 1
                 WHERE runtime_id = ?3 AND runtime_generation = ?4
                   AND observer_status = 'starting' AND revision = ?5",
                params![
                    i64::from(observer_pid),
                    observer_birth,
                    runtime_id.to_string(),
                    expected_generation,
                    expected_handle_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        let handle =
            load_opencode_handle(&transaction, runtime_id)?.ok_or(StateError::ConcurrentWrite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(handle)
    }

    /// Moves one exact spawned helper from `Starting` to `Ready` after the
    /// child corroborates the native endpoint and verifies its own PID/birth.
    ///
    /// # Errors
    ///
    /// Returns an error when the handle identity or revision is stale, or the
    /// observer process identity is invalid.
    pub fn mark_opencode_observer_ready(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
        expected_handle_revision: Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<OpenCodeRuntimeHandle, StateError> {
        if observer_pid == 0 {
            return Err(StateError::InvalidRegistryField("observer PID"));
        }
        validate_registry_text("observer birth", observer_birth)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let changed = transaction
            .execute(
                "UPDATE opencode_runtime_handles SET observer_status = 'ready',
                    revision = revision + 1
                 WHERE runtime_id = ?1 AND runtime_generation = ?2
                   AND observer_status = 'starting' AND observer_pid = ?3
                   AND observer_birth = ?4 AND revision = ?5",
                params![
                    runtime_id.to_string(),
                    expected_generation,
                    i64::from(observer_pid),
                    observer_birth,
                    expected_handle_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        let handle =
            load_opencode_handle(&transaction, runtime_id)?.ok_or(StateError::ConcurrentWrite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(handle)
    }

    /// Marks one helper unknown only when its persisted PID/birth and handle
    /// revision still identify the caller's exact generation.
    ///
    /// # Errors
    ///
    /// Returns an error when the handle identity or revision is stale, or the
    /// observer process identity is invalid.
    pub fn mark_opencode_observer_unknown_exact(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
        expected_handle_revision: Revision,
        observer_pid: u32,
        observer_birth: &str,
    ) -> Result<(), StateError> {
        if observer_pid == 0 {
            return Err(StateError::InvalidRegistryField("observer PID"));
        }
        validate_registry_text("observer birth", observer_birth)?;
        let changed = self
            .connection
            .execute(
                "UPDATE opencode_runtime_handles SET observer_status = 'unknown',
                    revision = revision + 1
                 WHERE runtime_id = ?1 AND runtime_generation = ?2
                   AND observer_pid = ?3 AND observer_birth = ?4 AND revision = ?5",
                params![
                    runtime_id.to_string(),
                    expected_generation,
                    i64::from(observer_pid),
                    observer_birth,
                    expected_handle_revision.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::HookEvidenceMismatch)
        }
    }

    /// Applies one exact `OpenCode` observer hint to the already-bound Runtime.
    /// The observer supplies evidence only: provider, generation, cwd,
    /// session, observer PID/birth, and the current Runtime revision must all
    /// match before a neutral lifecycle transition is committed.
    ///
    /// # Errors
    ///
    /// Returns an error when any identity/revision evidence is stale or a
    /// settled message has no bounded exact ID.
    pub fn apply_opencode_lifecycle_observation(
        &mut self,
        runtime_id: RuntimeId,
        observation: &OpenCodeLifecycleObservation,
    ) -> Result<Revision, StateError> {
        if observation.session.provider() != ProviderKind::OpenCode || observation.observer_pid == 0
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("observer birth", &observation.observer_birth)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let (lifecycle, workstream_id) =
            validate_opencode_observation(&transaction, runtime_id, observation)?;
        let activity_at_millis = match &observation.hint {
            crate::provider::lifecycle::LifecycleHint::Working if lifecycle != "working" => {
                Some(SystemClock.now_millis()?)
            }
            crate::provider::lifecycle::LifecycleHint::Settled { .. } => {
                Some(SystemClock.now_millis()?)
            }
            crate::provider::lifecycle::LifecycleHint::Started
            | crate::provider::lifecycle::LifecycleHint::Working
            | crate::provider::lifecycle::LifecycleHint::Ended => None,
        };
        let accepted = apply_opencode_lifecycle_transition(
            &transaction,
            runtime_id,
            observation.runtime_revision,
            &lifecycle,
            workstream_id,
            observation,
        )?;
        if accepted {
            touch_workstream(&transaction, &workstream_id.to_string(), activity_at_millis)?;
        }
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(if accepted {
            observation.runtime_revision.next()
        } else {
            observation.runtime_revision
        })
    }

    /// Removes only the exact private `OpenCode` handle after its observer has
    /// been validated/stopped and its Runtime is being deliberately parked.
    #[allow(clippy::missing_errors_doc)]
    pub fn delete_opencode_runtime_handle(
        &mut self,
        runtime_id: RuntimeId,
        expected_generation: &str,
    ) -> Result<(), StateError> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM opencode_runtime_handles
                 WHERE runtime_id = ?1 AND runtime_generation = ?2",
                params![runtime_id.to_string(), expected_generation],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::HookEvidenceMismatch)
        }
    }

    /// Persists the exact private-pane process identity while the Runtime is
    /// prepared for its initial native lifecycle binding.  The PID and birth
    /// token are one identity pair: callers must provide both, and a later
    /// generation reservation clears both before launching again.
    ///
    /// # Errors
    ///
    /// Returns an error when the PID or birth token is invalid, or when the
    /// Runtime is stale or no longer in its prepared `starting` state.
    pub fn record_runtime_process_identity(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
        provider_pid: u32,
        process_birth: &str,
    ) -> Result<(), StateError> {
        if provider_pid == 0 {
            return Err(StateError::InvalidRegistryField("provider PID"));
        }
        validate_registry_text("process birth", process_birth)?;
        let changed = self
            .connection
            .execute(
                "UPDATE runtimes SET provider_pid = ?1, process_birth = ?2,
                    revision = revision + 1
                 WHERE runtime_id = ?3 AND lifecycle = 'starting' AND revision = ?4",
                params![
                    i64::from(provider_pid),
                    process_birth,
                    runtime_id.to_string(),
                    expected.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::ConcurrentWrite)
        }
    }

    /// Repairs a retained Runtime that has a persisted birth token but no
    /// provider PID. This is field-level reconciliation, not a schema
    /// migration. The caller must have freshly probed the exact private pane:
    /// the supplied birth must still match the durable record, the Runtime must
    /// not be stopped, and the optimistic revision must be exact.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity is invalid, the Runtime is stale, or
    /// the durable birth/lifecycle boundary is ambiguous.
    pub fn backfill_runtime_provider_pid(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
        provider_pid: u32,
        process_birth: &str,
    ) -> Result<(), StateError> {
        if provider_pid == 0 {
            return Err(StateError::InvalidRegistryField("provider PID"));
        }
        validate_registry_text("process birth", process_birth)?;
        let changed = self
            .connection
            .execute(
                "UPDATE runtimes SET provider_pid = ?1, revision = revision + 1
                 WHERE runtime_id = ?2 AND provider_pid IS NULL
                   AND process_birth = ?3 AND lifecycle != 'stopped' AND revision = ?4",
                params![
                    i64::from(provider_pid),
                    runtime_id.to_string(),
                    process_birth,
                    expected.value(),
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StateError::ConcurrentWrite)
        }
    }

    /// Returns the prepared provider process fingerprint for one exact runtime
    /// generation. This is evidence for hook ancestry, never hook authority by
    /// itself.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime/generation is unknown or no process
    /// fingerprint was recorded for the prepared launch.
    pub fn expected_hook_process_birth(
        &self,
        runtime_id: RuntimeId,
        generation: &str,
    ) -> Result<String, StateError> {
        let row: Option<(String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT tmux_generation, process_birth FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        let Some((recorded_generation, process_birth)) = row else {
            return Err(StateError::UnknownRuntime(runtime_id));
        };
        if recorded_generation != generation {
            return Err(StateError::HookEvidenceMismatch);
        }
        process_birth.ok_or(StateError::HookEvidenceMismatch)
    }

    /// Caches an exact managed thread name after a successful canonical provider mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding is missing, changed, or cannot be
    /// transactionally updated.
    pub fn record_thread_name(
        &mut self,
        runtime_id: RuntimeId,
        native_session_id: &ProviderSessionId,
        name: &str,
    ) -> Result<(), StateError> {
        self.record_thread_metadata(runtime_id, native_session_id, Some(name))
    }

    /// Records only the bounded canonical name from an exact provider metadata
    /// read. A missing native name is distinct from an unavailable read; the
    /// latter leaves the existing cached value untouched.
    ///
    /// # Errors
    ///
    /// Returns an error if the binding is missing, changed, or cannot be
    /// transactionally updated.
    pub fn record_thread_metadata(
        &mut self,
        runtime_id: RuntimeId,
        native_session_id: &ProviderSessionId,
        name: Option<&str>,
    ) -> Result<(), StateError> {
        let (name, name_state) = match name.filter(|value| !value.trim().is_empty()) {
            Some(name) => {
                validate_registry_text("thread name", name)?;
                (Some(name), "named")
            }
            None => (None, "known_empty"),
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let generation: String = transaction
            .query_row(
                "SELECT tmux_generation FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::UnknownRuntime(runtime_id))?;
        let changed = transaction
            .execute(
                "UPDATE provider_bindings SET observed_thread_name = ?1, name_state = ?2,
             revision = revision + 1 WHERE runtime_id = ?3 AND provider = ?4
             AND native_session_id = ?5 AND runtime_generation = ?6",
                params![
                    name,
                    name_state,
                    runtime_id.to_string(),
                    native_session_id.provider().as_str(),
                    native_session_id.native_id(),
                    generation,
                ],
            )
            .map_err(StateError::Sqlite)?;
        if changed == 1 {
            transaction.commit().map_err(StateError::Sqlite)
        } else {
            Err(StateError::HookEvidenceMismatch)
        }
    }

    /// Marks the reserved Runtime stopped after its exact private tmux server is parked.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown runtime, stale state, or failed transaction.
    pub fn mark_runtime_stopped(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
    ) -> Result<(), StateError> {
        let changed = self
            .connection
            .execute(
                "UPDATE runtimes SET lifecycle = 'stopped', revision = revision + 1
             WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
            )
            .map_err(StateError::Sqlite)?;
        if changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        Ok(())
    }

    /// Records that an owned private Runtime disappeared without a deliberate
    /// park or verified native end. Its provider binding and project files are
    /// retained, but neither a blank start nor a stale hook may continue it.
    ///
    /// This operation is idempotent after the first transition so cleanup of a
    /// failed recovery launch cannot erase the original recovery evidence.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown or stale runtime, or a failed atomic
    /// transition of the Runtime, Workstream, and attention state.
    pub fn mark_runtime_recovery_required(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
    ) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let workstream_id: String = transaction
            .query_row(
                "SELECT workstream_id FROM runtimes
                 WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        let workstream_id = Uuid::parse_str(&workstream_id)
            .map(WorkstreamId::from)
            .map_err(StateError::InvalidPersistedUuid)?;
        let runtime_changed = transaction
            .execute(
                "UPDATE runtimes SET lifecycle = 'unknown', revision = revision + 1
                 WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
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
                params![activity_sequence, workstream_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        if workstream_changed == 0 {
            let lifecycle: String = transaction
                .query_row(
                    "SELECT lifecycle FROM workstreams WHERE workstream_id = ?1",
                    [workstream_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(StateError::Sqlite)?;
            if lifecycle != "recovery_required" {
                return Err(StateError::ConcurrentWrite);
            }
        }
        ensure_recovery_attention_in_transaction(&transaction, workstream_id)?;
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Records an explicit user park after the exact private tmux server has
    /// stopped. Provider history and project files are retained, while the
    /// Workstream's durable lifecycle becomes `parked`.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown or stale runtime, or when the
    /// Workstream state cannot be updated atomically with the stopped Runtime.
    pub fn park_runtime(
        &mut self,
        runtime_id: RuntimeId,
        expected: Revision,
    ) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let workstream_id: String = transaction
            .query_row(
                "SELECT workstream_id FROM runtimes WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?
            .ok_or(StateError::ConcurrentWrite)?;
        let runtime_changed = transaction
            .execute(
                "UPDATE runtimes SET lifecycle = 'stopped', revision = revision + 1
                 WHERE runtime_id = ?1 AND revision = ?2",
                params![runtime_id.to_string(), expected.value()],
            )
            .map_err(StateError::Sqlite)?;
        let activity_sequence = next_activity_sequence(&transaction)?;
        let workstream_changed = transaction
            .execute(
                "UPDATE workstreams SET lifecycle = 'parked',
                    last_activity_sequence = ?1,
                    revision = revision + 1
                 WHERE workstream_id = ?2
                   AND lifecycle IN ('open', 'parked', 'recovery_required')",
                params![activity_sequence, workstream_id],
            )
            .map_err(StateError::Sqlite)?;
        if runtime_changed != 1 || workstream_changed != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)
    }
}

pub(in crate::state) fn load_binding(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<Option<ProviderBinding>, StateError> {
    let binding = transaction
        .query_row(
            "SELECT provider, native_session_id, start_source, last_settled_turn_id,
                    observed_thread_name, name_state, predecessor_native_session_id,
                    predecessor_effective_name, runtime_generation, revision
             FROM provider_bindings WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| {
                let provider = provider_kind_from_text(&row.get::<_, String>(0)?)
                    .map_err(to_from_sql_error)?;
                let native_session_id = ProviderSessionId::new(provider, row.get::<_, String>(1)?)
                    .map_err(to_from_sql_error)?;
                let predecessor_native_session_id = row
                    .get::<_, Option<String>>(6)?
                    .map(|value| ProviderSessionId::new(provider, value))
                    .transpose()
                    .map_err(to_from_sql_error)?;
                Ok(ProviderBinding {
                    runtime_id,
                    provider,
                    native_session_id,
                    start_source: row.get(2)?,
                    last_settled_turn_id: row.get(3)?,
                    observed_thread_name: row.get(4)?,
                    name_state: name_state_from_text(&row.get::<_, String>(5)?)
                        .map_err(to_from_sql_error)?,
                    predecessor_native_session_id,
                    predecessor_effective_name: row.get(7)?,
                    runtime_generation: {
                        let generation: String = row.get(8)?;
                        validate_registry_text("runtime generation", &generation)
                            .map_err(to_from_sql_error)?;
                        generation
                    },
                    revision: Revision::try_from(row.get::<_, i64>(9)?)
                        .map_err(to_from_sql_error)?,
                })
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    if let Some(binding) = &binding {
        let runtime_provider: String = transaction
            .query_row(
                "SELECT provider FROM runtimes WHERE runtime_id = ?1",
                [runtime_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if provider_kind_from_text(&runtime_provider)? != binding.provider
            || binding.native_session_id.provider() != binding.provider
            || binding
                .predecessor_native_session_id
                .as_ref()
                .is_some_and(|id| id.provider() != binding.provider)
        {
            return Err(StateError::ProviderIdentityMismatch);
        }
    }
    Ok(binding)
}

pub(in crate::state) fn row_to_runtime(
    row: &rusqlite::Row<'_>,
    workstream_id: WorkstreamId,
) -> rusqlite::Result<RuntimeRecord> {
    let runtime_id: String = row.get(0)?;
    let provider: String = row.get(1)?;
    let generation: String = row.get(2)?;
    let session: String = row.get(3)?;
    let cwd: String = row.get(4)?;
    let provider_pid: Option<i64> = row.get(5)?;
    let process_birth: Option<String> = row.get(6)?;
    let lifecycle: String = row.get(7)?;
    let revision: i64 = row.get(8)?;
    Ok(RuntimeRecord {
        runtime_id: Uuid::parse_str(&runtime_id)
            .map(RuntimeId::from)
            .map_err(to_from_sql_error)?,
        workstream_id,
        provider: provider_kind_from_text(&provider).map_err(to_from_sql_error)?,
        tmux_generation: generation,
        tmux_session: session,
        cwd: PathBuf::from(cwd),
        provider_pid: provider_pid
            .map(|pid| {
                u32::try_from(pid)
                    .map_err(|_| {
                        to_from_sql_error(StateError::InvalidPersistedValue(
                            "provider PID".to_owned(),
                        ))
                    })
                    .and_then(|pid| {
                        (pid != 0).then_some(pid).ok_or_else(|| {
                            to_from_sql_error(StateError::InvalidPersistedValue(
                                "provider PID".to_owned(),
                            ))
                        })
                    })
            })
            .transpose()?,
        process_birth,
        status: runtime_status_from_text(&lifecycle).map_err(to_from_sql_error)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

pub(in crate::state) fn row_to_runtime_with_id(
    row: &rusqlite::Row<'_>,
    runtime_id: RuntimeId,
    workstream_id: WorkstreamId,
) -> rusqlite::Result<RuntimeRecord> {
    let provider: String = row.get(1)?;
    let generation: String = row.get(2)?;
    let session: String = row.get(3)?;
    let cwd: String = row.get(4)?;
    let provider_pid: Option<i64> = row.get(5)?;
    let process_birth: Option<String> = row.get(6)?;
    let lifecycle: String = row.get(7)?;
    let revision: i64 = row.get(8)?;
    Ok(RuntimeRecord {
        runtime_id,
        workstream_id,
        provider: provider_kind_from_text(&provider).map_err(to_from_sql_error)?,
        tmux_generation: generation,
        tmux_session: session,
        cwd: PathBuf::from(cwd),
        provider_pid: provider_pid
            .map(|pid| {
                u32::try_from(pid)
                    .map_err(|_| {
                        to_from_sql_error(StateError::InvalidPersistedValue(
                            "provider PID".to_owned(),
                        ))
                    })
                    .and_then(|pid| {
                        (pid != 0).then_some(pid).ok_or_else(|| {
                            to_from_sql_error(StateError::InvalidPersistedValue(
                                "provider PID".to_owned(),
                            ))
                        })
                    })
            })
            .transpose()?,
        process_birth,
        status: runtime_status_from_text(&lifecycle).map_err(to_from_sql_error)?,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

pub(in crate::state) struct PersistedOpenCodeHandle {
    generation: String,
    host: String,
    port: i64,
    version: String,
    session: String,
    observer_pid: Option<i64>,
    observer_birth: Option<String>,
    status: String,
    revision: i64,
}

pub(in crate::state) fn load_opencode_handle(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<Option<OpenCodeRuntimeHandle>, StateError> {
    let Some(parts) = read_opencode_handle(transaction, runtime_id)? else {
        return Ok(None);
    };
    let observer_status = validate_opencode_handle(transaction, runtime_id, &parts)?;
    let native_session_id = ProviderSessionId::new(ProviderKind::OpenCode, &parts.session)?;
    Ok(Some(OpenCodeRuntimeHandle {
        runtime_id,
        runtime_generation: parts.generation,
        endpoint_host: parts.host,
        endpoint_port: u16::try_from(parts.port)
            .map_err(|_| StateError::InvalidPersistedValue("OpenCode endpoint port".to_owned()))?,
        version: parts.version,
        native_session_id,
        observer_pid: parts
            .observer_pid
            .map(|pid| {
                u32::try_from(pid)
                    .map_err(|_| StateError::InvalidPersistedValue("observer PID".to_owned()))
            })
            .transpose()?,
        observer_birth: parts.observer_birth,
        observer_status,
        revision: Revision::try_from(parts.revision)?,
    }))
}

pub(in crate::state) fn read_opencode_handle(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<Option<PersistedOpenCodeHandle>, StateError> {
    transaction
        .query_row(
            "SELECT runtime_generation, endpoint_host, endpoint_port, version,
                    native_session_id, observer_pid, observer_birth,
                    observer_status, revision
             FROM opencode_runtime_handles WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| {
                Ok(PersistedOpenCodeHandle {
                    generation: row.get(0)?,
                    host: row.get(1)?,
                    port: row.get(2)?,
                    version: row.get(3)?,
                    session: row.get(4)?,
                    observer_pid: row.get(5)?,
                    observer_birth: row.get(6)?,
                    status: row.get(7)?,
                    revision: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(StateError::Sqlite)
}

pub(in crate::state) fn validate_opencode_handle(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
    parts: &PersistedOpenCodeHandle,
) -> Result<OpenCodeObserverStatus, StateError> {
    validate_registry_text("runtime generation", &parts.generation)?;
    validate_provider_metadata(&parts.version)?;
    let (provider, current_generation): (String, String) = transaction
        .query_row(
            "SELECT provider, tmux_generation FROM runtimes WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(StateError::Sqlite)?;
    if provider_kind_from_text(&provider)? != ProviderKind::OpenCode
        || current_generation != parts.generation
        || parts.host != crate::provider::opencode::LOOPBACK_HOST
        || !(1..=65_535).contains(&parts.port)
    {
        return Err(StateError::ProviderIdentityMismatch);
    }
    let native_session_id = ProviderSessionId::new(ProviderKind::OpenCode, &parts.session)?;
    let binding =
        load_binding(transaction, runtime_id)?.ok_or(StateError::ProviderIdentityMismatch)?;
    if binding.provider != ProviderKind::OpenCode || binding.native_session_id != native_session_id
    {
        return Err(StateError::ProviderIdentityMismatch);
    }
    if binding.runtime_generation != parts.generation {
        return Err(StateError::ProviderIdentityMismatch);
    }
    let observer_status = match parts.status.as_str() {
        "starting" => OpenCodeObserverStatus::Starting,
        "ready" => OpenCodeObserverStatus::Ready,
        "unknown" => OpenCodeObserverStatus::Unknown,
        "stopped" => OpenCodeObserverStatus::Stopped,
        _ => return Err(StateError::InvalidPersistedValue(parts.status.clone())),
    };
    if parts.observer_pid.is_some() != parts.observer_birth.is_some()
        || parts.observer_pid.is_some_and(|pid| pid <= 0)
        || observer_status == OpenCodeObserverStatus::Ready
            && (parts.observer_pid.is_none() || parts.observer_birth.is_none())
    {
        return Err(StateError::InvalidPersistedValue(
            "OpenCode observer identity".to_owned(),
        ));
    }
    Ok(observer_status)
}

pub(in crate::state) fn load_current_binding(
    transaction: &rusqlite::Transaction<'_>,
    runtime_id: RuntimeId,
) -> Result<Option<ProviderBinding>, StateError> {
    let binding = load_binding(transaction, runtime_id)?;
    let Some(binding) = binding else {
        return Ok(None);
    };
    let generation: String = transaction
        .query_row(
            "SELECT tmux_generation FROM runtimes WHERE runtime_id = ?1",
            [runtime_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if binding.runtime_generation != generation {
        return Err(StateError::HookEvidenceMismatch);
    }
    Ok(Some(binding))
}
