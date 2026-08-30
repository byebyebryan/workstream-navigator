//! Project and Location projection and transaction helpers.
//!
//! This boundary owns the private promotion-time Project/Location graph and
//! the bounded display projection. It does not inspect repositories.

use super::{
    Connection, IdGenerator, LocationId, MAX_PROJECT_PROJECTION_LOCATIONS,
    MAX_PROJECT_PROJECTION_PROJECTS, OptionalExtension, ProjectId, Revision, StateError, params,
    validate_foreign_keys, validate_project_display_name, validate_remote_identity_display,
    validate_repository_fingerprint, validate_table_columns,
};

/// Persisted Project row introduced by schema 15.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRecord {
    pub project_id: ProjectId,
    pub label_location_id: LocationId,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub revision: Revision,
}

/// One bounded host-local Location row used by current snapshots.
/// Repository paths remain private; only the safe display, validated
/// fingerprint evidence, and separate origin display are projected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLocationProjection {
    pub project_id: ProjectId,
    pub location_id: LocationId,
    pub revision: Revision,
    pub is_label_source: bool,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub origin_display: Option<String>,
}

/// One deterministic Project row and its Location membership.  Both vectors
/// are bounded and sorted by their opaque IDs before being returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectProjection {
    pub project_id: ProjectId,
    pub revision: Revision,
    pub label_location_id: LocationId,
    pub display_name: String,
    pub repository_fingerprint: Option<String>,
    pub locations: Vec<ProjectLocationProjection>,
}
pub(super) fn validate_project_catalog(connection: &Connection) -> Result<(), StateError> {
    validate_table_columns(
        connection,
        "projects",
        &[
            "project_id",
            "label_location_id",
            "display_name",
            "repository_fingerprint",
            "revision",
        ],
    )?;
    validate_table_columns(
        connection,
        "project_locations",
        &[
            "location_id",
            "project_id",
            "repository_path",
            "repository_display_name",
            "remote_identity_fingerprint",
            "remote_identity_display",
            "revision",
        ],
    )?;
    validate_table_columns(
        connection,
        "opencode_settled_messages",
        &[
            "settled_message_id",
            "runtime_id",
            "runtime_generation",
            "native_session_id",
            "message_id",
        ],
    )?;
    let has_fingerprint_index: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'index' AND name = 'project_repository_fingerprint_idx'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if !has_fingerprint_index {
        return Err(StateError::MalformedHostSchema);
    }
    let null_locations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE project_id IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if null_locations != 0 {
        return Err(StateError::MalformedHostSchema);
    }
    validate_foreign_keys(connection)?;
    validate_project_location_rows(connection)?;
    validate_project_membership(connection)?;
    Ok(())
}

pub(super) fn validate_project_location_rows(connection: &Connection) -> Result<(), StateError> {
    let mut statement = connection
        .prepare(
            "SELECT location_id, project_id, repository_display_name,
                    remote_identity_display, revision
             FROM project_locations ORDER BY location_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let location_id: String = row.get(0).map_err(StateError::Sqlite)?;
        let project_id: Option<String> = row.get(1).map_err(StateError::Sqlite)?;
        let display_name: String = row.get(2).map_err(StateError::Sqlite)?;
        let origin_display: Option<String> = row.get(3).map_err(StateError::Sqlite)?;
        let revision: i64 = row.get(4).map_err(StateError::Sqlite)?;
        location_id
            .parse::<LocationId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        project_id
            .ok_or(StateError::MalformedHostSchema)?
            .parse::<ProjectId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_project_display_name(&display_name)
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_safe_origin_display(origin_display.as_deref())
            .map_err(|_| StateError::MalformedHostSchema)?;
        Revision::try_from(revision).map_err(|_| StateError::MalformedHostSchema)?;
    }
    Ok(())
}

pub(super) fn validate_project_membership(connection: &Connection) -> Result<(), StateError> {
    let mut statement = connection
        .prepare(
            "SELECT project_id, label_location_id, display_name,
                    repository_fingerprint, revision
             FROM projects ORDER BY project_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let project_id: String = row.get(0).map_err(StateError::Sqlite)?;
        let label_location_id: String = row.get(1).map_err(StateError::Sqlite)?;
        let display_name: String = row.get(2).map_err(StateError::Sqlite)?;
        let fingerprint: Option<String> = row.get(3).map_err(StateError::Sqlite)?;
        let revision: i64 = row.get(4).map_err(StateError::Sqlite)?;
        let project_id = project_id
            .parse::<ProjectId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        let label_location_id = label_location_id
            .parse::<LocationId>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_project_display_name(&display_name)
            .map_err(|_| StateError::MalformedHostSchema)?;
        validate_repository_fingerprint(fingerprint.as_deref())
            .map_err(|_| StateError::MalformedHostSchema)?;
        Revision::try_from(revision).map_err(|_| StateError::MalformedHostSchema)?;
        let membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
                [project_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if membership == 0 {
            return Err(StateError::MalformedHostSchema);
        }
        let source_membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM project_locations
                 WHERE project_id = ?1 AND location_id = ?2",
                params![project_id.to_string(), label_location_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if source_membership != 1 {
            return Err(StateError::MalformedHostSchema);
        }
        let (source_display_name, source_fingerprint): (String, Option<String>) = connection
            .query_row(
                "SELECT repository_display_name, remote_identity_fingerprint
                 FROM project_locations WHERE project_id = ?1 AND location_id = ?2",
                params![project_id.to_string(), label_location_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(StateError::Sqlite)?;
        if source_display_name != display_name
            || !project_source_fingerprints_compatible(
                fingerprint.as_deref(),
                source_fingerprint.as_deref(),
            )
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

pub(super) fn normalize_persisted_fingerprint(value: Option<&str>) -> Option<String> {
    let value = value.filter(|value| !value.is_empty())?;
    if validate_repository_fingerprint(Some(value)).is_ok() {
        Some(value.to_owned())
    } else {
        // An old or ambiguous origin is deliberately ungrouped rather than
        // guessed into a Project.  The raw bounded location field remains
        // untouched by migration.
        None
    }
}

pub(super) fn validate_safe_origin_display(value: Option<&str>) -> Result<(), StateError> {
    if value.is_some_and(str::is_empty) {
        return Ok(());
    }
    validate_remote_identity_display(value)
}

pub(super) fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    let project_id: String = row.get(0)?;
    let label_location_id: String = row.get(1)?;
    let revision: i64 = row.get(4)?;
    Ok(ProjectRecord {
        project_id: project_id.parse().map_err(to_sql_error)?,
        label_location_id: label_location_id.parse().map_err(to_sql_error)?,
        display_name: row.get(2)?,
        repository_fingerprint: row.get(3)?,
        revision: Revision::try_from(revision).map_err(domain_to_sql_error)?,
    })
}

pub(super) fn to_sql_error(error: uuid::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

pub(super) fn domain_to_sql_error(error: crate::domain::DomainError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error))
}

pub(super) fn load_project_projections(
    connection: &Connection,
) -> Result<Vec<ProjectProjection>, StateError> {
    let mut statement = connection
        .prepare(
            "SELECT project_id, label_location_id, display_name,
                    repository_fingerprint, revision
             FROM projects ORDER BY project_id",
        )
        .map_err(StateError::Sqlite)?;
    let projects = statement
        .query_map([], row_to_project)
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    if projects.len() > MAX_PROJECT_PROJECTION_PROJECTS {
        return Err(StateError::InvalidPersistedValue(
            "too many Project projection rows".to_owned(),
        ));
    }
    let mut total_locations = 0_usize;
    let mut projections = Vec::with_capacity(projects.len());
    for project in projects {
        let mut statement = connection
            .prepare(
                "SELECT location_id, repository_display_name,
                        remote_identity_fingerprint, remote_identity_display, revision
                 FROM project_locations WHERE project_id = ?1 ORDER BY location_id",
            )
            .map_err(StateError::Sqlite)?;
        let locations = statement
            .query_map([project.project_id.to_string()], |row| {
                let location_id: String = row.get(0)?;
                let display_name: String = row.get(1)?;
                let fingerprint: Option<String> = row.get(2)?;
                let origin_display: Option<String> = row.get(3)?;
                let revision: i64 = row.get(4)?;
                Ok((
                    location_id,
                    display_name,
                    fingerprint,
                    origin_display,
                    revision,
                ))
            })
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)?;
        if locations.is_empty() || locations.len() > MAX_PROJECT_PROJECTION_LOCATIONS {
            return Err(StateError::MalformedHostSchema);
        }
        total_locations = total_locations
            .checked_add(locations.len())
            .ok_or_else(|| {
                StateError::InvalidPersistedValue("Project projection size".to_owned())
            })?;
        if total_locations > MAX_PROJECT_PROJECTION_LOCATIONS {
            return Err(StateError::InvalidPersistedValue(
                "too many Project Location projection rows".to_owned(),
            ));
        }
        let mut projected_locations = Vec::with_capacity(locations.len());
        for (location_id, display_name, fingerprint, origin_display, revision) in locations {
            let location_id = location_id
                .parse::<LocationId>()
                .map_err(|_| StateError::MalformedHostSchema)?;
            validate_project_display_name(&display_name)?;
            validate_safe_origin_display(origin_display.as_deref())?;
            let revision = Revision::try_from(revision)?;
            projected_locations.push(ProjectLocationProjection {
                project_id: project.project_id,
                location_id,
                revision,
                is_label_source: location_id == project.label_location_id,
                display_name,
                repository_fingerprint: normalize_persisted_fingerprint(fingerprint.as_deref()),
                origin_display: origin_display.filter(|value| !value.is_empty()),
            });
        }
        projections.push(ProjectProjection {
            project_id: project.project_id,
            revision: project.revision,
            label_location_id: project.label_location_id,
            display_name: project.display_name,
            repository_fingerprint: project.repository_fingerprint,
            locations: projected_locations,
        });
    }
    Ok(projections)
}

pub(super) fn validate_project_source_transaction(
    transaction: &rusqlite::Transaction<'_>,
    project: &ProjectRecord,
) -> Result<(), StateError> {
    if project.display_name.trim().is_empty() {
        return Err(StateError::InvalidPersistedValue(
            "empty Project display name".to_owned(),
        ));
    }
    let membership: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM project_locations
             WHERE project_id = ?1 AND location_id = ?2",
            params![
                project.project_id.to_string(),
                project.label_location_id.to_string()
            ],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)?;
    if membership != 1 {
        return Err(StateError::MalformedHostSchema);
    }
    let (display_name, source_fingerprint): (String, Option<String>) = transaction
        .query_row(
            "SELECT repository_display_name, remote_identity_fingerprint
             FROM project_locations WHERE project_id = ?1 AND location_id = ?2",
            params![
                project.project_id.to_string(),
                project.label_location_id.to_string()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(StateError::Sqlite)?;
    if display_name != project.display_name
        || !project_source_fingerprints_compatible(
            project.repository_fingerprint.as_deref(),
            source_fingerprint.as_deref(),
        )
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

pub(super) fn project_source_fingerprints_compatible(
    project_fingerprint: Option<&str>,
    source_fingerprint: Option<&str>,
) -> bool {
    let project_fingerprint = project_fingerprint.filter(|value| !value.is_empty());
    let source_fingerprint = normalize_persisted_fingerprint(source_fingerprint);
    match (project_fingerprint, source_fingerprint) {
        // A missing later observation is allowed to retain the Project's last
        // positive fingerprint without clearing the durable association.
        (Some(_) | None, None) => true,
        (Some(project), Some(source)) => project == source,
        (None, Some(_)) => false,
    }
}

pub(super) fn validate_project_membership_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StateError> {
    let mut projects = transaction
        .prepare("SELECT project_id, label_location_id, display_name, repository_fingerprint, revision FROM projects")
        .map_err(StateError::Sqlite)?;
    let rows = projects
        .query_map([], row_to_project)
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    for project in rows {
        validate_project_source_transaction(transaction, &project)?;
        let members = project_member_count(transaction, project.project_id)?;
        if members == 0 {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

pub(super) fn find_project_by_fingerprint(
    transaction: &rusqlite::Transaction<'_>,
    fingerprint: &str,
) -> Result<Option<ProjectRecord>, StateError> {
    transaction
        .query_row(
            "SELECT project_id, label_location_id, display_name,
                    repository_fingerprint, revision
             FROM projects WHERE repository_fingerprint = ?1",
            [fingerprint],
            row_to_project,
        )
        .optional()
        .map_err(StateError::Sqlite)
}

pub(super) fn create_project(
    transaction: &rusqlite::Transaction<'_>,
    location_id: LocationId,
    display_name: &str,
    fingerprint: Option<&str>,
    id_generator: &dyn IdGenerator,
) -> Result<ProjectRecord, StateError> {
    let project_id = ProjectId::from(id_generator.uuid());
    transaction
        .execute(
            "INSERT INTO projects (
                project_id, label_location_id, display_name,
                repository_fingerprint, revision
             ) VALUES (?1, ?2, ?3, ?4, 1)",
            params![
                project_id.to_string(),
                location_id.to_string(),
                display_name,
                fingerprint,
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(ProjectRecord {
        project_id,
        label_location_id: location_id,
        display_name: display_name.to_owned(),
        repository_fingerprint: fingerprint.map(str::to_owned),
        revision: Revision::INITIAL,
    })
}

pub(super) fn bump_project_revision(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
) -> Result<(), StateError> {
    transaction
        .execute(
            "UPDATE projects SET revision = revision + 1 WHERE project_id = ?1",
            [project_id.to_string()],
        )
        .map_err(StateError::Sqlite)?;
    if transaction.changes() != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    Ok(())
}

pub(super) fn project_member_count(
    transaction: &rusqlite::Transaction<'_>,
    project_id: ProjectId,
) -> Result<i64, StateError> {
    transaction
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}
