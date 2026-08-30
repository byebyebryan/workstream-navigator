use super::{
    BTreeMap, BTreeSet, MAX_PROVISIONAL_INVENTORY_ENTRIES, OnboardingOperationInventory,
    OnboardingPhase, OsString, PRESENTATION_DIRECTORY, PROVISIONAL_MARKER_FILE, Path, PathBuf,
    Presentation, ProvisionalInventory, ProvisionalInventoryError, ProvisionalPhase,
    ProvisionalSlot, RuntimePaths, fs, is_private_owner_directory, presentation_session_name,
    read_marker,
};

/// Cross-checks all presentation markers against exact durable onboarding
/// journal claims and registered Runtime paths. Any malformed, changed,
/// markerless, or unregistered runtime-shaped evidence is a closed refusal;
/// the function never makes a new candidate to evade ambiguity.
///
/// The registered paths and operation inventory must come from the same
/// schema-15 passive read while the caller retains the stable provisional
/// lease. They are intentionally private classifier inputs, not Navigator
/// projection data.
#[allow(
    dead_code,
    clippy::too_many_lines,
    reason = "the singleton proof intentionally keeps every marker, journal, and runtime-path cross-check in one fail-closed classifier"
)]
pub(crate) fn classify_provisional_inventory(
    state_root: &Path,
    registered_runtime_paths: &[RuntimePaths],
    operations: &[OnboardingOperationInventory],
) -> Result<ProvisionalInventory, ProvisionalInventoryError> {
    if registered_runtime_paths.len() > MAX_PROVISIONAL_INVENTORY_ENTRIES
        || operations.len() > MAX_PROVISIONAL_INVENTORY_ENTRIES
    {
        return Err(ProvisionalInventoryError::Ambiguous);
    }
    let state_root = canonical_inventory_root(state_root)?;
    let mut operations_by_id = BTreeMap::new();
    for operation in operations {
        if operations_by_id
            .insert(operation.operation_id.as_uuid(), operation)
            .is_some()
        {
            return Err(ProvisionalInventoryError::Ambiguous);
        }
    }

    let mut matched_operations = BTreeSet::new();
    let mut allowed_runtime_directories = registered_runtime_paths
        .iter()
        .map(|paths| paths.directory.clone())
        .collect::<BTreeSet<_>>();
    let mut occupied = false;
    let presentation_root = state_root.join(PRESENTATION_DIRECTORY);
    match fs::symlink_metadata(&presentation_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ProvisionalInventoryError::Unavailable),
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !is_private_owner_directory(&metadata) =>
        {
            return Err(ProvisionalInventoryError::Ambiguous);
        }
        Ok(_) => {
            let entries = fs::read_dir(&presentation_root)
                .map_err(|_| ProvisionalInventoryError::Unavailable)?;
            for (count, entry) in entries.enumerate() {
                if count >= MAX_PROVISIONAL_INVENTORY_ENTRIES {
                    return Err(ProvisionalInventoryError::Ambiguous);
                }
                let entry = entry.map_err(|_| ProvisionalInventoryError::Unavailable)?;
                let directory = entry.path();
                let metadata = fs::symlink_metadata(&directory)
                    .map_err(|_| ProvisionalInventoryError::Unavailable)?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_dir()
                    || !is_private_owner_directory(&metadata)
                    || presentation_session_name(&directory).is_none()
                {
                    return Err(ProvisionalInventoryError::Ambiguous);
                }
                let context = Presentation::context_from_directory(&state_root, &directory)
                    .map_err(|_| ProvisionalInventoryError::Ambiguous)?;
                let marker_path = directory.join(PROVISIONAL_MARKER_FILE);
                let slot = match fs::symlink_metadata(&marker_path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => return Err(ProvisionalInventoryError::Unavailable),
                    Ok(_) => Some(
                        read_marker(&state_root, &directory)
                            .map_err(|_| ProvisionalInventoryError::Ambiguous)?,
                    ),
                };
                let Some(slot) = slot else {
                    continue;
                };
                if slot.presentation_id() != context.presentation_id()
                    || slot.presentation_revision() != context.presentation_revision()
                {
                    return Err(ProvisionalInventoryError::Ambiguous);
                }
                match slot.phase() {
                    ProvisionalPhase::Materializing => {
                        if operations
                            .iter()
                            .any(|operation| operation.runtime_id == slot.candidate_runtime_id())
                        {
                            return Err(ProvisionalInventoryError::Ambiguous);
                        }
                        occupied = true;
                        allowed_runtime_directories.insert(slot.runtime_paths().directory.clone());
                    }
                    ProvisionalPhase::Materialized => {
                        match_materialized_slot_operation(
                            &slot,
                            operations,
                            &mut matched_operations,
                        )?;
                        occupied = true;
                        allowed_runtime_directories.insert(slot.runtime_paths().directory.clone());
                    }
                    ProvisionalPhase::HandoffIssued => {
                        match_slot_operation(
                            &slot,
                            &operations_by_id,
                            &mut matched_operations,
                            &[
                                OnboardingPhase::CapabilityIssued,
                                OnboardingPhase::RolledBack,
                            ],
                        )?;
                        occupied = true;
                        allowed_runtime_directories.insert(slot.runtime_paths().directory.clone());
                    }
                    ProvisionalPhase::RuntimeOwnedLaunching => {
                        match_slot_operation(
                            &slot,
                            &operations_by_id,
                            &mut matched_operations,
                            &[
                                OnboardingPhase::RuntimeOwnedLaunching,
                                OnboardingPhase::ProviderPreparation,
                                OnboardingPhase::ProviderExternalEffectStarted,
                                OnboardingPhase::ProviderExecStarted,
                                OnboardingPhase::KnownAbsentExec,
                                OnboardingPhase::RecoveryRequired,
                                OnboardingPhase::ProviderExecProven,
                            ],
                        )?;
                        require_registered_runtime_path(&slot, registered_runtime_paths)?;
                    }
                    ProvisionalPhase::ProviderExecProven => {
                        match_slot_operation(
                            &slot,
                            &operations_by_id,
                            &mut matched_operations,
                            &[OnboardingPhase::ProviderExecProven],
                        )?;
                        require_registered_runtime_path(&slot, registered_runtime_paths)?;
                    }
                    ProvisionalPhase::Cancelled => {
                        return Err(ProvisionalInventoryError::Ambiguous);
                    }
                }
            }
        }
    }
    if operations.iter().any(|operation| {
        !matches!(
            operation.phase,
            OnboardingPhase::RolledBack | OnboardingPhase::ProviderExecProven
        ) && !matched_operations.contains(&operation.operation_id.as_uuid())
    }) {
        return Err(ProvisionalInventoryError::Ambiguous);
    }
    classify_runtime_namespace(&state_root, &allowed_runtime_directories)?;
    Ok(if occupied {
        ProvisionalInventory::Occupied
    } else {
        ProvisionalInventory::Vacant
    })
}

fn canonical_inventory_root(state_root: &Path) -> Result<PathBuf, ProvisionalInventoryError> {
    let metadata =
        fs::symlink_metadata(state_root).map_err(|_| ProvisionalInventoryError::Unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(ProvisionalInventoryError::Ambiguous);
    }
    let state_root =
        fs::canonicalize(state_root).map_err(|_| ProvisionalInventoryError::Unavailable)?;
    let metadata =
        fs::symlink_metadata(&state_root).map_err(|_| ProvisionalInventoryError::Unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(ProvisionalInventoryError::Ambiguous);
    }
    Ok(state_root)
}

fn match_materialized_slot_operation(
    slot: &ProvisionalSlot,
    operations: &[OnboardingOperationInventory],
    matched_operations: &mut BTreeSet<uuid::Uuid>,
) -> Result<(), ProvisionalInventoryError> {
    let matches = operations
        .iter()
        .filter(|operation| operation.runtime_id == slot.candidate_runtime_id())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(()),
        [operation]
            if matches!(
                operation.phase,
                OnboardingPhase::CapabilityIssued | OnboardingPhase::RolledBack
            ) =>
        {
            matched_operations.insert(operation.operation_id.as_uuid());
            Ok(())
        }
        _ => Err(ProvisionalInventoryError::Ambiguous),
    }
}

fn match_slot_operation(
    slot: &ProvisionalSlot,
    operations: &BTreeMap<uuid::Uuid, &OnboardingOperationInventory>,
    matched_operations: &mut BTreeSet<uuid::Uuid>,
    allowed_phases: &[OnboardingPhase],
) -> Result<(), ProvisionalInventoryError> {
    let request = slot
        .handoff_request()
        .ok_or(ProvisionalInventoryError::Ambiguous)?;
    let operation = operations
        .get(&request)
        .ok_or(ProvisionalInventoryError::Ambiguous)?;
    if operation.runtime_id != slot.candidate_runtime_id()
        || !allowed_phases.contains(&operation.phase)
        || !matched_operations.insert(request)
    {
        return Err(ProvisionalInventoryError::Ambiguous);
    }
    Ok(())
}

fn require_registered_runtime_path(
    slot: &ProvisionalSlot,
    registered_runtime_paths: &[RuntimePaths],
) -> Result<(), ProvisionalInventoryError> {
    registered_runtime_paths
        .iter()
        .any(|paths| paths == slot.runtime_paths())
        .then_some(())
        .ok_or(ProvisionalInventoryError::Ambiguous)
}

fn classify_runtime_namespace(
    state_root: &Path,
    allowed_runtime_directories: &BTreeSet<PathBuf>,
) -> Result<(), ProvisionalInventoryError> {
    let runtime_root = state_root.join("run");
    let metadata = match fs::symlink_metadata(&runtime_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(ProvisionalInventoryError::Unavailable),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || !is_private_owner_directory(&metadata)
    {
        return Err(ProvisionalInventoryError::Ambiguous);
    }
    let entries =
        fs::read_dir(&runtime_root).map_err(|_| ProvisionalInventoryError::Unavailable)?;
    for (count, entry) in entries.enumerate() {
        if count >= MAX_PROVISIONAL_INVENTORY_ENTRIES {
            return Err(ProvisionalInventoryError::Ambiguous);
        }
        let entry = entry.map_err(|_| ProvisionalInventoryError::Unavailable)?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with("runtime-"))
            && !allowed_runtime_directories.contains(&entry.path())
        {
            return Err(ProvisionalInventoryError::Ambiguous);
        }
    }
    Ok(())
}

impl Presentation {
    pub(crate) fn provider_respawn_for_command(
        &self,
        provider: &str,
        command: Vec<OsString>,
    ) -> Vec<OsString> {
        let mut arguments = vec![
            "respawn-pane".into(),
            "-k".into(),
            "-t".into(),
            self.pane_target(provider).into(),
        ];
        arguments.extend(command);
        arguments
    }

    /// The provisional attach command is deliberately direct argv: no shell,
    /// provider command, or user-derived string crosses into the outer pane.
    /// `env -u TMUX` prevents tmux's nested-server warning path from changing
    /// an attachment to the exact private Runtime socket.
    pub(super) fn provisional_attach_command(paths: &RuntimePaths) -> Vec<OsString> {
        vec![
            "env".into(),
            "-u".into(),
            "TMUX".into(),
            "tmux".into(),
            "-u".into(),
            "-S".into(),
            paths.socket.clone().into_os_string(),
            "attach-session".into(),
            "-t".into(),
            paths.session_name.clone().into(),
        ]
    }
}
