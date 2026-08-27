use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use crate::domain::{
    OperationKind, OperationPhase, ProviderKind, RuntimeStatus, WorkstreamLifecycle,
    WorkstreamOrigin,
};
use crate::provider::names::NameState;

use super::models::{IntegrationLifecycle, StateError};
use super::schema::{MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES, MAX_PROJECT_BROWSER_ROOT_BYTES};

pub(in crate::state) fn to_from_sql_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

pub(in crate::state) const fn operation_kind_text(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Onboard => "onboard",
        OperationKind::Start => "start",
        OperationKind::Fork => "fork",
    }
}

pub(in crate::state) fn operation_kind_from_text(value: &str) -> Result<OperationKind, StateError> {
    match value {
        "onboard" => Ok(OperationKind::Onboard),
        "start" => Ok(OperationKind::Start),
        "fork" => Ok(OperationKind::Fork),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

pub(in crate::state) const fn workstream_origin_text(origin: WorkstreamOrigin) -> &'static str {
    match origin {
        WorkstreamOrigin::External => "external",
        WorkstreamOrigin::Independent => "independent",
        WorkstreamOrigin::Fork => "fork",
    }
}

pub(in crate::state) const fn operation_phase_text(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::Prepared => "prepared",
        OperationPhase::CapabilityIssued => "capability_issued",
        OperationPhase::RuntimeOwnedLaunching => "runtime_owned_launching",
        OperationPhase::ProviderPreparation => "provider_preparation",
        OperationPhase::ExternalEffectStarted => "external_effect_started",
        OperationPhase::ProviderExecStarted => "provider_exec_started",
        OperationPhase::ProviderExecProven => "provider_exec_proven",
        OperationPhase::ExecFailedKnownAbsent => "exec_failed_known_absent",
        OperationPhase::RolledBack => "rolled_back",
        OperationPhase::AwaitingReconciliation => "awaiting_reconciliation",
        OperationPhase::Committed => "committed",
        OperationPhase::RecoveryRequired => "recovery_required",
        OperationPhase::Failed => "failed",
    }
}

pub(in crate::state) fn operation_phase_from_text(
    value: &str,
) -> Result<OperationPhase, StateError> {
    match value {
        "prepared" => Ok(OperationPhase::Prepared),
        "capability_issued" => Ok(OperationPhase::CapabilityIssued),
        "runtime_owned_launching" => Ok(OperationPhase::RuntimeOwnedLaunching),
        "provider_preparation" => Ok(OperationPhase::ProviderPreparation),
        "external_effect_started" => Ok(OperationPhase::ExternalEffectStarted),
        "provider_exec_started" => Ok(OperationPhase::ProviderExecStarted),
        "provider_exec_proven" => Ok(OperationPhase::ProviderExecProven),
        "exec_failed_known_absent" => Ok(OperationPhase::ExecFailedKnownAbsent),
        "rolled_back" => Ok(OperationPhase::RolledBack),
        "awaiting_reconciliation" => Ok(OperationPhase::AwaitingReconciliation),
        "committed" => Ok(OperationPhase::Committed),
        "recovery_required" => Ok(OperationPhase::RecoveryRequired),
        "failed" => Ok(OperationPhase::Failed),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

pub(in crate::state) fn runtime_status_from_text(value: &str) -> Result<RuntimeStatus, StateError> {
    match value {
        "starting" => Ok(RuntimeStatus::Starting),
        "idle" => Ok(RuntimeStatus::Idle),
        "working" => Ok(RuntimeStatus::Working),
        "attention" => Ok(RuntimeStatus::Attention),
        "stopped" => Ok(RuntimeStatus::Stopped),
        "unknown" => Ok(RuntimeStatus::Unknown),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

pub(in crate::state) fn provider_kind_from_text(value: &str) -> Result<ProviderKind, StateError> {
    value
        .parse::<ProviderKind>()
        .map_err(|_| StateError::InvalidPersistedValue(format!("provider kind {value}")))
}

pub(in crate::state) const fn default_provider_kind() -> ProviderKind {
    ProviderKind::Codex
}

pub(in crate::state) fn workstream_lifecycle_from_text(
    value: &str,
) -> Result<WorkstreamLifecycle, StateError> {
    match value {
        "open" => Ok(WorkstreamLifecycle::Open),
        "parked" => Ok(WorkstreamLifecycle::Parked),
        "recovery_required" => Ok(WorkstreamLifecycle::RecoveryRequired),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

pub(in crate::state) fn name_state_from_text(value: &str) -> Result<NameState, StateError> {
    match value {
        "named" => Ok(NameState::Named),
        "known_empty" => Ok(NameState::KnownEmpty),
        "unavailable" => Ok(NameState::Unavailable),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

pub(in crate::state) const fn integration_lifecycle_text(
    lifecycle: IntegrationLifecycle,
) -> &'static str {
    match lifecycle {
        IntegrationLifecycle::TrustPending => "trust_pending",
        IntegrationLifecycle::Ready => "ready",
        IntegrationLifecycle::Modified => "modified",
        IntegrationLifecycle::Disabled => "disabled",
    }
}

pub(in crate::state) fn integration_lifecycle_from_text(
    value: &str,
) -> Result<IntegrationLifecycle, StateError> {
    match value {
        "trust_pending" => Ok(IntegrationLifecycle::TrustPending),
        "ready" => Ok(IntegrationLifecycle::Ready),
        "modified" => Ok(IntegrationLifecycle::Modified),
        "disabled" => Ok(IntegrationLifecycle::Disabled),
        _ => Err(StateError::InvalidPersistedValue(value.to_owned())),
    }
}

pub(in crate::state) fn validate_registry_text(
    name: &'static str,
    value: &str,
) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') || value.contains('\n') {
        return Err(StateError::InvalidRegistryField(name));
    }
    Ok(())
}

pub(in crate::state) fn validate_provider_metadata(value: &str) -> Result<(), StateError> {
    if value.is_empty() || value.len() > 256 || value.contains(['\n', '\r']) {
        return Err(StateError::InvalidProviderMetadata);
    }
    Ok(())
}

pub(in crate::state) fn validate_project_display_name(value: &str) -> Result<(), StateError> {
    if value.trim().is_empty() || value.chars().count() > 128 || value.contains(['\0', '\n', '\r'])
    {
        return Err(StateError::InvalidProjectDisplayName);
    }
    Ok(())
}

pub(in crate::state) fn validate_remote_identity_display(
    value: Option<&str>,
) -> Result<(), StateError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim().is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.contains('\\')
        || value.contains('@')
        || value.contains("//")
        || value.contains(['?', '#'])
        || value.starts_with('/')
    {
        return Err(StateError::InvalidRegistryField("remote identity display"));
    }
    Ok(())
}

pub(in crate::state) fn validate_repository_fingerprint(
    value: Option<&str>,
) -> Result<(), StateError> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(hash) = value.strip_prefix("git-remote-v1:") else {
        return Err(StateError::InvalidRepositoryFingerprint);
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StateError::InvalidRepositoryFingerprint);
    }
    Ok(())
}

pub(in crate::state) fn default_project_browser_root() -> Result<PathBuf, StateError> {
    let home = env::var_os("HOME").ok_or(StateError::ProjectBrowserRootUnavailable)?;
    Ok(PathBuf::from(home))
}

pub(in crate::state) fn resolve_project_browser_root(value: &str) -> Result<PathBuf, StateError> {
    if value.is_empty()
        || value.len() > MAX_PROJECT_BROWSER_ROOT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(StateError::InvalidProjectBrowserRoot);
    }
    if value == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(StateError::ProjectBrowserRootUnavailable);
    }
    if let Some(relative) = value.strip_prefix("~/") {
        validate_project_browser_relative_path(relative)?;
        let home = env::var_os("HOME").ok_or(StateError::ProjectBrowserRootUnavailable)?;
        return Ok(PathBuf::from(home).join(relative));
    }
    let path = PathBuf::from(value);
    path.is_absolute()
        .then_some(path)
        .ok_or(StateError::InvalidProjectBrowserRoot)
}

pub(in crate::state) fn validate_project_browser_relative_path(
    value: &str,
) -> Result<(), StateError> {
    if value.len() > MAX_PROJECT_BROWSER_RELATIVE_PATH_BYTES
        || value.chars().any(char::is_control)
        || value.contains('\\')
        || value.starts_with('/')
        || value.ends_with('/')
    {
        return Err(StateError::InvalidProjectBrowserRelativePath);
    }
    if !value.is_empty()
        && Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(StateError::InvalidProjectBrowserRelativePath);
    }
    Ok(())
}

pub(in crate::state) fn project_browser_directory(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, StateError> {
    let current = fs::canonicalize(root.join(relative_path))
        .map_err(|_| StateError::ProjectBrowserRootUnavailable)?;
    if current.starts_with(root) && current.is_dir() {
        Ok(current)
    } else {
        Err(StateError::InvalidProjectBrowserRelativePath)
    }
}

pub(in crate::state) fn safe_project_browser_entry_name(name: &str, include_hidden: bool) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && (include_hidden || !name.starts_with('.'))
        && !name.chars().any(char::is_control)
        && !name.contains(['/', '\\'])
        && !matches!(name, "." | "..")
}

pub(in crate::state) fn project_browser_root_label(root: &Path) -> String {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project root")
            .to_owned();
    };
    if let Ok(relative) = root.strip_prefix(home) {
        if relative.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", relative.to_string_lossy())
        }
    } else {
        root.file_name().and_then(|name| name.to_str()).map_or_else(
            || "custom project root".to_owned(),
            |name| format!("custom root · {name}"),
        )
    }
}

#[cfg(unix)]
pub(in crate::state) fn set_private_directory_permissions(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
pub(in crate::state) fn set_private_directory_permissions(_path: &Path) -> Result<(), StateError> {
    Ok(())
}

#[cfg(all(unix, test))]
pub(in crate::state) fn set_private_file_permissions(path: &Path) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
pub(in crate::state) fn set_private_file_permissions(_path: &Path) -> Result<(), StateError> {
    Ok(())
}
