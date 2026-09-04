//! Bounded schema-15 Workstreams projection for the Navigator.
//!
//! This projection deliberately has no browser state or repository path. It is
//! a passive registry read: materialization, provider launch,
//! reconciliation, tmux, Git, and observer effects remain outside it.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::{
    domain::{
        LocationId, OperationId, OperationKind, OperationPhase, ProjectId, ProviderKind, Revision,
        RuntimeId, RuntimeStatus, WorkstreamId, WorkstreamLifecycle,
    },
    state::{StateError, StateRoot, current::OnboardingVisibility, open_current},
};

/// One display-safe project group. Its locations remain presentation data;
/// neither grants onboarding authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectSnapshot {
    pub(crate) project_id: ProjectId,
    pub(crate) display_name: String,
    pub(crate) locations: Vec<LocationSnapshot>,
}

/// One exact registered launch location, without its private filesystem path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocationSnapshot {
    pub(crate) location_id: LocationId,
    pub(crate) display_name: String,
    pub(crate) revision: Revision,
    pub(crate) is_label_source: bool,
}

/// One managed Workstream/card view. Native session identifiers, commands,
/// paths, process metadata, and provider content are intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkstreamSnapshot {
    pub(crate) project_id: ProjectId,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) provider: ProviderKind,
    pub(crate) lifecycle: WorkstreamLifecycle,
    pub(crate) archived: bool,
    /// Monotonic activity sequence used by both Navigator rows and provider
    /// cycling. The value is bounded registry metadata, not provider content.
    pub(crate) last_activity_sequence: i64,
    /// Wall-clock time of the most recent observed native conversation
    /// activity. `None` means no turn has been observed and no time is
    /// inferred.
    pub(crate) last_activity_at_millis: Option<i64>,
    pub(crate) revision: Revision,
    pub(crate) runtime: Option<RuntimeSnapshot>,
    pub(crate) onboarding: Option<OnboardingStatus>,
    pub(crate) native_name: Option<String>,
}

/// One unresolved non-onboarding creation operation retained for the public
/// operations diagnostic. Navigator rows do not project it as a recovery
/// target; request keys, effect details, project paths, and provider payloads
/// remain private to state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OperationSnapshot {
    pub(crate) operation_id: OperationId,
    pub(crate) kind: OperationKind,
    pub(crate) provider: ProviderKind,
    pub(crate) phase: OperationPhase,
    pub(crate) revision: Revision,
}

/// Bounded runtime status used only to select an exact existing Workstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeSnapshot {
    pub(crate) runtime_id: RuntimeId,
    pub(crate) status: RuntimeStatus,
    pub(crate) revision: Revision,
}

/// The bounded onboarding state rendered on a Runtime-owned card. A reserved
/// graph has no card yet; a proven native exec returns to ordinary Workstream
/// projection and therefore has no value here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OnboardingStatus {
    ActionFenced,
    RecoveryRequired,
}

/// Complete passive input to the Workstreams and Archived pages plus the
/// bounded unresolved-operation data used by the public `operations` command.
/// Navigator rows omit that diagnostic data. The provisional shell card is
/// derived by the Navigator, not persisted here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) projects: Vec<ProjectSnapshot>,
    pub(crate) workstreams: Vec<WorkstreamSnapshot>,
    /// Public diagnostic only; this is not a Navigator recovery-row source.
    pub(crate) unresolved_operations: Vec<OperationSnapshot>,
}

/// Bounded passive-snapshot failure. It never includes a private path,
/// provider payload, process detail, terminal capture, or registry text.
#[derive(Debug, Error)]
pub(crate) enum SnapshotError {
    #[error("Workstreams state is unavailable")]
    State(#[from] StateError),
    #[error("Workstreams state has inconsistent project membership")]
    ProjectMembership,
    #[error("Workstreams state has inconsistent provider identity")]
    ProviderIdentity,
    #[error("Workstreams state has inconsistent onboarding ownership")]
    OnboardingOwnership,
}

/// Reads one passive schema-15 Workstreams and operations projection. It
/// neither opens a browser nor resolves a repository path, even internally.
#[allow(
    clippy::too_many_lines,
    reason = "the one bounded projection keeps project, onboarding, and runtime cross-checks together"
)]
pub(crate) fn read_snapshot(root: &StateRoot) -> Result<Snapshot, SnapshotError> {
    let state = open_current(root)?;
    let projects = state.project_projections()?;
    let onboarding = state.onboarding_workstream_projections()?;
    let registry = state.into_host_registry()?;
    let workstreams = registry.workstream_overviews()?;
    let unresolved_operations = registry
        .unresolved_operation_overviews()?
        .into_iter()
        .map(|operation| OperationSnapshot {
            operation_id: operation.operation_id,
            kind: operation.kind,
            provider: operation.provider,
            phase: operation.phase,
            revision: operation.revision,
        })
        .collect();

    let mut project_for_location = BTreeMap::new();
    let projects = projects
        .into_iter()
        .map(|project| {
            let locations = project
                .locations
                .into_iter()
                .map(|location| {
                    project_for_location.insert(location.location_id, project.project_id);
                    LocationSnapshot {
                        location_id: location.location_id,
                        display_name: location.display_name,
                        revision: location.revision,
                        is_label_source: location.is_label_source,
                    }
                })
                .collect();
            ProjectSnapshot {
                project_id: project.project_id,
                display_name: project.display_name,
                locations,
            }
        })
        .collect();

    let mut onboarding = onboarding
        .into_iter()
        .map(|projection| (projection.workstream_id, projection))
        .collect::<BTreeMap<_, _>>();
    let workstreams = workstreams
        .into_iter()
        .map(|workstream| {
            let project_id = project_for_location
                .get(&workstream.location_id)
                .copied()
                .ok_or(SnapshotError::ProjectMembership)?;
            if workstream
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.provider != workstream.provider)
            {
                return Err(SnapshotError::ProviderIdentity);
            }
            let onboarding = onboarding.remove(&workstream.workstream_id);
            let onboarding = match onboarding {
                Some(projection) => {
                    if workstream
                        .runtime
                        .as_ref()
                        .map(|runtime| runtime.runtime_id)
                        != Some(projection.runtime_id)
                    {
                        return Err(SnapshotError::OnboardingOwnership);
                    }
                    match projection.visibility {
                        OnboardingVisibility::Reserved => return Ok(None),
                        OnboardingVisibility::ActionFenced => Some(OnboardingStatus::ActionFenced),
                        OnboardingVisibility::RecoveryRequired => {
                            Some(OnboardingStatus::RecoveryRequired)
                        }
                    }
                }
                None => None,
            };
            Ok(Some(WorkstreamSnapshot {
                project_id,
                location_id: workstream.location_id,
                workstream_id: workstream.workstream_id,
                provider: workstream.provider,
                lifecycle: workstream.lifecycle,
                archived: workstream.archived_at_millis.is_some(),
                last_activity_sequence: workstream.last_activity_sequence,
                last_activity_at_millis: workstream.last_activity_at_millis,
                revision: workstream.revision,
                runtime: workstream.runtime.map(|runtime| RuntimeSnapshot {
                    runtime_id: runtime.runtime_id,
                    status: runtime.status,
                    revision: runtime.revision,
                }),
                onboarding,
                native_name: workstream
                    .binding
                    .and_then(|binding| binding.observed_thread_name)
                    .filter(|name| !name.is_empty()),
            }))
        })
        .collect::<Result<Vec<Option<_>>, SnapshotError>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !onboarding.is_empty() {
        return Err(SnapshotError::OnboardingOwnership);
    }

    Ok(Snapshot {
        projects,
        workstreams,
        unresolved_operations,
    })
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs};

    use rusqlite::{Connection, params};
    use uuid::Uuid;

    use super::{OnboardingStatus, read_snapshot};
    use crate::{
        domain::{ProviderKind, RandomIdGenerator, Revision, RuntimeId, WorkstreamLifecycle},
        navigator::{ManagedAction, apply_managed_action},
        onboarding::{ShellCommandDecision, classify_shell_command},
        presentation::{ProvisionalInventory, ProvisionalInventoryError},
        provisional::{HostInventoryError, classify_host_inventory},
        repository::RepositoryDiscovery,
        runtime::RuntimePaths,
        state::{
            StateRoot, create_current,
            current::{OnboardingPreparation, OnboardingPrepareRequest},
            open_current,
        },
    };

    #[test]
    fn current_snapshot_groups_retained_workstreams_without_browser_state() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let mut state = create_current(&state_path, &RandomIdGenerator).unwrap();
        let (_, workstream_id) = state
            .seed_test_workstream(
                &checkout,
                "checkout",
                ProviderKind::OpenCode,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);

        let root = StateRoot::select(&state_path);

        let snapshot = read_snapshot(&root).unwrap();
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].display_name, "checkout");
        assert_eq!(snapshot.projects[0].locations.len(), 1);
        assert_eq!(snapshot.workstreams.len(), 1);
        assert_eq!(snapshot.workstreams[0].workstream_id, workstream_id);
        assert_eq!(snapshot.workstreams[0].provider, ProviderKind::OpenCode);
        assert_eq!(snapshot.workstreams[0].lifecycle, WorkstreamLifecycle::Open);
        assert!(!snapshot.workstreams[0].archived);
        assert!(snapshot.workstreams[0].runtime.is_none());
        assert!(snapshot.unresolved_operations.is_empty());
        assert_eq!(snapshot.workstreams[0].last_activity_at_millis, None);

        let connection = Connection::open(state_path.join("host.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE workstreams SET last_activity_at_millis = ?1
                 WHERE workstream_id = ?2",
                params![1_700_000_000_000_i64, workstream_id.to_string()],
            )
            .unwrap();
        drop(connection);
        let snapshot = read_snapshot(&root).unwrap();
        assert_eq!(
            snapshot.workstreams[0].last_activity_at_millis,
            Some(1_700_000_000_000)
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the fixture proves the complete reserved-to-owned passive snapshot boundary"
    )]
    fn current_snapshot_hides_reserved_onboarding_then_fences_runtime_owned_card() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        drop(create_current(&state_path, &RandomIdGenerator).unwrap());

        let root = StateRoot::select(&state_path);

        let candidate_runtime_id = RuntimeId::from(Uuid::from_u128(2));
        let arguments = [OsString::from("--model"), OsString::from("gpt-5.6")];
        let ShellCommandDecision::ManagedFresh(launch) =
            classify_shell_command(ProviderKind::Codex, &arguments).unwrap()
        else {
            panic!("fixture must use a promotable Codex launch");
        };
        let request = OnboardingPrepareRequest {
            request_key: "snapshot-onboarding".to_owned(),
            presentation_id: Uuid::from_u128(3),
            presentation_revision: Revision::INITIAL,
            slot_generation: Uuid::from_u128(4),
            candidate_runtime_id,
            runtime_paths: RuntimePaths::for_runtime(&state_path, candidate_runtime_id),
            provider: ProviderKind::Codex,
            repository: RepositoryDiscovery {
                project_root: checkout.clone(),
                display_name: "checkout".to_owned(),
                remote_identity_fingerprint: None,
                remote_identity_display: None,
            },
            shell_cwd: checkout,
            shell_pid: 5,
            shell_birth: "birth-5".to_owned(),
            shell_process_group: 5,
            shell_session: 5,
            argv_digest: launch.argv_digest().to_owned(),
            boot_provenance: format!("wsnav-boot-v1:sha256:{}", "a".repeat(64)),
            now_monotonic_millis: 10,
            expiry_monotonic_millis: 1_010,
        };
        let mut state = open_current(&root).unwrap();
        let provisional = state.acquire_provisional_lease().unwrap();
        assert_eq!(
            classify_host_inventory(&state, &provisional).unwrap(),
            ProvisionalInventory::Vacant
        );
        let issued = match state
            .prepare_onboarding_current(&provisional, &request, &RandomIdGenerator)
            .unwrap()
        {
            OnboardingPreparation::Issued(issued) => issued,
            OnboardingPreparation::Existing(_) => panic!("first request must issue"),
        };
        assert!(matches!(
            classify_host_inventory(&state, &provisional),
            Err(HostInventoryError::Inventory(
                ProvisionalInventoryError::Ambiguous
            ))
        ));
        assert!(read_snapshot(&root).unwrap().workstreams.is_empty());

        let token = issued.capability().token().to_owned();
        let ownership = state
            .consume_onboarding_current(
                &provisional,
                &request,
                &token,
                request.now_monotonic_millis + 1,
            )
            .unwrap();
        assert_eq!(
            state.registered_runtime_paths().unwrap(),
            vec![request.runtime_paths.clone()]
        );
        let snapshot = read_snapshot(&root).unwrap();
        assert_eq!(snapshot.workstreams.len(), 1);
        assert_eq!(
            snapshot.workstreams[0].runtime.unwrap().runtime_id,
            candidate_runtime_id
        );
        assert_eq!(
            snapshot.workstreams[0].onboarding,
            Some(OnboardingStatus::ActionFenced)
        );

        state
            .record_recovery_required_current(&provisional, &request, ownership)
            .unwrap();
        assert_eq!(
            read_snapshot(&root).unwrap().workstreams[0].onboarding,
            Some(OnboardingStatus::RecoveryRequired)
        );

        drop(provisional);
        drop(state);

        let before_archive = read_snapshot(&root).unwrap();
        let workstream_id = before_archive.workstreams[0].workstream_id;
        let expected_revision = before_archive.workstreams[0].revision;
        assert_eq!(
            apply_managed_action(
                &root,
                ManagedAction::Archive {
                    workstream_id,
                    expected_workstream_revision: expected_revision,
                },
            )
            .unwrap(),
            None
        );

        let archived = read_snapshot(&root).unwrap();
        assert!(archived.workstreams[0].archived);
        assert_eq!(
            archived.workstreams[0].lifecycle,
            crate::domain::WorkstreamLifecycle::Parked
        );
        assert_eq!(
            archived.workstreams[0].onboarding, None,
            "Archive must resolve terminal onboarding recovery before hiding the row"
        );
        assert_eq!(
            archived.workstreams[0].runtime.as_ref().unwrap().status,
            crate::domain::RuntimeStatus::Stopped
        );
        let connection = Connection::open(root.host_database_path()).unwrap();
        let (phase, outcome): (String, Option<String>) = connection
            .query_row(
                "SELECT phase, outcome_json FROM compound_operations WHERE kind = 'onboard'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(phase, "committed");
        assert_eq!(
            outcome.as_deref(),
            Some(r#"{"code":"parked_recovery_resolved_v1"}"#)
        );
        drop(connection);

        let archived_revision = archived.workstreams[0].revision;
        assert_eq!(
            apply_managed_action(
                &root,
                ManagedAction::Restore {
                    workstream_id: archived.workstreams[0].workstream_id,
                    expected_workstream_revision: archived_revision,
                },
            )
            .unwrap(),
            None
        );
        let resolved = read_snapshot(&root).unwrap();
        assert!(!resolved.workstreams[0].archived);
        assert_eq!(
            resolved.workstreams[0].lifecycle,
            crate::domain::WorkstreamLifecycle::Open
        );
        assert_eq!(resolved.workstreams[0].onboarding, None);
        assert_eq!(
            resolved.workstreams[0].runtime.as_ref().unwrap().status,
            crate::domain::RuntimeStatus::Stopped
        );

        // The exact terminal journal outcome remains valid after ordinary
        // Resume reuses the retained Runtime. Requiring this terminal outcome
        // to keep the row hidden would incorrectly re-fence this Workstream on
        // the next refresh.
        let state = open_current(&root).unwrap();
        let mut registry = state.into_host_registry().unwrap();
        registry
            .reserve_runtime_with_provider(
                resolved.workstreams[0].workstream_id,
                crate::domain::ProviderKind::Codex,
            )
            .unwrap();
        drop(registry);
        let resumed = read_snapshot(&root).unwrap();
        assert_eq!(
            resumed.workstreams[0].lifecycle,
            crate::domain::WorkstreamLifecycle::Open
        );
        assert_eq!(resumed.workstreams[0].onboarding, None);
    }
}
