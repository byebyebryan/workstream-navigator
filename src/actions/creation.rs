use super::model::active_workstream_overview;
use super::{ActionError, HostRegistry, ProviderKind, Revision, StartOutcome, WorkstreamId, start};

#[derive(Clone, Copy)]
pub(super) struct IndependentStartSpec<'a> {
    pub(super) source_workstream_id: WorkstreamId,
    pub(super) expected_revision: Option<Revision>,
    pub(super) request_key: &'a str,
    pub(super) provider: ProviderKind,
}

pub(super) fn start_independent_workstream_with<R, S>(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    spec: IndependentStartSpec<'_>,
    readiness: R,
    starter: S,
) -> Result<WorkstreamId, ActionError>
where
    R: FnOnce(&HostRegistry, ProviderKind) -> Result<(), ActionError>,
    S: FnOnce(
        &crate::state::StateRoot,
        &mut HostRegistry,
        WorkstreamId,
        Option<Revision>,
        ProviderKind,
    ) -> Result<StartOutcome, ActionError>,
{
    let source = active_workstream_overview(registry, spec.source_workstream_id)?;
    if spec
        .expected_revision
        .is_some_and(|expected| expected != source.revision)
    {
        return Err(ActionError::WorkstreamRevisionConflict);
    }
    readiness(registry, spec.provider)?;
    let created = registry.create_independent_workstream(
        spec.request_key,
        spec.source_workstream_id,
        source.revision,
        spec.provider,
    )?;
    let _ = starter(
        root,
        registry,
        created.workstream_id,
        Some(created.revision),
        spec.provider,
    )?;
    Ok(created.workstream_id)
}

/// Creates an independent Workstream at a registered project's root, then
/// starts its first native provider Runtime. The source must remain in the
/// active catalog; archive changes Navigator visibility only and does not
/// revoke its project.
///
/// The source selects a `ProjectLocation` and expected revision only. This
/// action never invokes Git or copies files; the native provider owns any
/// worktree workflow it chooses after the native session starts.
///
/// # Errors
///
/// Returns an error when the source revision is stale or observer readiness
/// prevents the native start.
pub fn start_independent_workstream(
    root: &crate::state::StateRoot,
    registry: &mut HostRegistry,
    source_workstream_id: WorkstreamId,
    expected_revision: Option<Revision>,
    request_key: &str,
    provider: ProviderKind,
) -> Result<WorkstreamId, ActionError> {
    start_independent_workstream_with(
        root,
        registry,
        IndependentStartSpec {
            source_workstream_id,
            expected_revision,
            request_key,
            provider,
        },
        |registry, provider| {
            crate::provider::require_new_eligible(registry, provider)
                .map_err(ActionError::ProviderReadiness)
        },
        |root, registry, workstream_id, expected_revision, _provider| {
            start(root, registry, workstream_id, expected_revision)
        },
    )
}
