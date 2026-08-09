use super::model::AppError;
use super::{HostRegistry, Path, StateError, StateRoot, WorkstreamId, actions};

pub(super) const fn runtime_status_label(status: crate::domain::RuntimeStatus) -> &'static str {
    match status {
        crate::domain::RuntimeStatus::Starting => "starting",
        crate::domain::RuntimeStatus::Idle | crate::domain::RuntimeStatus::Attention => "idle",
        crate::domain::RuntimeStatus::Working => "working",
        crate::domain::RuntimeStatus::Stopped => "parked",
        crate::domain::RuntimeStatus::Unknown => "unknown",
        crate::domain::RuntimeStatus::Unreachable => "unreachable",
    }
}

pub(super) fn acknowledge(
    registry: &mut HostRegistry,
    workstream_id: WorkstreamId,
    attention_revision: i64,
) -> Result<(), AppError> {
    let revision = crate::domain::Revision::try_from(attention_revision)
        .map_err(|_| AppError::InvalidAttentionRevision)?;
    registry.acknowledge_result_attention(workstream_id, revision)?;
    println!("acknowledged workstream {workstream_id}");
    Ok(())
}

pub(super) fn register(
    registry: &mut HostRegistry,
    checkout: &Path,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let repository = crate::repository::inspect(checkout)?;
    let capabilities = crate::provider::discover_capabilities(registry)?;
    let provider =
        crate::provider::select_registration_provider(&capabilities, requested_provider)?;
    crate::provider::require_new_eligible(registry, provider)?;
    let registered = registry.register_external_workstream_with_metadata(
        &repository.project_root,
        &repository.display_name,
        repository.remote_identity_fingerprint.as_deref(),
        repository.remote_identity_display.as_deref(),
        provider,
    )?;
    println!("registered workstream {}", registered.workstream_id);
    Ok(())
}

pub(super) fn new_workstream(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let request_key = uuid::Uuid::new_v4().to_string();
    let source_provider = registry
        .workstream_overviews()?
        .into_iter()
        .find(|overview| overview.workstream_id == source_workstream_id)
        .ok_or(StateError::UnknownOpenWorkstream(source_workstream_id))?
        .provider;
    let capabilities = crate::provider::discover_capabilities(registry)?;
    let provider =
        crate::provider::select_new_provider(&capabilities, requested_provider, source_provider)?;
    let workstream_id = actions::start_independent_workstream(
        root,
        registry,
        source_workstream_id,
        None,
        &request_key,
        provider,
    )?;
    println!("started independent workstream {workstream_id}");
    Ok(())
}

pub(super) fn fork_workstream(
    root: &StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
) -> Result<(), AppError> {
    let workstream_id = actions::fork_workstream(
        root,
        registry,
        source_workstream_id,
        None,
        uuid::Uuid::new_v4().to_string(),
    )?;
    println!("forked workstream {workstream_id}");
    Ok(())
}
