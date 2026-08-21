//! D16 launcher-owned state classification and confirmation.
//!
//! This boundary runs before presentation reuse.  Classification performs no
//! creation, migration, cleanup, provider action, or process signal.  Only an
//! ordinary interactive caller may turn a [`StartupAssessment::Cutover`] into
//! a confirmed cutover orchestration run.

use std::{
    fs,
    io::{BufRead, Write},
};

use rusqlite::{Connection, OpenFlags};

use thiserror::Error;

use crate::{
    cutover::{
        CutoverConfirmationInput, CutoverConfirmationSummary, CutoverError, CutoverPlan,
        PresentationProofSource, discover_cutover,
    },
    state::{
        D16_HOST_SCHEMA_VERSION, D16_SCHEMA_12_VERSION, D16State, FreshRootClassification,
        FreshRootRejection, LEGACY_CLIENT_DATABASE_FILE, LEGACY_CLIENT_DATABASE_SHM_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE, StateError, StateRoot, classify_fresh_root,
        open_current_only,
    },
};

/// The only safe outcomes of pre-presentation startup classification.
#[allow(
    clippy::large_enum_variant,
    reason = "The current state handle is kept behind a box so the assessment remains a small launcher value."
)]
pub enum StartupAssessment {
    Current(Box<D16State>),
    Fresh(FreshRootClassification),
    Cutover(CutoverPlan),
}

/// Typed launcher failures; operator input is intentionally not echoed.
#[derive(Debug, Error)]
pub enum StartupError {
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Cutover(#[from] CutoverError),
    #[error("could not read D16 cutover confirmation")]
    ConfirmationIo(#[source] std::io::Error),
}

/// Classifies the selected state root without adopting or changing it.
///
/// # Errors
///
/// Returns a typed state, presentation, or read-only schema inspection error
/// when the root cannot be classified safely.
pub fn assess_startup<P: PresentationProofSource>(
    root: &StateRoot,
    presentation: &mut P,
) -> Result<StartupAssessment, StartupError> {
    match classify_fresh_root(root.base()) {
        Ok(classification) => return Ok(StartupAssessment::Fresh(classification)),
        Err(StateError::FreshRootRejected(FreshRootRejection::UnknownArtifact)) => {}
        Err(error) => return Err(error.into()),
    }
    // Classify host evidence before inspecting a presentation. A missing,
    // pre-12, or future host database beside a legacy artifact is recovery
    // state, not authority to prompt, acquire a lease, or retire anything.
    // A normal schema-13 presentation is current reconnect state and is owned
    // by `Presentation::open_or_create`, never by the legacy cutover parser.
    let has_client = has_legacy_client_artifact(root.base())?;
    let schema = host_schema_version(root)?;
    match schema {
        None => {
            return Err(StateError::StateRecoveryRequired(
                crate::state::StateRecoveryReason::MissingHostDatabase,
            )
            .into());
        }
        Some(value) if value < D16_SCHEMA_12_VERSION => {
            return Err(StateError::StateRecoveryRequired(
                crate::state::StateRecoveryReason::UnsupportedLegacySchema,
            )
            .into());
        }
        Some(value) if value > D16_HOST_SCHEMA_VERSION => {
            return Err(StateError::UnsupportedFutureHostSchema(value).into());
        }
        Some(D16_HOST_SCHEMA_VERSION) if !has_client => {
            return open_current_only(root)
                .map(|state| StartupAssessment::Current(Box::new(state)))
                .map_err(Into::into);
        }
        Some(D16_SCHEMA_12_VERSION | D16_HOST_SCHEMA_VERSION) => {}
        Some(_) => return Err(StateError::MalformedHostSchema.into()),
    }

    if schema == Some(D16_SCHEMA_12_VERSION) || has_client {
        let plan = discover_cutover(presentation, root.base())?;
        return Ok(StartupAssessment::Cutover(plan));
    }

    Err(StateError::MalformedHostSchema.into())
}

fn has_legacy_client_artifact(root: &std::path::Path) -> Result<bool, StartupError> {
    for name in [
        LEGACY_CLIENT_DATABASE_FILE,
        LEGACY_CLIENT_DATABASE_WAL_FILE,
        LEGACY_CLIENT_DATABASE_SHM_FILE,
    ] {
        match fs::symlink_metadata(root.join(name)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(StateError::Io {
                    path: root.to_path_buf(),
                    source: error,
                }
                .into());
            }
        }
    }
    Ok(false)
}

fn host_schema_version(root: &StateRoot) -> Result<Option<i64>, StartupError> {
    let path = root.host_database_path();
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(StateError::MalformedHostSchema.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(StateError::Io {
                path: path.clone(),
                source: error,
            }
            .into());
        }
    }
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(StateError::Sqlite)?;
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| StateError::MalformedHostSchema)?;
    Ok(Some(version))
}

/// Shows the complete fixed D16 clean-break summary and reads one bounded line.
/// Only an exact `yes` (case-insensitive, surrounding whitespace ignored)
/// confirms.  EOF and every other input decline without mutation.
///
/// # Errors
///
/// Returns [`StartupError::ConfirmationIo`] when the bounded confirmation
/// summary or response cannot be read or flushed.
pub fn prompt_cutover_confirmation<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
) -> Result<CutoverConfirmationInput, StartupError> {
    let summary = CutoverConfirmationSummary::standard();
    writeln!(output, "Workstream Navigator D16 host-local cutover")
        .map_err(StartupError::ConfirmationIo)?;
    writeln!(output, "Discarded without being read or imported:")
        .map_err(StartupError::ConfirmationIo)?;
    writeln!(
        output,
        "  - {LEGACY_CLIENT_DATABASE_FILE}, {LEGACY_CLIENT_DATABASE_WAL_FILE}, and {LEGACY_CLIENT_DATABASE_SHM_FILE}"
    )
    .map_err(StartupError::ConfirmationIo)?;
    for category in summary.discarded() {
        writeln!(output, "  - {}", category.label()).map_err(StartupError::ConfirmationIo)?;
    }
    writeln!(output, "Preserved on this host:").map_err(StartupError::ConfirmationIo)?;
    for category in summary.preserved() {
        writeln!(output, "  - {}", category.label()).map_err(StartupError::ConfirmationIo)?;
    }
    writeln!(
        output,
        "No automatic backup or downgrade path is created. Type yes to continue:"
    )
    .map_err(StartupError::ConfirmationIo)?;
    output.flush().map_err(StartupError::ConfirmationIo)?;

    let mut response = Vec::new();
    let mut limited = std::io::Read::take(std::io::Read::by_ref(input), 33);
    let read = limited
        .read_until(b'\n', &mut response)
        .map_err(StartupError::ConfirmationIo)?;
    let bounded_line =
        read > 0 && response.len() <= 32 && (response.ends_with(b"\n") || response.len() < 32);
    let accepted = bounded_line
        && std::str::from_utf8(&response)
            .is_ok_and(|value| value.trim().eq_ignore_ascii_case("yes"));
    if !accepted {
        return Ok(CutoverConfirmationInput::declined_interactive());
    }
    Ok(CutoverConfirmationInput::confirmed_interactive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cutover::PresentationProofSource,
        presentation::LegacyPresentationAssessment,
        state::{StateRecoveryReason, fresh_create},
    };
    use std::io::Cursor;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[derive(Default)]
    struct NoPresentation;

    impl PresentationProofSource for NoPresentation {
        fn prove(
            &mut self,
            _state_root: &std::path::Path,
        ) -> Result<LegacyPresentationAssessment, CutoverError> {
            Err(CutoverError::PresentationInspection(
                "fresh classification must not inspect presentation".to_owned(),
            ))
        }
    }

    #[cfg(unix)]
    fn private_directory(path: &std::path::Path) {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    fn private_file(path: &std::path::Path) {
        std::fs::File::create(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn absent_root_is_classified_without_creation() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("absent");
        let root = StateRoot::select(&path);
        let assessment = assess_startup(&root, &mut NoPresentation).unwrap();
        assert!(matches!(
            assessment,
            StartupAssessment::Fresh(FreshRootClassification::Absent)
        ));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn exact_private_transition_lock_only_root_remains_fresh() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let lock = path.join("transition.lock");
        std::fs::File::create(&lock).unwrap();
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
        let root = StateRoot::select(&path);
        let assessment = assess_startup(&root, &mut NoPresentation).unwrap();
        assert!(matches!(
            assessment,
            StartupAssessment::Fresh(FreshRootClassification::TransitionLeaseOnly)
        ));
        assert!(!path.join("host.sqlite").exists());
    }

    #[cfg(unix)]
    #[test]
    fn missing_host_beside_client_artifact_is_recovery_without_presentation_inspection() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        private_directory(&path);
        private_file(&path.join(LEGACY_CLIENT_DATABASE_FILE));
        let error = assess_startup(&StateRoot::select(&path), &mut NoPresentation)
            .err()
            .expect("missing host must refuse");
        assert!(matches!(
            error,
            StartupError::State(StateError::StateRecoveryRequired(
                StateRecoveryReason::MissingHostDatabase
            ))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_host_versions_refuse_before_presentation_inspection() {
        for (version, future) in [(11_i64, false), (14_i64, true)] {
            let temporary = tempdir().unwrap();
            let path = temporary.path().join("state");
            private_directory(&path);
            let database = path.join("host.sqlite");
            let connection = Connection::open(&database).unwrap();
            connection
                .execute_batch(&format!("PRAGMA user_version = {version}"))
                .unwrap();
            drop(connection);
            std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600)).unwrap();
            private_file(&path.join(LEGACY_CLIENT_DATABASE_FILE));

            let error = assess_startup(&StateRoot::select(&path), &mut NoPresentation)
                .err()
                .expect("unsupported host must refuse");
            if future {
                assert!(matches!(
                    error,
                    StartupError::State(StateError::UnsupportedFutureHostSchema(14))
                ));
            } else {
                assert!(matches!(
                    error,
                    StartupError::State(StateError::StateRecoveryRequired(
                        StateRecoveryReason::UnsupportedLegacySchema
                    ))
                ));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn current_schema_ignores_current_presentation_directory_during_startup() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("state");
        let state = fresh_create(&path, &crate::domain::RandomIdGenerator).unwrap();
        drop(state);
        private_directory(&path.join("presentation"));

        let assessment = assess_startup(&StateRoot::select(&path), &mut NoPresentation).unwrap();
        assert!(matches!(assessment, StartupAssessment::Current(_)));
    }

    #[test]
    fn confirmation_is_complete_bounded_and_exact() {
        let mut output = Vec::new();
        let confirmed =
            prompt_cutover_confirmation(&mut Cursor::new(b" yes \n"), &mut output).unwrap();
        assert!(confirmed.confirmed);
        let rendered = String::from_utf8(output).unwrap();
        for category in CutoverConfirmationSummary::standard().discarded() {
            assert!(rendered.contains(category.label()));
        }
        for category in CutoverConfirmationSummary::standard().preserved() {
            assert!(rendered.contains(category.label()));
        }

        let declined =
            prompt_cutover_confirmation(&mut Cursor::new(b"y\n"), &mut Vec::new()).unwrap();
        assert!(!declined.confirmed);

        let overlong = prompt_cutover_confirmation(
            &mut Cursor::new(b"yes                             \n"),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(!overlong.confirmed);
    }
}
