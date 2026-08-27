//! Bounded schema-14 Workstreams projection for the D17 Navigator.
//!
//! This projection deliberately has no Project-browser state or repository
//! path. It is a passive registry read: materialization, provider launch,
//! reconciliation, tmux, Git, and observer effects remain outside it.

#![allow(
    dead_code,
    reason = "the D17 Workstreams navigator remains unreachable until the atomic cutover"
)]

use std::collections::BTreeMap;

use thiserror::Error;

use crate::{
    domain::{
        LocationId, ProjectId, ProviderKind, Revision, RuntimeId, RuntimeStatus, WorkstreamId,
    },
    state::{StateError, StateRoot, d16::D17OnboardingVisibility, open_d17_current_only},
};

/// One display-safe D17 project group. Its locations remain presentation data;
/// neither grants onboarding authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17ProjectSnapshot {
    pub(crate) project_id: ProjectId,
    pub(crate) display_name: String,
    pub(crate) locations: Vec<D17LocationSnapshot>,
}

/// One exact registered launch location, without its private filesystem path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17LocationSnapshot {
    pub(crate) location_id: LocationId,
    pub(crate) display_name: String,
    pub(crate) revision: Revision,
    pub(crate) is_label_source: bool,
}

/// One managed Workstream/card view. Native session identifiers, commands,
/// paths, process metadata, and provider content are intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17WorkstreamSnapshot {
    pub(crate) project_id: ProjectId,
    pub(crate) location_id: LocationId,
    pub(crate) workstream_id: WorkstreamId,
    pub(crate) provider: ProviderKind,
    pub(crate) archived: bool,
    pub(crate) revision: Revision,
    pub(crate) runtime: Option<D17RuntimeSnapshot>,
    pub(crate) onboarding: Option<D17OnboardingStatus>,
    pub(crate) native_name: Option<String>,
    pub(crate) result_unseen: bool,
    pub(crate) recovery_unseen: bool,
}

/// Bounded runtime status used only to select an exact existing Workstream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct D17RuntimeSnapshot {
    pub(crate) runtime_id: RuntimeId,
    pub(crate) status: RuntimeStatus,
    pub(crate) revision: Revision,
}

/// The bounded onboarding state rendered on a Runtime-owned card. A reserved
/// graph has no card yet; a proven native exec returns to ordinary Workstream
/// projection and therefore has no value here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum D17OnboardingStatus {
    ActionFenced,
    RecoveryRequired,
}

/// Complete passive input to the D17 Workstreams and Archived pages. The
/// provisional shell card is derived by the Navigator, not persisted here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct D17Snapshot {
    pub(crate) projects: Vec<D17ProjectSnapshot>,
    pub(crate) workstreams: Vec<D17WorkstreamSnapshot>,
}

/// Bounded passive-snapshot failure. It never includes a private path,
/// provider payload, process detail, terminal capture, or registry text.
#[derive(Debug, Error)]
pub(crate) enum D17SnapshotError {
    #[error("D17 Workstreams state is unavailable")]
    State(#[from] StateError),
    #[error("D17 Workstreams state has inconsistent project membership")]
    ProjectMembership,
    #[error("D17 Workstreams state has inconsistent provider identity")]
    ProviderIdentity,
    #[error("D17 Workstreams state has inconsistent onboarding ownership")]
    OnboardingOwnership,
}

/// Reads one passive schema-14 Workstreams projection. It neither opens a
/// browser nor resolves a repository path, even internally.
#[allow(
    clippy::too_many_lines,
    reason = "the one bounded projection keeps project, onboarding, and runtime cross-checks together"
)]
pub(crate) fn read_snapshot(root: &StateRoot) -> Result<D17Snapshot, D17SnapshotError> {
    let state = open_d17_current_only(root)?;
    let projects = state.d17_project_projections()?;
    let onboarding = state.d17_onboarding_workstream_projections()?;
    let registry = state.into_d17_host_registry()?;
    let workstreams = registry.workstream_overviews()?;

    let mut project_for_location = BTreeMap::new();
    let projects = projects
        .into_iter()
        .map(|project| {
            let locations = project
                .locations
                .into_iter()
                .map(|location| {
                    project_for_location.insert(location.location_id, project.project_id);
                    D17LocationSnapshot {
                        location_id: location.location_id,
                        display_name: location.display_name,
                        revision: location.revision,
                        is_label_source: location.is_label_source,
                    }
                })
                .collect();
            D17ProjectSnapshot {
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
                .ok_or(D17SnapshotError::ProjectMembership)?;
            if workstream
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.provider != workstream.provider)
            {
                return Err(D17SnapshotError::ProviderIdentity);
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
                        return Err(D17SnapshotError::OnboardingOwnership);
                    }
                    match projection.visibility {
                        D17OnboardingVisibility::Reserved => return Ok(None),
                        D17OnboardingVisibility::ActionFenced => {
                            Some(D17OnboardingStatus::ActionFenced)
                        }
                        D17OnboardingVisibility::RecoveryRequired => {
                            Some(D17OnboardingStatus::RecoveryRequired)
                        }
                    }
                }
                None => None,
            };
            Ok(Some(D17WorkstreamSnapshot {
                project_id,
                location_id: workstream.location_id,
                workstream_id: workstream.workstream_id,
                provider: workstream.provider,
                archived: workstream.archived_at_millis.is_some(),
                revision: workstream.revision,
                runtime: workstream.runtime.map(|runtime| D17RuntimeSnapshot {
                    runtime_id: runtime.runtime_id,
                    status: runtime.status,
                    revision: runtime.revision,
                }),
                onboarding,
                native_name: workstream
                    .binding
                    .and_then(|binding| binding.observed_thread_name)
                    .filter(|name| !name.is_empty()),
                result_unseen: workstream
                    .attention
                    .as_ref()
                    .is_some_and(|attention| attention.result_unseen_since_revision.is_some()),
                recovery_unseen: workstream
                    .attention
                    .as_ref()
                    .is_some_and(|attention| attention.recovery_unseen_since_revision.is_some()),
            }))
        })
        .collect::<Result<Vec<Option<_>>, D17SnapshotError>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !onboarding.is_empty() {
        return Err(D17SnapshotError::OnboardingOwnership);
    }

    Ok(D17Snapshot {
        projects,
        workstreams,
    })
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, fs::OpenOptions};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use uuid::Uuid;

    use super::{D17OnboardingStatus, read_snapshot};
    use crate::{
        domain::{ProviderKind, RandomIdGenerator, Revision, RuntimeId},
        onboarding::{ShellCommandDecision, classify_shell_command},
        presentation::{D17ProvisionalInventory, D17ProvisionalInventoryError},
        provisional::{HostInventoryError, classify_host_inventory},
        repository::RepositoryRegistration,
        runtime::RuntimePaths,
        state::{
            StateRoot, TRANSITION_LOCK_FILE, acquire_transition_lease,
            d16::{OnboardingPreparation, OnboardingPrepareRequest},
            fresh_create, open_cutover_transition, open_d17_current_only,
        },
    };

    #[test]
    fn schema14_snapshot_groups_retained_workstreams_without_browser_state() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let mut state = fresh_create(&state_path, &RandomIdGenerator).unwrap();
        let registered = state
            .register_project_location_with_initial_workstream(
                &checkout,
                "checkout",
                None,
                None,
                ProviderKind::OpenCode,
                &RandomIdGenerator,
            )
            .unwrap();
        drop(state);

        let root = StateRoot::select(&state_path);
        let transition_lock = state_path.join(TRANSITION_LOCK_FILE);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&transition_lock)
            .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&transition_lock, fs::Permissions::from_mode(0o600)).unwrap();
        let lease = acquire_transition_lease(&state_path).unwrap();
        let mut state = open_cutover_transition(&root, &lease).unwrap();
        state.migrate_schema13_to14(&lease).unwrap();
        drop(state);
        drop(lease);
        fs::remove_file(state_path.join(TRANSITION_LOCK_FILE)).unwrap();

        let snapshot = read_snapshot(&root).unwrap();
        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].display_name, "checkout");
        assert_eq!(snapshot.projects[0].locations.len(), 1);
        assert_eq!(snapshot.workstreams.len(), 1);
        assert_eq!(
            snapshot.workstreams[0].workstream_id,
            registered.workstream.workstream_id
        );
        assert_eq!(snapshot.workstreams[0].provider, ProviderKind::OpenCode);
        assert!(!snapshot.workstreams[0].archived);
        assert!(snapshot.workstreams[0].runtime.is_none());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the fixture proves the complete reserved-to-owned passive snapshot boundary"
    )]
    fn schema14_snapshot_hides_reserved_onboarding_then_fences_runtime_owned_card() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let checkout = temporary.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        drop(fresh_create(&state_path, &RandomIdGenerator).unwrap());

        let root = StateRoot::select(&state_path);
        let transition_lock = state_path.join(TRANSITION_LOCK_FILE);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&transition_lock)
            .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&transition_lock, fs::Permissions::from_mode(0o600)).unwrap();
        let transition = acquire_transition_lease(&state_path).unwrap();
        let mut migrating = open_cutover_transition(&root, &transition).unwrap();
        migrating.migrate_schema13_to14(&transition).unwrap();
        drop(migrating);
        drop(transition);
        fs::remove_file(&transition_lock).unwrap();

        let candidate_runtime_id = RuntimeId::from(Uuid::from_u128(2));
        let arguments = [OsString::from("--model"), OsString::from("gpt-5.6")];
        let ShellCommandDecision::ManagedFresh(launch) =
            classify_shell_command(ProviderKind::Codex, &arguments).unwrap()
        else {
            panic!("fixture must use a promotable Codex launch");
        };
        let request = OnboardingPrepareRequest {
            request_key: "d17-snapshot-onboarding".to_owned(),
            presentation_id: Uuid::from_u128(3),
            presentation_revision: Revision::INITIAL,
            slot_generation: Uuid::from_u128(4),
            candidate_runtime_id,
            runtime_paths: RuntimePaths::for_runtime(&state_path, candidate_runtime_id),
            provider: ProviderKind::Codex,
            repository: RepositoryRegistration {
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
            boot_provenance: format!("d17-boot-v1:sha256:{}", "a".repeat(64)),
            now_monotonic_millis: 10,
            expiry_monotonic_millis: 1_010,
        };
        let mut state = open_d17_current_only(&root).unwrap();
        let provisional = state.acquire_d17_provisional_lease().unwrap();
        assert_eq!(
            classify_host_inventory(&state, &provisional).unwrap(),
            D17ProvisionalInventory::Vacant
        );
        let issued = match state
            .prepare_d17_onboarding_current(&provisional, &request, &RandomIdGenerator)
            .unwrap()
        {
            OnboardingPreparation::Issued(issued) => issued,
            OnboardingPreparation::Existing(_) => panic!("first request must issue"),
        };
        assert!(matches!(
            classify_host_inventory(&state, &provisional),
            Err(HostInventoryError::Inventory(
                D17ProvisionalInventoryError::Ambiguous
            ))
        ));
        assert!(read_snapshot(&root).unwrap().workstreams.is_empty());

        let token = issued.capability().token().to_owned();
        let ownership = state
            .consume_d17_onboarding_current(
                &provisional,
                &request,
                &token,
                request.now_monotonic_millis + 1,
            )
            .unwrap();
        assert_eq!(
            state.d17_registered_runtime_paths().unwrap(),
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
            Some(D17OnboardingStatus::ActionFenced)
        );

        state
            .record_d17_recovery_required_current(&provisional, &request, ownership)
            .unwrap();
        assert_eq!(
            read_snapshot(&root).unwrap().workstreams[0].onboarding,
            Some(D17OnboardingStatus::RecoveryRequired)
        );
    }
}
