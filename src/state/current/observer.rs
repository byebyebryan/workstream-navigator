//! Observer ownership and bounded degraded-marker state.
//!
//! This module owns observer-specific `SQLite` retry budgets, marker evidence,
//! and exact cleanup. It does not own provider lifecycle observation itself.

use super::*;

/// A monotonic budget for observer `SQLite` work. Only `BUSY` and `LOCKED`
/// errors are retried; all other database errors return immediately.
#[derive(Clone, Copy, Debug)]
pub struct ObserverDatabaseDeadline {
    deadline: Instant,
    retry_delay: Duration,
}

impl ObserverDatabaseDeadline {
    #[must_use]
    pub fn from_now(duration: Duration) -> Self {
        Self {
            deadline: Instant::now() + duration,
            retry_delay: Duration::from_millis(1),
        }
    }

    #[must_use]
    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            retry_delay: Duration::from_millis(1),
        }
    }

    #[must_use]
    pub fn deadline(self) -> Instant {
        self.deadline
    }

    pub fn run<T, F>(self, operation: F) -> Result<T, ObserverDatabaseError>
    where
        F: FnMut() -> Result<T, rusqlite::Error>,
    {
        self.run_with_degraded_reason(operation)
            .map_err(|(error, _)| error)
    }

    fn run_with_degraded_reason<T, F>(
        self,
        mut operation: F,
    ) -> Result<T, (ObserverDatabaseError, ObserverDegradedReason)>
    where
        F: FnMut() -> Result<T, rusqlite::Error>,
    {
        if Instant::now() >= self.deadline {
            return Err((
                ObserverDatabaseError::DeadlineExceeded,
                ObserverDegradedReason::BusyDeadline,
            ));
        }
        let mut retry_reason = ObserverDegradedReason::BusyDeadline;
        loop {
            match operation() {
                Ok(value) => {
                    return Ok(value);
                }
                Err(error) if is_retryable_observer_error(&error) => {
                    retry_reason = observer_retry_reason(&error);
                    let now = Instant::now();
                    if now >= self.deadline {
                        return Err((ObserverDatabaseError::DeadlineExceeded, retry_reason));
                    }
                    let remaining = self.deadline.saturating_duration_since(now);
                    thread_sleep(self.retry_delay.min(remaining));
                }
                Err(error) => {
                    return Err((ObserverDatabaseError::Sqlite(error), retry_reason));
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum ObserverDatabaseError {
    DeadlineExceeded,
    Sqlite(rusqlite::Error),
}

/// Runs one bounded observer operation and records the closed degraded marker
/// when only retryable contention survives until the deadline.
pub fn run_observer_write_with_degraded_marker<T, F>(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
    deadline: ObserverDatabaseDeadline,
    operation: F,
) -> Result<T, StateError>
where
    F: FnMut() -> Result<T, rusqlite::Error>,
{
    match deadline.run_with_degraded_reason(operation) {
        Ok(value) => {
            clear_observer_degraded_marker(root, runtime_id, runtime_generation)?;
            Ok(value)
        }
        Err((ObserverDatabaseError::DeadlineExceeded, reason)) => {
            write_observer_degraded_marker(root, runtime_id, runtime_generation, reason)?;
            Err(StateError::ObserverDatabaseDeadlineExceeded)
        }
        Err((ObserverDatabaseError::Sqlite(error), _)) => {
            write_observer_degraded_marker(
                root,
                runtime_id,
                runtime_generation,
                ObserverDegradedReason::CommitFailed,
            )?;
            Err(StateError::Sqlite(error))
        }
    }
}

fn is_retryable_observer_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            )
    )
}

fn observer_retry_reason(error: &rusqlite::Error) -> ObserverDegradedReason {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::DatabaseLocked => {
            ObserverDegradedReason::LockedDeadline
        }
        _ => ObserverDegradedReason::BusyDeadline,
    }
}

fn thread_sleep(duration: Duration) {
    if !duration.is_zero() {
        std::thread::sleep(duration);
    }
}

/// Closed marker reason recorded after exact observer authority has been
/// established but a bounded `SQLite` commit cannot complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverDegradedReason {
    BusyDeadline,
    LockedDeadline,
    CommitFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserverDegradedMarkerWire {
    version: u8,
    runtime_id: String,
    runtime_generation: String,
    reason: ObserverDegradedReason,
}

/// Computes the one exact marker path for a Runtime generation.  Callers do
/// not discover markers by scanning the run tree.
pub fn observer_degraded_marker_path(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<PathBuf, StateError> {
    validate_generation(runtime_generation)?;
    let mut digest = Sha256::new();
    digest.update(runtime_generation.as_bytes());
    let digest = digest.finalize();
    let mut digest_hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut digest_hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(root
        .join("run")
        .join(runtime_id.to_string())
        .join("observer-degraded")
        .join(digest_hex))
}

fn observer_degraded_marker_temp_path(path: &Path) -> PathBuf {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("marker");
    path.with_file_name(format!("{filename}{OBSERVER_DEGRADED_MARKER_TEMP_SUFFIX}"))
}

fn check_observer_marker_deadline(deadline: Instant) -> Result<(), StateError> {
    if Instant::now() >= deadline {
        Err(StateError::ObserverDatabaseDeadlineExceeded)
    } else {
        Ok(())
    }
}

/// Writes or verifies one private, bounded degraded marker.  The body carries
/// no event, turn, message, payload, or diagnostic text.
#[allow(
    clippy::too_many_lines,
    reason = "The atomic marker protocol keeps exact-path validation, promotion, and crash recovery in one bounded operation."
)]
pub fn write_observer_degraded_marker(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
    reason: ObserverDegradedReason,
) -> Result<PathBuf, StateError> {
    write_observer_degraded_marker_with_deadline(
        root,
        runtime_id,
        runtime_generation,
        reason,
        Instant::now() + OBSERVER_DEGRADED_MARKER_BUDGET,
    )
}

/// Writes or verifies one marker until the supplied monotonic deadline. The
/// default helper above starts the fixed 250 ms outer margin at entry; this
/// variant lets the observer transition preserve one absolute cutoff when a
/// caller has already started that margin.
#[allow(
    clippy::too_many_lines,
    reason = "The atomic marker protocol keeps exact-path validation, promotion, and crash recovery in one bounded operation."
)]
pub fn write_observer_degraded_marker_with_deadline(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
    reason: ObserverDegradedReason,
    deadline: Instant,
) -> Result<PathBuf, StateError> {
    check_observer_marker_deadline(deadline)?;
    let path = observer_degraded_marker_path(root, runtime_id, runtime_generation)?;
    check_observer_marker_deadline(deadline)?;
    let temp_path = observer_degraded_marker_temp_path(&path);
    let parent = path
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let runtime_directory = parent
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let run_directory = runtime_directory
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    check_observer_marker_deadline(deadline)?;
    let root_metadata =
        exact_artifact_metadata(root)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
    if !root_metadata.is_dir() || !is_private_owner_directory(&root_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    // Create each exact derived directory separately.  `create_dir_all` can
    // follow a swapped-in symlink before the later metadata check gets a
    // chance to reject it.
    for directory in [run_directory, runtime_directory, parent] {
        check_observer_marker_deadline(deadline)?;
        ensure_private_marker_directory(directory)?;
    }
    check_observer_marker_deadline(deadline)?;
    let wire = ObserverDegradedMarkerWire {
        version: 1,
        runtime_id: runtime_id.to_string(),
        runtime_generation: runtime_generation.to_owned(),
        reason,
    };
    let body = serde_json::to_vec(&wire).map_err(|_| StateError::InvalidObserverDegradedMarker)?;
    if body.len() > 1024 {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    check_observer_marker_deadline(deadline)?;
    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
        &path,
        &temp_path,
        runtime_id,
        runtime_generation,
    )?;
    check_observer_marker_deadline(deadline)?;
    match (final_reason, temp_reason) {
        (Some(final_reason), Some(temp_reason)) => {
            if final_reason != temp_reason || final_reason != reason {
                return Err(StateError::InvalidObserverDegradedMarker);
            }
            return Ok(path);
        }
        (Some(final_reason), None) => {
            if final_reason != reason {
                return Err(StateError::InvalidObserverDegradedMarker);
            }
            return Ok(path);
        }
        (None, Some(temp_reason)) => {
            if temp_reason != reason {
                return Err(StateError::InvalidObserverDegradedMarker);
            }
            match fs::rename(&temp_path, &path) {
                Ok(()) => {
                    check_observer_marker_deadline(deadline)?;
                    sync_directory(parent)?;
                    check_observer_marker_deadline(deadline)?;
                    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
                        &path,
                        &temp_path,
                        runtime_id,
                        runtime_generation,
                    )?;
                    check_observer_marker_deadline(deadline)?;
                    if final_reason == Some(reason)
                        && temp_reason.is_none_or(|value| value == reason)
                    {
                        return Ok(path);
                    }
                    return Err(StateError::InvalidObserverDegradedMarker);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    check_observer_marker_deadline(deadline)?;
                    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
                        &path,
                        &temp_path,
                        runtime_id,
                        runtime_generation,
                    )?;
                    check_observer_marker_deadline(deadline)?;
                    if final_reason == Some(reason)
                        && temp_reason.is_none_or(|value| value == reason)
                    {
                        return Ok(path);
                    }
                    return Err(StateError::InvalidObserverDegradedMarker);
                }
                Err(error) => return Err(StateError::io(&path, error)),
            }
        }
        (None, None) => {}
    }
    check_observer_marker_deadline(deadline)?;
    let mut file = match open_private_observer_marker_file(&temp_path) {
        Ok(file) => file,
        Err(error) => {
            check_observer_marker_deadline(deadline)?;
            let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
                &path,
                &temp_path,
                runtime_id,
                runtime_generation,
            )?;
            check_observer_marker_deadline(deadline)?;
            if final_reason == Some(reason) && temp_reason.is_none_or(|value| value == reason) {
                return Ok(path);
            }
            return Err(error);
        }
    };
    check_observer_marker_deadline(deadline)?;
    file.write_all(&body)
        .map_err(|error| StateError::io(&temp_path, error))?;
    check_observer_marker_deadline(deadline)?;
    file.sync_all()
        .map_err(|error| StateError::io(&temp_path, error))?;
    check_observer_marker_deadline(deadline)?;
    fs::rename(&temp_path, &path).map_err(|error| StateError::io(&path, error))?;
    check_observer_marker_deadline(deadline)?;
    sync_directory(parent)?;
    check_observer_marker_deadline(deadline)?;
    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
        &path,
        &temp_path,
        runtime_id,
        runtime_generation,
    )?;
    check_observer_marker_deadline(deadline)?;
    if final_reason == Some(reason) && temp_reason.is_none_or(|value| value == reason) {
        Ok(path)
    } else {
        Err(StateError::InvalidObserverDegradedMarker)
    }
}

/// Reads only the exact generation-derived marker path.
pub fn read_observer_degraded_marker(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<Option<ObserverDegradedReason>, StateError> {
    validate_observer_marker_ancestors(root, runtime_id)?;
    let path = observer_degraded_marker_path(root, runtime_id, runtime_generation)?;
    let temp_path = observer_degraded_marker_temp_path(&path);
    let (final_reason, temp_reason) = read_observer_degraded_marker_candidates(
        &path,
        &temp_path,
        runtime_id,
        runtime_generation,
    )?;
    match (final_reason, temp_reason) {
        (None, None) => Ok(None),
        (Some(reason), None) | (None, Some(reason)) => Ok(Some(reason)),
        (Some(final_reason), Some(temp_reason)) if final_reason == temp_reason => {
            Ok(Some(final_reason))
        }
        (Some(_), Some(_)) => Err(StateError::InvalidObserverDegradedMarker),
    }
}

/// Removes only the exact current-generation degraded marker and its exact
/// temporary candidate after validating both candidates.  This is an
/// explicit reconciliation step: it never scans the run tree, follows a
/// derived symlink, or touches a marker for another Runtime generation.
pub fn clear_observer_degraded_marker(
    root: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<(), StateError> {
    // Reading first validates the root ancestry, exact wire identity, and
    // agreement between final and temporary candidates.  A malformed or
    // foreign candidate therefore fails closed before anything is removed.
    if read_observer_degraded_marker(root, runtime_id, runtime_generation)?.is_none() {
        return Ok(());
    }
    let path = observer_degraded_marker_path(root, runtime_id, runtime_generation)?;
    let temp_path = observer_degraded_marker_temp_path(&path);
    remove_observer_degraded_marker_candidate(&path, runtime_id, runtime_generation)?;
    remove_observer_degraded_marker_candidate(&temp_path, runtime_id, runtime_generation)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn remove_observer_degraded_marker_candidate(
    path: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<(), StateError> {
    let Some(metadata) = exact_artifact_metadata(path)? else {
        return Ok(());
    };
    // Re-read the exact candidate immediately before removal and compare its
    // identity with the earlier lstat.  This rejects a foreign or swapped-in
    // candidate instead of unlinking an unrelated path at the derived name.
    read_observer_degraded_marker_candidate(path, runtime_id, runtime_generation)?
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let current =
        exact_artifact_metadata(path)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
    if file_identity(&metadata) != file_identity(&current)
        || !current.is_file()
        || !is_private_owner_file(&current)
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StateError::io(path, error)),
    }
}

fn validate_observer_marker_ancestors(
    root: &Path,
    runtime_id: RuntimeId,
) -> Result<(), StateError> {
    let Some(root_metadata) = exact_artifact_metadata(root)? else {
        return Ok(());
    };
    if !root_metadata.is_dir() || !is_private_owner_directory(&root_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let run_directory = root.join("run");
    let Some(run_metadata) = exact_artifact_metadata(&run_directory)? else {
        return Ok(());
    };
    if !run_metadata.is_dir() || !is_private_owner_directory(&run_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let runtime_directory = run_directory.join(runtime_id.to_string());
    let Some(runtime_metadata) = exact_artifact_metadata(&runtime_directory)? else {
        return Ok(());
    };
    if !runtime_metadata.is_dir() || !is_private_owner_directory(&runtime_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let marker_directory = runtime_directory.join("observer-degraded");
    if let Some(marker_metadata) = exact_artifact_metadata(&marker_directory)?
        && (!marker_metadata.is_dir() || !is_private_owner_directory(&marker_metadata))
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(())
}

fn read_observer_degraded_marker_candidates(
    final_path: &Path,
    temp_path: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<
    (
        Option<ObserverDegradedReason>,
        Option<ObserverDegradedReason>,
    ),
    StateError,
> {
    Ok((
        read_observer_degraded_marker_candidate(final_path, runtime_id, runtime_generation)?,
        read_observer_degraded_marker_candidate(temp_path, runtime_id, runtime_generation)?,
    ))
}

fn read_observer_degraded_marker_candidate(
    path: &Path,
    runtime_id: RuntimeId,
    runtime_generation: &str,
) -> Result<Option<ObserverDegradedReason>, StateError> {
    let Some(path_metadata) = exact_artifact_metadata(path)? else {
        return Ok(None);
    };
    validate_observer_marker_directories(path)?;
    if !path_metadata.is_file() || !is_private_owner_file(&path_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| StateError::io(path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StateError::io(path, error))?;
    if !metadata.is_file()
        || !is_private_owner_file(&metadata)
        || file_identity(&metadata) != file_identity(&path_metadata)
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let mut body = Vec::new();
    file.take(1025)
        .read_to_end(&mut body)
        .map_err(|error| StateError::io(path, error))?;
    if body.len() > 1024 {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    let wire: ObserverDegradedMarkerWire =
        serde_json::from_slice(&body).map_err(|_| StateError::InvalidObserverDegradedMarker)?;
    if wire.version != 1
        || wire.runtime_id != runtime_id.to_string()
        || wire.runtime_generation != runtime_generation
    {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(Some(wire.reason))
}

fn validate_observer_marker_directories(path: &Path) -> Result<(), StateError> {
    let parent = path
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let runtime_directory = parent
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let run_directory = runtime_directory
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    let root = run_directory
        .parent()
        .ok_or(StateError::InvalidObserverDegradedMarker)?;
    for directory in [root, run_directory, runtime_directory, parent] {
        let metadata =
            exact_artifact_metadata(directory)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
        if !metadata.is_dir() || !is_private_owner_directory(&metadata) {
            return Err(StateError::InvalidObserverDegradedMarker);
        }
    }
    Ok(())
}

fn ensure_private_marker_directory(path: &Path) -> Result<(), StateError> {
    match exact_artifact_metadata(path)? {
        Some(metadata) if !metadata.is_dir() || !is_current_owner(&metadata) => {
            return Err(StateError::InvalidObserverDegradedMarker);
        }
        Some(_) => {}
        None => match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StateError::io(path, error)),
        },
    }
    let metadata =
        exact_artifact_metadata(path)?.ok_or(StateError::InvalidObserverDegradedMarker)?;
    if !metadata.is_dir() || !is_current_owner(&metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut options = OpenOptions::new();
        options.read(true);
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
        let directory = options
            .open(path)
            .map_err(|error| StateError::io(path, error))?;
        let opened = directory
            .metadata()
            .map_err(|error| StateError::io(path, error))?;
        if !opened.is_dir() || file_identity(&opened) != file_identity(&metadata) {
            return Err(StateError::InvalidObserverDegradedMarker);
        }
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| StateError::io(path, error))?;
    }
    #[cfg(not(unix))]
    {
        super::super::utils::set_private_directory_permissions(path)?;
    }
    let private_metadata =
        fs::symlink_metadata(path).map_err(|error| StateError::io(path, error))?;
    if !private_metadata.is_dir() || !is_private_owner_directory(&private_metadata) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(())
}

fn validate_generation(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\0', '\n', '\r']) {
        return Err(StateError::InvalidObserverDegradedMarker);
    }
    Ok(())
}

impl CurrentState {
    /// Reads the retained owned Codex observer integration through the
    /// schema-15-only boundary. It is launch-readiness evidence only: the
    /// shell helper must still inspect the exact profile before it selects
    /// `--profile wsnav-observer` for a native Codex exec.
    pub(crate) fn codex_integration(
        &self,
    ) -> Result<Option<super::super::models::CodexIntegration>, StateError> {
        ensure_current_mode(self.mode)?;
        validate_schema15(&self.connection)?;
        self.connection
            .query_row(
                "SELECT canonical_profile_path, owner_id, profile_schema_version,
                    hook_executable_path, generated_content_hash, lifecycle, revision
                 FROM codex_integrations WHERE profile_name = ?1",
                [OBSERVER_PROFILE_NAME],
                super::super::host::row_to_integration,
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Runs one bounded observer write.  This method is intentionally limited
    /// to the transition bridge and does not expose Project or presentation
    /// operations through observer mode.
    pub fn observer_write<T, F>(
        &mut self,
        deadline: ObserverDatabaseDeadline,
        mut operation: F,
    ) -> Result<T, StateError>
    where
        F: FnMut(&Connection) -> Result<T, rusqlite::Error>,
    {
        if self.mode != StateMode::Current {
            return Err(StateError::MalformedHostSchema);
        }
        deadline
            .run(|| operation(&self.connection))
            .map_err(|error| match error {
                ObserverDatabaseError::DeadlineExceeded => {
                    StateError::ObserverDatabaseDeadlineExceeded
                }
                ObserverDatabaseError::Sqlite(error) => StateError::Sqlite(error),
            })
    }

    /// Runs one observer-transition operation against this handle's actual
    /// `SQLite` connection and records the exact generation-scoped degraded
    /// marker on bounded contention or a non-retryable write failure.  The
    /// operation remains a narrow state-owned closure until the provider
    /// adapter supplies typed lifecycle/binding/attention calls;
    /// it cannot accidentally open a second registry connection.
    pub fn observer_write_with_degraded_marker<T, F>(
        &mut self,
        runtime_id: RuntimeId,
        runtime_generation: &str,
        deadline: ObserverDatabaseDeadline,
        mut operation: F,
    ) -> Result<T, StateError>
    where
        F: FnMut(&Connection) -> Result<T, rusqlite::Error>,
    {
        if self.mode != StateMode::Current {
            return Err(StateError::MalformedHostSchema);
        }
        let root = self.root.clone();
        run_observer_write_with_degraded_marker(
            &root,
            runtime_id,
            runtime_generation,
            deadline,
            || operation(&self.connection),
        )
    }

    /// Runs one narrow observer-transition write and leaves a generation
    /// scoped degraded marker when bounded `SQLite` work cannot complete. The
    /// marker is written only after this handle has already proved the
    /// observer-transition mode and exact runtime generation.
    fn observer_transition_write<T, F>(
        &mut self,
        runtime_id: RuntimeId,
        runtime_generation: &str,
        deadline: ObserverDatabaseDeadline,
        operation: F,
    ) -> Result<T, StateError>
    where
        F: FnMut(&Connection) -> Result<T, rusqlite::Error>,
    {
        let root = self.root.clone();
        match self.observer_write(deadline, operation) {
            Ok(value) => {
                clear_observer_degraded_marker(&root, runtime_id, runtime_generation)?;
                Ok(value)
            }
            Err(StateError::ObserverDatabaseDeadlineExceeded) => {
                write_observer_degraded_marker(
                    &root,
                    runtime_id,
                    runtime_generation,
                    ObserverDegradedReason::BusyDeadline,
                )?;
                Err(StateError::ObserverDatabaseDeadlineExceeded)
            }
            // A compare-and-swap miss is an expected stale-observer outcome,
            // not a database failure.  In particular, do not write a
            // degraded marker for a replacement generation that won the
            // race while this observer was validating its evidence.
            Err(StateError::Sqlite(rusqlite::Error::QueryReturnedNoRows)) => {
                Err(StateError::ConcurrentWrite)
            }
            // State-owned semantic validation can happen inside the same
            // bounded closure as its transaction. Preserve that typed error
            // across the existing rusqlite-only retry seam instead of
            // treating malformed authority as a stale compare-and-swap.
            Err(StateError::Sqlite(rusqlite::Error::ToSqlConversionFailure(error))) => {
                match error.downcast::<StateError>() {
                    Ok(error) => Err(*error),
                    Err(error) => {
                        write_observer_degraded_marker(
                            &root,
                            runtime_id,
                            runtime_generation,
                            ObserverDegradedReason::CommitFailed,
                        )?;
                        Err(StateError::Sqlite(rusqlite::Error::ToSqlConversionFailure(
                            error,
                        )))
                    }
                }
            }
            Err(StateError::Sqlite(error)) => {
                write_observer_degraded_marker(
                    &root,
                    runtime_id,
                    runtime_generation,
                    ObserverDegradedReason::CommitFailed,
                )?;
                Err(StateError::Sqlite(error))
            }
            Err(error) => Err(error),
        }
    }

    /// Returns only current, process-fingerprinted Runtime rows eligible for a
    /// Codex hook. Process and private-tmux corroboration remains outside this
    /// state read; callers must still prove one exact live match.
    pub fn observer_hook_runtime_candidates(&self) -> Result<Vec<RuntimeRecord>, StateError> {
        ensure_current_mode(self.mode)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT runtimes.runtime_id, runtimes.provider,
                        runtimes.tmux_generation, runtimes.tmux_session,
                        runtimes.cwd, runtimes.provider_pid,
                        runtimes.process_birth, runtimes.lifecycle,
                        runtimes.revision, runtimes.workstream_id
                 FROM runtimes
                 JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
                 WHERE runtimes.lifecycle IN ('starting', 'idle', 'working', 'attention')
                   AND runtimes.provider_pid IS NOT NULL
                   AND runtimes.process_birth IS NOT NULL
                   AND runtimes.provider = workstreams.provider",
            )
            .map_err(StateError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                let workstream_id: String = row.get(9)?;
                let workstream_id = Uuid::parse_str(&workstream_id).map_err(to_sql_error)?;
                row_to_runtime(row, WorkstreamId::from(workstream_id))
            })
            .map_err(StateError::Sqlite)?;
        rows.map(|row| row.map_err(StateError::Sqlite)).collect()
    }

    /// Records bounded Codex thread-name metadata through the observer
    /// transition authority. The exact Runtime generation and native session
    /// are part of the compare target; no prompt, response, or turn payload is
    /// persisted.
    pub fn observer_record_thread_metadata(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        native_session_id: &ProviderSessionId,
        name: Option<&str>,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<(), StateError> {
        if native_session_id.provider() != ProviderKind::Codex {
            return Err(StateError::ProviderIdentityMismatch);
        }
        validate_registry_text("runtime generation", generation)?;
        let (name, name_state) = match name.filter(|value| !value.trim().is_empty()) {
            Some(name) => {
                validate_registry_text("thread name", name)?;
                (Some(name), "named")
            }
            None => (None, "known_empty"),
        };
        self.observer_transition_write(runtime_id, generation, deadline, |connection| {
            let transaction = connection.unchecked_transaction()?;
            let changed = transaction.execute(
                "UPDATE provider_bindings SET observed_thread_name = ?1,
                         name_state = ?2, revision = revision + 1
                     WHERE runtime_id = ?3 AND provider = 'codex'
                       AND native_session_id = ?4 AND runtime_generation = ?5
                       AND EXISTS (
                           SELECT 1 FROM runtimes
                           WHERE runtime_id = ?3 AND tmux_generation = ?5
                       )",
                params![
                    name,
                    name_state,
                    runtime_id.to_string(),
                    native_session_id.native_id(),
                    generation,
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// Applies one exact Codex hook observation through the narrow current
    /// observer-transition authority. All `SQLite` work, including validation,
    /// is inside the bounded generation-scoped write so a busy/locked
    /// database records the closed degraded marker before returning.
    pub fn observer_apply_codex_lifecycle_observation(
        &mut self,
        runtime_id: RuntimeId,
        generation: &str,
        observation: &LifecycleObservation,
        deadline: ObserverDatabaseDeadline,
    ) -> Result<(), StateError> {
        validate_registry_text("runtime generation", generation)?;
        let activity_at_millis = match observation.event {
            LifecycleEvent::UserPromptSubmit | LifecycleEvent::Stop => {
                Some(SystemClock.now_millis()?)
            }
            LifecycleEvent::SessionStart | LifecycleEvent::SessionEnd => None,
        };
        self.observer_transition_write(runtime_id, generation, deadline, |connection| {
            let transaction = connection.unchecked_transaction()?;
            let runtime = transaction
                .query_row(
                    "SELECT runtimes.workstream_id, runtimes.provider,
                                runtimes.tmux_generation, runtimes.cwd,
                                runtimes.lifecycle, runtimes.revision,
                                workstreams.provider, workstreams.lifecycle
                         FROM runtimes
                         JOIN workstreams ON workstreams.workstream_id = runtimes.workstream_id
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
                .map_err(StateError::Sqlite)
                .map_err(Self::state_error_as_sqlite)?
                .ok_or_else(|| {
                    Self::state_error_as_sqlite(StateError::UnknownRuntime(runtime_id))
                })?;
            let workstream_id = Uuid::parse_str(&runtime.0)
                .map(WorkstreamId::from)
                .map_err(StateError::InvalidPersistedUuid)
                .map_err(Self::state_error_as_sqlite)?;
            let provider = super::super::utils::provider_kind_from_text(&runtime.1)
                .map_err(Self::state_error_as_sqlite)?;
            let workstream_provider = super::super::utils::provider_kind_from_text(&runtime.6)
                .map_err(Self::state_error_as_sqlite)?;
            if provider != ProviderKind::Codex
                || workstream_provider != ProviderKind::Codex
                || provider != workstream_provider
                || runtime.2 != generation
                || runtime.3 != observation.cwd
            {
                return Err(Self::state_error_as_sqlite(
                    StateError::HookEvidenceMismatch,
                ));
            }
            let runtime_revision = Revision::try_from(runtime.5)
                .map_err(|error| Self::state_error_as_sqlite(StateError::Domain(error)))?;
            let workstream_lifecycle =
                workstream_lifecycle_from_text(&runtime.7).map_err(Self::state_error_as_sqlite)?;
            let existing =
                load_binding(&transaction, runtime_id).map_err(Self::state_error_as_sqlite)?;
            let observed_session =
                ProviderSessionId::new(provider, observation.native_session_id.clone())
                    .map_err(|error| Self::state_error_as_sqlite(StateError::Domain(error)))?;
            apply_lifecycle_event(
                LifecycleEventContext {
                    transaction: &transaction,
                    runtime_id,
                    provider,
                    runtime_status: &runtime.4,
                    runtime_revision,
                    generation,
                    workstream_id,
                    workstream_lifecycle,
                    existing,
                    observed_session,
                },
                observation,
            )
            .map_err(Self::state_error_as_sqlite)?;
            touch_workstream(&transaction, &runtime.0, activity_at_millis)
                .map_err(Self::state_error_as_sqlite)?;
            transaction.commit()?;
            Ok(())
        })
    }

    fn state_error_as_sqlite(error: StateError) -> rusqlite::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }

    /// Installs or acquires 's stable provisional lease from a normal
    /// schema-15 opening. The same pending-to-ready protocol is used as the
    /// bootstrap seam; no other lease can authorize this operation.
    ///
    /// The returned descriptor remains `CLOEXEC`, locked, and bound to the
    /// exact root/inode/generation. No marker, tmux server, Runtime, or
    /// provider process is created here.
    pub fn acquire_provisional_lease(&mut self) -> Result<ProvisionalLease, StateError> {
        ensure_current_mode(self.mode)?;
        validate_schema15(&self.connection)?;
        let metadata = load_provisional_lock_metadata(&self.connection)?;
        let root_directory = open_bootstrap_root_directory(&self.root)
            .map_err(|_| StateError::InvalidProvisionalLease)?;
        let root = self.root.clone();
        let root_metadata = root_directory
            .metadata()
            .map_err(|_| StateError::InvalidProvisionalLease)?;
        let root_identity = file_identity(&root_metadata);
        let lock_path = root.join(PROVISIONAL_LOCK_FILE);
        let expected_contents = provisional_lock_contents(&metadata.host_id, metadata.generation)?;
        let (file, lock_identity) = match metadata.phase {
            ProvisionalLockPhase::Pending => {
                let file = match exact_artifact_metadata(&lock_path)? {
                    None => {
                        let mut file = open_private_provisional_file_at(
                            &root_directory,
                            PROVISIONAL_LOCK_FILE,
                            &lock_path,
                            true,
                        )?;
                        file.write_all(&expected_contents)
                            .map_err(|error| StateError::io(&lock_path, error))?;
                        file.sync_all()
                            .map_err(|error| StateError::io(&lock_path, error))?;
                        sync_directory(&root)?;
                        file
                    }
                    Some(_) => open_private_provisional_file_at(
                        &root_directory,
                        PROVISIONAL_LOCK_FILE,
                        &lock_path,
                        false,
                    )?,
                };
                let identity =
                    validate_provisional_lock_file(&file, &lock_path, &expected_contents)?;
                (file, identity)
            }
            ProvisionalLockPhase::Ready { expected_identity } => {
                let file = open_private_provisional_file_at(
                    &root_directory,
                    PROVISIONAL_LOCK_FILE,
                    &lock_path,
                    false,
                )?;
                let identity =
                    validate_provisional_lock_file(&file, &lock_path, &expected_contents)?;
                if identity != expected_identity {
                    return Err(StateError::InvalidProvisionalLease);
                }
                (file, identity)
            }
        };
        let file = nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
            .map_err(|(_file, _error)| StateError::ProvisionalLeaseBusy)?;
        let provisional = ProvisionalLease::new(
            root,
            root_identity,
            lock_path,
            lock_identity,
            metadata.generation,
            expected_contents,
            file,
        );
        provisional.revalidate_for_mutation(&self.root)?;
        if matches!(metadata.phase, ProvisionalLockPhase::Pending) {
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(StateError::Sqlite)?;
            let changed = transaction
                .execute(
                    "UPDATE host_operational_metadata
                     SET provisional_lock_phase = 'ready',
                         provisional_lock_device = ?1,
                         provisional_lock_inode = ?2
                     WHERE singleton = 1
                       AND provisional_lease_generation = ?3
                       AND provisional_lock_phase = 'pending'",
                    params![
                        i64::try_from(lock_identity.device)
                            .map_err(|_| StateError::InvalidProvisionalLease)?,
                        i64::try_from(lock_identity.inode)
                            .map_err(|_| StateError::InvalidProvisionalLease)?,
                        metadata.generation,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            if changed != 1 {
                return Err(StateError::ConcurrentWrite);
            }
            validate_schema15(&transaction)?;
            transaction.commit().map_err(StateError::Sqlite)?;
            provisional.revalidate_for_mutation(&self.root)?;
        }
        Ok(provisional)
    }
}
