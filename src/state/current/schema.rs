//! Validation of the accepted schema-15 catalog and durable identities.
//!
//! Bootstrap may classify raw `SQLite` headers without opening a database; this
//! module owns the full bounded validator used after that identity is proven.

use super::{
    BootstrapOperationalMetadata, Connection, HOST_SCHEMA_VERSION, OnboardingPhase,
    OnboardingProviderExecutableIdentity, OperationId, OptionalExtension,
    PARKED_RECOVERY_RESOLVED_OUTCOME, PersistedOnboardingIntent, ProviderKind, StateError, Uuid,
    operation_phase_from_text, table_exists, table_has_column_readonly, validate_foreign_keys,
    validate_host_identity, validate_project_catalog, validate_table_columns,
};

pub(super) fn schema_version(connection: &Connection) -> Result<i64, StateError> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| match error {
            rusqlite::Error::SqliteFailure(_, _) => StateError::MalformedHostSchema,
            error => StateError::Sqlite(error),
        })
}

/// Validates the direct current schema-15 shape. This validator is intentionally
/// independent from the historical schema-12/13/14 validators so a current
/// open cannot accidentally accept a migration-era table or metadata row.
pub(super) fn validate_schema15(connection: &Connection) -> Result<(), StateError> {
    if schema_version(connection)? != HOST_SCHEMA_VERSION {
        return Err(StateError::MalformedHostSchema);
    }
    validate_host_identity(connection, HOST_SCHEMA_VERSION)?;
    for (table, columns) in required_schema15_tables() {
        validate_exact_table_columns(connection, table, columns)?;
    }
    if table_exists(connection, "project_browser_settings")?
        || !table_has_column_readonly(connection, "project_locations", "project_id")?
    {
        return Err(StateError::MalformedHostSchema);
    }
    validate_schema15_catalog(connection)?;
    validate_project_catalog(connection)?;
    validate_table_columns(
        connection,
        "compound_operations",
        &[
            "launch_token_id",
            "launch_token_verifier",
            "launch_token_expiry_monotonic",
            "launch_claims_digest",
        ],
    )?;
    validate_table_columns(
        connection,
        "host_operational_metadata",
        &[
            "singleton",
            "bootstrap_host_id",
            "bootstrap_generation",
            "provisional_lease_generation",
            "provisional_lock_phase",
            "provisional_lock_device",
            "provisional_lock_inode",
        ],
    )?;
    let metadata: Option<BootstrapOperationalMetadata> = connection
        .query_row(
            "SELECT bootstrap_host_id, bootstrap_generation,
                    provisional_lease_generation, provisional_lock_phase,
                    provisional_lock_device, provisional_lock_inode
             FROM host_operational_metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(StateError::Sqlite)?;
    let Some((host_id, generation, lease_generation, phase, device, inode)) = metadata else {
        return Err(StateError::MalformedHostSchema);
    };
    if Uuid::parse_str(&host_id).is_err()
        || Uuid::parse_str(&generation).is_err()
        || lease_generation <= 0
        || !matches!(phase.as_str(), "pending" | "ready")
        || (phase == "pending" && (device.is_some() || inode.is_some()))
        || (phase == "ready" && (device.is_none() || inode.is_none()))
        || device.is_some_and(|value| value < 0)
        || inode.is_some_and(|value| value <= 0)
    {
        return Err(StateError::MalformedHostSchema);
    }
    validate_onboarding_operation_columns(connection)?;
    validate_schema15_onboarding_exec_targets(connection)?;
    validate_foreign_keys(connection)
}

pub(super) fn validate_schema15_catalog(connection: &Connection) -> Result<(), StateError> {
    const ALLOWED_TABLES: &[&str] = &[
        "host_identity",
        "codex_integrations",
        "project_locations",
        "workstreams",
        "independent_creation_requests",
        "runtimes",
        "opencode_runtime_handles",
        "provider_bindings",
        "attention_states",
        "compound_operations",
        "projects",
        "opencode_settled_messages",
        "host_operational_metadata",
        "onboarding_exec_targets",
        "sqlite_sequence",
    ];
    const REQUIRED_INDEXES: &[&str] = &[
        "compound_operations_phase_idx",
        "compound_operations_launch_token_id_idx",
        "project_repository_fingerprint_idx",
        "opencode_settled_messages_runtime_idx",
    ];
    let mut statement = connection
        .prepare("SELECT type, name FROM sqlite_master ORDER BY type, name")
        .map_err(StateError::Sqlite)?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<(String, String)>, _>>()
        .map_err(StateError::Sqlite)?;
    for (kind, name) in &objects {
        match kind.as_str() {
            "table" if ALLOWED_TABLES.contains(&name.as_str()) => {}
            "index"
                if name.starts_with("sqlite_autoindex_")
                    || REQUIRED_INDEXES.contains(&name.as_str()) => {}
            _ => return Err(StateError::MalformedHostSchema),
        }
    }
    for index in REQUIRED_INDEXES {
        if !objects
            .iter()
            .any(|(kind, name)| kind == "index" && name == index)
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "the direct schema column contract is kept explicit for exact validation"
)]
pub(super) fn required_schema15_tables() -> [(&'static str, &'static [&'static str]); 15] {
    [
        (
            "host_identity",
            &[
                "singleton",
                "host_id",
                "registry_generation",
                "schema_version",
            ],
        ),
        (
            "codex_integrations",
            &[
                "integration_id",
                "profile_name",
                "canonical_profile_path",
                "owner_id",
                "profile_schema_version",
                "hook_executable_path",
                "generated_content_hash",
                "lifecycle",
                "revision",
            ],
        ),
        (
            "project_locations",
            &[
                "location_id",
                "repository_path",
                "repository_display_name",
                "remote_identity_fingerprint",
                "remote_identity_display",
                "revision",
                "project_id",
            ],
        ),
        (
            "workstreams",
            &[
                "workstream_id",
                "location_id",
                "provider",
                "origin",
                "source_workstream_id",
                "lifecycle",
                "archived_at_millis",
                "last_activity_sequence",
                "last_activity_at_millis",
                "revision",
            ],
        ),
        (
            "independent_creation_requests",
            &[
                "request_key",
                "source_workstream_id",
                "source_revision",
                "workstream_id",
            ],
        ),
        (
            "runtimes",
            &[
                "runtime_id",
                "workstream_id",
                "provider",
                "tmux_generation",
                "tmux_session",
                "cwd",
                "provider_pid",
                "process_birth",
                "lifecycle",
                "revision",
            ],
        ),
        (
            "opencode_runtime_handles",
            &[
                "runtime_id",
                "runtime_generation",
                "endpoint_host",
                "endpoint_port",
                "version",
                "native_session_id",
                "observer_pid",
                "observer_birth",
                "observer_status",
                "revision",
            ],
        ),
        (
            "provider_bindings",
            &[
                "binding_id",
                "runtime_id",
                "provider",
                "native_session_id",
                "start_source",
                "last_settled_turn_id",
                "observed_thread_name",
                "name_state",
                "name_observed_at",
                "predecessor_native_session_id",
                "predecessor_effective_name",
                "runtime_generation",
                "revision",
            ],
        ),
        (
            "attention_states",
            &[
                "workstream_id",
                "result_unseen_since_revision",
                "recovery_unseen_since_revision",
                "latest_native_session_id",
                "latest_native_session_provider",
                "latest_turn_id",
                "revision",
            ],
        ),
        (
            "compound_operations",
            &[
                "operation_id",
                "request_key",
                "kind",
                "phase",
                "expected_revisions_json",
                "effect_watermark",
                "outcome_json",
                "revision",
                "launch_token_id",
                "launch_token_verifier",
                "launch_token_expiry_monotonic",
                "launch_claims_digest",
            ],
        ),
        (
            "projects",
            &[
                "project_id",
                "label_location_id",
                "display_name",
                "repository_fingerprint",
                "revision",
            ],
        ),
        (
            "opencode_settled_messages",
            &[
                "settled_message_id",
                "runtime_id",
                "runtime_generation",
                "native_session_id",
                "message_id",
            ],
        ),
        (
            "host_operational_metadata",
            &[
                "singleton",
                "bootstrap_host_id",
                "bootstrap_generation",
                "provisional_lease_generation",
                "provisional_lock_phase",
                "provisional_lock_device",
                "provisional_lock_inode",
            ],
        ),
        (
            "onboarding_exec_targets",
            &[
                "operation_id",
                "provider",
                "executable_device",
                "executable_inode",
            ],
        ),
        ("sqlite_sequence", &["name", "seq"]),
    ]
}

pub(super) fn validate_exact_table_columns(
    connection: &Connection,
    table: &str,
    expected: &[&str],
) -> Result<(), StateError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(StateError::Sqlite)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(StateError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(StateError::Sqlite)?;
    if columns.len() != expected.len()
        || columns
            .iter()
            .zip(expected)
            .any(|(actual, expected)| actual != expected)
    {
        return Err(StateError::MalformedHostSchema);
    }
    Ok(())
}

pub(super) fn validate_schema15_onboarding_exec_targets(
    connection: &Connection,
) -> Result<(), StateError> {
    validate_table_columns(
        connection,
        "onboarding_exec_targets",
        &[
            "operation_id",
            "provider",
            "executable_device",
            "executable_inode",
        ],
    )?;
    let mut statement = connection
        .prepare(
            "SELECT targets.operation_id, targets.provider,
                    targets.executable_device, targets.executable_inode,
                    operations.kind, operations.phase, operations.expected_revisions_json,
                    operations.outcome_json
             FROM onboarding_exec_targets AS targets
             JOIN compound_operations AS operations
               ON operations.operation_id = targets.operation_id
             ORDER BY targets.operation_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let operation_id: String = row.get(0).map_err(StateError::Sqlite)?;
        let provider: String = row.get(1).map_err(StateError::Sqlite)?;
        let device: i64 = row.get(2).map_err(StateError::Sqlite)?;
        let inode: i64 = row.get(3).map_err(StateError::Sqlite)?;
        let kind: String = row.get(4).map_err(StateError::Sqlite)?;
        let phase: String = row.get(5).map_err(StateError::Sqlite)?;
        let encoded_intent: String = row.get(6).map_err(StateError::Sqlite)?;
        let outcome_json: Option<String> = row.get(7).map_err(StateError::Sqlite)?;
        if operation_id.parse::<OperationId>().is_err() || kind != "onboard" {
            return Err(StateError::MalformedHostSchema);
        }
        provider
            .parse::<ProviderKind>()
            .map_err(|_| StateError::MalformedHostSchema)?;
        OnboardingProviderExecutableIdentity::new(
            u64::try_from(device).map_err(|_| StateError::MalformedHostSchema)?,
            u64::try_from(inode).map_err(|_| StateError::MalformedHostSchema)?,
        )
        .map_err(|_| StateError::MalformedHostSchema)?;
        let operation_phase =
            operation_phase_from_text(&phase).map_err(|_| StateError::MalformedHostSchema)?;
        let committed_park_resolution = operation_phase == crate::domain::OperationPhase::Committed;
        let onboarding_phase = OnboardingPhase::from_operation_phase(operation_phase);
        if committed_park_resolution {
            if outcome_json.as_deref() != Some(PARKED_RECOVERY_RESOLVED_OUTCOME) {
                return Err(StateError::MalformedHostSchema);
            }
        } else if onboarding_phase.is_none() {
            return Err(StateError::MalformedHostSchema);
        }
        let intent: PersistedOnboardingIntent =
            serde_json::from_str(&encoded_intent).map_err(|_| StateError::MalformedHostSchema)?;
        if intent.version != 1
            || intent.provider
                != provider
                    .parse()
                    .map_err(|_| StateError::MalformedHostSchema)?
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

pub(super) fn validate_onboarding_operation_columns(
    connection: &Connection,
) -> Result<(), StateError> {
    validate_table_columns(
        connection,
        "compound_operations",
        &[
            "launch_token_id",
            "launch_token_verifier",
            "launch_token_expiry_monotonic",
            "launch_claims_digest",
        ],
    )?;
    let mut statement = connection
        .prepare(
            "SELECT kind, phase, launch_token_id, launch_token_verifier,
                    launch_token_expiry_monotonic, launch_claims_digest, outcome_json
             FROM compound_operations ORDER BY operation_id",
        )
        .map_err(StateError::Sqlite)?;
    let mut rows = statement.query([]).map_err(StateError::Sqlite)?;
    while let Some(row) = rows.next().map_err(StateError::Sqlite)? {
        let kind: String = row.get(0).map_err(StateError::Sqlite)?;
        let phase: String = row.get(1).map_err(StateError::Sqlite)?;
        let token_id: Option<String> = row.get(2).map_err(StateError::Sqlite)?;
        let token_verifier: Option<String> = row.get(3).map_err(StateError::Sqlite)?;
        let token_expiry: Option<i64> = row.get(4).map_err(StateError::Sqlite)?;
        let claims_digest: Option<String> = row.get(5).map_err(StateError::Sqlite)?;
        let outcome_json: Option<String> = row.get(6).map_err(StateError::Sqlite)?;
        if kind == "onboard" {
            validate_onboarding_operation(
                &phase,
                token_id.as_deref(),
                token_verifier.as_deref(),
                token_expiry,
                claims_digest.as_deref(),
                outcome_json.as_deref(),
            )?;
        } else if token_id.is_some()
            || token_verifier.is_some()
            || token_expiry.is_some()
            || claims_digest.is_some()
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

pub(super) fn validate_onboarding_operation(
    phase: &str,
    token_id: Option<&str>,
    token_verifier: Option<&str>,
    token_expiry: Option<i64>,
    claims_digest: Option<&str>,
    outcome_json: Option<&str>,
) -> Result<(), StateError> {
    let operation_phase =
        operation_phase_from_text(phase).map_err(|_| StateError::MalformedHostSchema)?;
    let committed_park_resolution = operation_phase == crate::domain::OperationPhase::Committed;
    let phase = OnboardingPhase::from_operation_phase(operation_phase);
    if phase.is_none() && !committed_park_resolution {
        return Err(StateError::MalformedHostSchema);
    }
    if committed_park_resolution && outcome_json != Some(PARKED_RECOVERY_RESOLVED_OUTCOME) {
        return Err(StateError::MalformedHostSchema);
    }
    let capability_is_absent = token_id.is_none()
        && token_verifier.is_none()
        && token_expiry.is_none()
        && claims_digest.is_none();
    let capability_is_complete = matches!(
        (token_id, token_verifier, token_expiry, claims_digest),
        (Some(_), Some(_), Some(_), Some(_))
    );
    match phase {
        None if committed_park_resolution && !capability_is_complete => {
            return Err(StateError::MalformedHostSchema);
        }
        None if committed_park_resolution => {}
        Some(OnboardingPhase::Prepared) if !capability_is_absent => {
            return Err(StateError::MalformedHostSchema);
        }
        Some(OnboardingPhase::RolledBack) if !(capability_is_absent || capability_is_complete) => {
            return Err(StateError::MalformedHostSchema);
        }
        Some(OnboardingPhase::Prepared | OnboardingPhase::RolledBack) => {}
        _ if !capability_is_complete => return Err(StateError::MalformedHostSchema),
        _ => {}
    }
    if capability_is_complete {
        let token_id = token_id.ok_or(StateError::MalformedHostSchema)?;
        let token_verifier = token_verifier.ok_or(StateError::MalformedHostSchema)?;
        let token_expiry = token_expiry.ok_or(StateError::MalformedHostSchema)?;
        let claims_digest = claims_digest.ok_or(StateError::MalformedHostSchema)?;
        if Uuid::parse_str(token_id).is_err()
            || token_expiry <= 0
            || !is_versioned_sha256(token_verifier, "wsnav-launch-verifier-v1:sha256:")
            || !is_versioned_sha256(claims_digest, "wsnav-launch-claims-v1:sha256:")
        {
            return Err(StateError::MalformedHostSchema);
        }
    }
    Ok(())
}

pub(super) fn is_versioned_sha256(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}
