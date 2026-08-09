use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::domain::{HostId, LocationId, ProjectId, Revision};
use crate::protocol::{Capabilities, HelloResponse};

use super::models::{
    ClientHost, ClientHostTransport, ClientProjectLocation, HostIdentity, StateError, StateRoot,
};
use super::schema::{configure_connection, migrate_client_schema};
use super::utils::{
    set_private_file_permissions, to_from_sql_error, validate_client_host_alias,
    validate_client_host_text, validate_project_display_name, validate_repository_fingerprint,
};

#[derive(Debug)]
pub struct ClientCatalog {
    pub(in crate::state) connection: Connection,
}

impl ClientCatalog {
    /// Opens the client catalog, applying only known development migrations.
    ///
    /// # Errors
    ///
    /// Returns an error for I/O, `SQLite`, permission, or unsupported-schema
    /// failures.
    pub fn open(root: &StateRoot) -> Result<Self, StateError> {
        let path = root.client_database_path();
        let mut connection = Connection::open(&path).map_err(StateError::Sqlite)?;
        set_private_file_permissions(&path)?;
        configure_connection(&connection)?;
        migrate_client_schema(&mut connection)?;
        Ok(Self { connection })
    }

    /// Returns the client schema version recorded by `SQLite`.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read the schema version.
    pub fn schema_version(&self) -> Result<i64, StateError> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(StateError::Sqlite)
    }

    /// Creates the local client-side Project grouping for a newly registered
    /// host location. The generated Project identity is never inferred from a
    /// path; the supplied label is only initial presentation text.
    ///
    /// # Errors
    ///
    /// Returns an error when the local host identity changes unexpectedly, a
    /// display label is unsafe, or the client catalog cannot commit atomically.
    pub fn register_local_project_location(
        &mut self,
        host: &HostIdentity,
        location_id: LocationId,
        executable_path: &Path,
        display_name: &str,
    ) -> Result<ClientProjectLocation, StateError> {
        self.register_local_project_location_with_identity(
            host,
            location_id,
            executable_path,
            display_name,
            None,
        )
    }

    /// Associates one local host location with a presentation Project,
    /// reusing an existing Project when the repository fingerprint matches.
    ///
    /// # Errors
    ///
    /// Returns an error when local host trust changed, metadata is unsafe, or
    /// the client catalog cannot commit atomically.
    pub fn register_local_project_location_with_identity(
        &mut self,
        host: &HostIdentity,
        location_id: LocationId,
        executable_path: &Path,
        display_name: &str,
        repository_fingerprint: Option<&str>,
    ) -> Result<ClientProjectLocation, StateError> {
        validate_project_display_name(display_name)?;
        validate_repository_fingerprint(repository_fingerprint)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        ensure_local_client_host(&transaction, host, executable_path)?;
        let project = associate_project_location(
            &transaction,
            host.host_id,
            location_id,
            display_name,
            repository_fingerprint,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(project)
    }

    /// Associates a location on an already trusted host with a presentation
    /// Project. This changes only the local client catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is unknown, metadata is unsafe, or the
    /// client catalog cannot commit atomically.
    pub fn register_host_project_location(
        &mut self,
        host_id: HostId,
        location_id: LocationId,
        display_name: &str,
        repository_fingerprint: Option<&str>,
    ) -> Result<ClientProjectLocation, StateError> {
        validate_project_display_name(display_name)?;
        validate_repository_fingerprint(repository_fingerprint)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let known: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM hosts WHERE host_id = ?1)",
                [host_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)?;
        if !known {
            return Err(StateError::UnknownClientHost);
        }
        let project = associate_project_location(
            &transaction,
            host_id,
            location_id,
            display_name,
            repository_fingerprint,
        )?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(project)
    }

    /// Records one explicit SSH host registration after a successful bounded
    /// protocol handshake. A changed identity, generation, executable, or
    /// capability fingerprint is rejected until the user explicitly resets
    /// the registration.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe client record, a conflicting existing
    /// registration, or a failed atomic catalog update.
    pub fn register_ssh_host(
        &mut self,
        alias: &str,
        identity: &HostIdentity,
        executable_path: &Path,
        destination: &str,
        capabilities: Capabilities,
    ) -> Result<ClientHost, StateError> {
        validate_client_host_alias(alias)?;
        if alias == "local" {
            return Err(StateError::ClientHostRegistrationMismatch);
        }
        validate_client_host_text("remote executable", &executable_path.to_string_lossy())?;
        validate_client_host_text("SSH destination", destination)?;
        validate_client_host_text("registry generation", &identity.registry_generation)?;
        let capabilities_json = serialize_capabilities(&capabilities)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let existing = load_client_host_by_alias(&transaction, alias)?;
        let host = ClientHost {
            alias: alias.to_owned(),
            host_id: identity.host_id,
            registry_generation: identity.registry_generation.clone(),
            executable_path: executable_path.to_path_buf(),
            transport: ClientHostTransport::Ssh {
                destination: destination.to_owned(),
            },
            capabilities,
            revision: Revision::INITIAL,
        };
        if let Some(existing) = existing {
            validate_unchanged_ssh_registration(&existing, &host)?;
            transaction.commit().map_err(StateError::Sqlite)?;
            return Ok(existing);
        }
        let duplicate_alias: Option<String> = transaction
            .query_row(
                "SELECT host_alias FROM hosts WHERE host_id = ?1",
                [host.host_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StateError::Sqlite)?;
        if duplicate_alias.is_some() {
            return Err(StateError::ClientHostAlreadyRegistered);
        }
        transaction
            .execute(
                "INSERT INTO hosts (
                    host_alias, host_id, registry_generation, executable_path,
                    transport, ssh_destination, capabilities_json, revision
                 ) VALUES (?1, ?2, ?3, ?4, 'ssh', ?5, ?6, 1)",
                params![
                    host.alias,
                    host.host_id.to_string(),
                    host.registry_generation,
                    host.executable_path.to_string_lossy(),
                    destination,
                    capabilities_json,
                ],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(host)
    }

    /// Returns the exact client-side registration for one host alias.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog cannot be queried or contains an
    /// invalid persisted host record.
    pub fn host(&self, alias: &str) -> Result<Option<ClientHost>, StateError> {
        self.connection
            .query_row(
                "SELECT host_alias, host_id, registry_generation, executable_path,
                        transport, ssh_destination, capabilities_json, revision
                 FROM hosts WHERE host_alias = ?1",
                [alias],
                row_to_client_host,
            )
            .optional()
            .map_err(StateError::Sqlite)
    }

    /// Returns every explicitly registered SSH host in deterministic alias
    /// order. Local host bookkeeping is deliberately excluded.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog cannot be queried or contains an
    /// invalid persisted host record.
    pub fn ssh_hosts(&self) -> Result<Vec<ClientHost>, StateError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT host_alias, host_id, registry_generation, executable_path,
                        transport, ssh_destination, capabilities_json, revision
                 FROM hosts WHERE transport = 'ssh' ORDER BY host_alias",
            )
            .map_err(StateError::Sqlite)?;
        statement
            .query_map([], row_to_client_host)
            .map_err(StateError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StateError::Sqlite)
    }

    /// Verifies fresh `hello` evidence against the user's fixed registration.
    /// A mismatch does not update the catalog and callers must disable remote
    /// mutation until the operator resets and re-registers the host.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is unknown or its identity, generation,
    /// or capabilities differ from the recorded registration.
    pub fn verify_hello(
        &self,
        alias: &str,
        hello: &HelloResponse,
    ) -> Result<ClientHost, StateError> {
        let host = self.host(alias)?.ok_or(StateError::UnknownClientHost)?;
        host.verify_hello(hello)?;
        Ok(host)
    }

    /// Removes one explicit SSH host registration and its client-side project
    /// associations. It never contacts the host or mutates the host registry.
    ///
    /// # Errors
    ///
    /// Returns an error for the protected local record, an unknown alias, or a
    /// failed atomic catalog update.
    pub fn reset_ssh_host(&mut self, alias: &str) -> Result<(), StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let host =
            load_client_host_by_alias(&transaction, alias)?.ok_or(StateError::UnknownClientHost)?;
        if !matches!(host.transport, ClientHostTransport::Ssh { .. }) {
            return Err(StateError::ClientHostResetRefused);
        }
        transaction
            .execute(
                "DELETE FROM project_locations WHERE host_id = ?1",
                [host.host_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        transaction
            .execute(
                "DELETE FROM ignored_project_locations WHERE host_id = ?1",
                [host.host_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        let deleted = transaction
            .execute("DELETE FROM hosts WHERE host_alias = ?1", [alias])
            .map_err(StateError::Sqlite)?;
        if deleted != 1 {
            return Err(StateError::ConcurrentWrite);
        }
        transaction.commit().map_err(StateError::Sqlite)
    }

    /// Looks up the client-local Project label for one exact local host
    /// location. Missing data is a normal fallback condition during D2.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog cannot be queried or contains an
    /// invalid persisted identity.
    pub fn local_project_location(
        &self,
        host_id: HostId,
        location_id: LocationId,
    ) -> Result<Option<ClientProjectLocation>, StateError> {
        self.connection
            .query_row(
                "SELECT projects.project_id, projects.display_name,
                        projects.repository_fingerprint
                 FROM project_locations
                 JOIN projects ON projects.project_id = project_locations.project_id
                 WHERE project_locations.host_id = ?1 AND project_locations.location_id = ?2",
                params![host_id.to_string(), location_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StateError::Sqlite)
            .and_then(|row| {
                row.map_or(Ok(None), |(project_id, display_name, fingerprint)| {
                    Uuid::parse_str(&project_id)
                        .map(ProjectId::from)
                        .map(|project_id| {
                            Some(ClientProjectLocation {
                                project_id,
                                display_name,
                                repository_fingerprint: fingerprint,
                            })
                        })
                        .map_err(StateError::InvalidPersistedUuid)
                })
            })
    }

    /// Returns whether this host-owned location was explicitly forgotten from
    /// the client navigator without mutating its host registry.
    ///
    /// # Errors
    ///
    /// Returns an error when the client catalog cannot be queried.
    pub fn project_location_is_ignored(
        &self,
        host_id: HostId,
        location_id: LocationId,
    ) -> Result<bool, StateError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM ignored_project_locations
                    WHERE host_id = ?1 AND location_id = ?2
                 )",
                params![host_id.to_string(), location_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StateError::Sqlite)
    }

    /// Hides every client-visible location in one Project. Host registries,
    /// project files, runtimes, and provider sessions remain untouched.
    ///
    /// # Errors
    ///
    /// Returns an error when the client catalog cannot commit the exact
    /// client-only visibility change atomically.
    pub fn ignore_project_locations(&mut self, project_id: ProjectId) -> Result<usize, StateError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StateError::Sqlite)?;
        let hidden = transaction
            .execute(
                "INSERT OR IGNORE INTO ignored_project_locations (host_id, location_id)
                 SELECT host_id, location_id FROM project_locations WHERE project_id = ?1",
                [project_id.to_string()],
            )
            .map_err(StateError::Sqlite)?;
        transaction.commit().map_err(StateError::Sqlite)?;
        Ok(hidden)
    }
}

fn associate_project_location(
    transaction: &rusqlite::Transaction<'_>,
    host_id: HostId,
    location_id: LocationId,
    display_name: &str,
    repository_fingerprint: Option<&str>,
) -> Result<ClientProjectLocation, StateError> {
    let existing: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT projects.project_id, projects.display_name,
                    projects.repository_fingerprint
             FROM project_locations
             JOIN projects ON projects.project_id = project_locations.project_id
             WHERE project_locations.host_id = ?1 AND project_locations.location_id = ?2",
            params![host_id.to_string(), location_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let matching = repository_fingerprint
        .map(|fingerprint| {
            transaction
                .query_row(
                    "SELECT project_id, display_name FROM projects
                     WHERE repository_fingerprint = ?1",
                    [fingerprint],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(StateError::Sqlite)
        })
        .transpose()?
        .flatten();

    if let Some(existing) = existing {
        return reassociate_existing_project(
            transaction,
            host_id,
            location_id,
            display_name,
            repository_fingerprint,
            existing,
            matching,
        );
    }

    let project = if let Some((project_id, display_name)) = matching {
        project_location(
            &project_id,
            display_name,
            repository_fingerprint.map(str::to_owned),
        )?
    } else {
        create_project(transaction, display_name, repository_fingerprint)?
    };
    transaction
        .execute(
            "INSERT INTO project_locations (project_id, host_id, location_id)
             VALUES (?1, ?2, ?3)",
            params![
                project.project_id.to_string(),
                host_id.to_string(),
                location_id.to_string(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(project)
}

#[allow(clippy::too_many_arguments)]
fn reassociate_existing_project(
    transaction: &rusqlite::Transaction<'_>,
    host_id: HostId,
    location_id: LocationId,
    display_name: &str,
    repository_fingerprint: Option<&str>,
    existing: (String, String, Option<String>),
    matching: Option<(String, String)>,
) -> Result<ClientProjectLocation, StateError> {
    let (existing_id, existing_name, existing_fingerprint) = existing;
    if let Some((matching_id, matching_name)) = matching {
        if matching_id != existing_id {
            transaction
                .execute(
                    "UPDATE project_locations SET project_id = ?1
                     WHERE host_id = ?2 AND location_id = ?3 AND project_id = ?4",
                    params![
                        matching_id,
                        host_id.to_string(),
                        location_id.to_string(),
                        existing_id,
                    ],
                )
                .map_err(StateError::Sqlite)?;
            delete_orphan_project(transaction, &existing_id)?;
        }
        return project_location(
            &matching_id,
            matching_name,
            repository_fingerprint.map(str::to_owned),
        );
    }
    if repository_fingerprint.is_none() {
        if existing_fingerprint.is_none() && existing_name != display_name {
            let location_count = project_location_count(transaction, &existing_id)?;
            if location_count == 1 {
                transaction
                    .execute(
                        "UPDATE projects SET display_name = ?1,
                         revision = revision + 1 WHERE project_id = ?2",
                        params![display_name, existing_id],
                    )
                    .map_err(StateError::Sqlite)?;
                return project_location(&existing_id, display_name.to_owned(), None);
            }
        }
        return project_location(&existing_id, existing_name, existing_fingerprint);
    }
    if existing_fingerprint.as_deref() == repository_fingerprint {
        return project_location(&existing_id, existing_name, existing_fingerprint);
    }

    if project_location_count(transaction, &existing_id)? == 1 {
        transaction
            .execute(
                "UPDATE projects SET repository_fingerprint = ?1,
                     display_name = ?2, revision = revision + 1
                 WHERE project_id = ?3",
                params![repository_fingerprint, display_name, existing_id],
            )
            .map_err(StateError::Sqlite)?;
        return project_location(
            &existing_id,
            display_name.to_owned(),
            repository_fingerprint.map(str::to_owned),
        );
    }

    let project = create_project(transaction, display_name, repository_fingerprint)?;
    transaction
        .execute(
            "UPDATE project_locations SET project_id = ?1
             WHERE host_id = ?2 AND location_id = ?3 AND project_id = ?4",
            params![
                project.project_id.to_string(),
                host_id.to_string(),
                location_id.to_string(),
                existing_id,
            ],
        )
        .map_err(StateError::Sqlite)?;
    Ok(project)
}

fn project_location_count(
    transaction: &rusqlite::Transaction<'_>,
    project_id: &str,
) -> Result<i64, StateError> {
    transaction
        .query_row(
            "SELECT COUNT(*) FROM project_locations WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .map_err(StateError::Sqlite)
}

fn create_project(
    transaction: &rusqlite::Transaction<'_>,
    display_name: &str,
    repository_fingerprint: Option<&str>,
) -> Result<ClientProjectLocation, StateError> {
    let project_id = ProjectId::new();
    transaction
        .execute(
            "INSERT INTO projects (
                project_id, display_name, repository_fingerprint, revision
             ) VALUES (?1, ?2, ?3, 1)",
            params![project_id.to_string(), display_name, repository_fingerprint],
        )
        .map_err(StateError::Sqlite)?;
    Ok(ClientProjectLocation {
        project_id,
        display_name: display_name.to_owned(),
        repository_fingerprint: repository_fingerprint.map(str::to_owned),
    })
}

fn project_location(
    project_id: &str,
    display_name: String,
    repository_fingerprint: Option<String>,
) -> Result<ClientProjectLocation, StateError> {
    Ok(ClientProjectLocation {
        project_id: Uuid::parse_str(project_id)
            .map(ProjectId::from)
            .map_err(StateError::InvalidPersistedUuid)?,
        display_name,
        repository_fingerprint,
    })
}

fn delete_orphan_project(
    transaction: &rusqlite::Transaction<'_>,
    project_id: &str,
) -> Result<(), StateError> {
    transaction
        .execute(
            "DELETE FROM projects WHERE project_id = ?1
             AND NOT EXISTS (
                SELECT 1 FROM project_locations WHERE project_id = ?1
             )",
            [project_id],
        )
        .map_err(StateError::Sqlite)?;
    Ok(())
}

fn ensure_local_client_host(
    transaction: &rusqlite::Transaction<'_>,
    identity: &HostIdentity,
    executable_path: &Path,
) -> Result<(), StateError> {
    let existing = load_client_host_by_alias(transaction, "local")?;
    let Some(existing) = existing else {
        transaction
            .execute(
                "INSERT INTO hosts (
                    host_alias, host_id, registry_generation, executable_path,
                    transport, ssh_destination, capabilities_json, revision
                 ) VALUES ('local', ?1, ?2, ?3, 'local', NULL, ?4, 1)",
                params![
                    identity.host_id.to_string(),
                    identity.registry_generation,
                    executable_path.to_string_lossy(),
                    serialize_capabilities(&Capabilities::default())?,
                ],
            )
            .map_err(StateError::Sqlite)?;
        return Ok(());
    };
    if existing.host_id != identity.host_id {
        return Err(StateError::ClientHostIdentityMismatch);
    }
    if !matches!(existing.transport, ClientHostTransport::Local) {
        return Err(StateError::ClientHostRegistrationMismatch);
    }
    if !existing.registry_generation.is_empty()
        && existing.registry_generation != identity.registry_generation
    {
        return Err(StateError::ClientHostGenerationMismatch);
    }
    if existing.registry_generation == identity.registry_generation
        && existing.executable_path == executable_path
    {
        return Ok(());
    }
    let changed = transaction
        .execute(
            "UPDATE hosts SET registry_generation = ?1, executable_path = ?2,
                 revision = revision + 1
             WHERE host_alias = 'local' AND host_id = ?3 AND revision = ?4",
            params![
                identity.registry_generation,
                executable_path.to_string_lossy(),
                identity.host_id.to_string(),
                existing.revision.value(),
            ],
        )
        .map_err(StateError::Sqlite)?;
    if changed != 1 {
        return Err(StateError::ConcurrentWrite);
    }
    Ok(())
}

fn load_client_host_by_alias(
    connection: &rusqlite::Transaction<'_>,
    alias: &str,
) -> Result<Option<ClientHost>, StateError> {
    connection
        .query_row(
            "SELECT host_alias, host_id, registry_generation, executable_path,
                    transport, ssh_destination, capabilities_json, revision
             FROM hosts WHERE host_alias = ?1",
            [alias],
            row_to_client_host,
        )
        .optional()
        .map_err(StateError::Sqlite)
}

fn row_to_client_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClientHost> {
    let alias: String = row.get(0)?;
    let host_id: String = row.get(1)?;
    let registry_generation: String = row.get(2)?;
    let executable_path: String = row.get(3)?;
    let transport: String = row.get(4)?;
    let destination: Option<String> = row.get(5)?;
    let capabilities_json: String = row.get(6)?;
    let revision: i64 = row.get(7)?;
    let host_id = Uuid::parse_str(&host_id)
        .map(HostId::from)
        .map_err(to_from_sql_error)?;
    let capabilities = serde_json::from_str(&capabilities_json)
        .map_err(|_| to_from_sql_error(StateError::InvalidPersistedCapabilities))?;
    let transport = match transport.as_str() {
        "local" => ClientHostTransport::Local,
        "ssh" => ClientHostTransport::Ssh {
            destination: destination.ok_or_else(|| {
                to_from_sql_error(StateError::InvalidPersistedValue(
                    "missing SSH destination".to_owned(),
                ))
            })?,
        },
        _ => {
            return Err(to_from_sql_error(StateError::InvalidPersistedValue(
                transport,
            )));
        }
    };
    Ok(ClientHost {
        alias,
        host_id,
        registry_generation,
        executable_path: PathBuf::from(executable_path),
        transport,
        capabilities,
        revision: Revision::try_from(revision).map_err(to_from_sql_error)?,
    })
}

fn validate_unchanged_ssh_registration(
    existing: &ClientHost,
    candidate: &ClientHost,
) -> Result<(), StateError> {
    if existing.host_id != candidate.host_id {
        return Err(StateError::ClientHostIdentityMismatch);
    }
    if existing.registry_generation != candidate.registry_generation {
        return Err(StateError::ClientHostGenerationMismatch);
    }
    if existing.capabilities != candidate.capabilities {
        return Err(StateError::ClientHostCapabilitiesMismatch);
    }
    if existing.executable_path != candidate.executable_path
        || existing.transport != candidate.transport
    {
        return Err(StateError::ClientHostRegistrationMismatch);
    }
    Ok(())
}

pub(in crate::state) fn serialize_capabilities(
    capabilities: &Capabilities,
) -> Result<String, StateError> {
    serde_json::to_string(capabilities).map_err(StateError::ClientCapabilitiesEncoding)
}
