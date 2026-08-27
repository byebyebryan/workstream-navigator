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
    state::{StateError, StateRoot, open_d17_current_only},
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
}

/// Reads one passive schema-14 Workstreams projection. It neither opens a
/// browser nor resolves a repository path, even internally.
pub(crate) fn read_snapshot(root: &StateRoot) -> Result<D17Snapshot, D17SnapshotError> {
    let state = open_d17_current_only(root)?;
    let projects = state.d17_project_projections()?;
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
            Ok(D17WorkstreamSnapshot {
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
            })
        })
        .collect::<Result<Vec<_>, D17SnapshotError>>()?;

    Ok(D17Snapshot {
        projects,
        workstreams,
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, fs::OpenOptions};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::read_snapshot;
    use crate::{
        domain::{ProviderKind, RandomIdGenerator},
        state::{
            StateRoot, TRANSITION_LOCK_FILE, acquire_transition_lease, fresh_create,
            open_cutover_transition,
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
}
