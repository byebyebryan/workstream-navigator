//! Exact ownership and atomic file handling for the scoped Codex observer profile.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

pub const OBSERVER_PROFILE_NAME: &str = "wsnav-observer";
const PROFILE_MARKER: &str = "# Managed by Workstream Navigator. Do not edit manually.\n";

/// The immutable record retained by host state for an owned profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileOwnership {
    pub canonical_path: PathBuf,
    pub owner_id: String,
    pub hook_executable: PathBuf,
    pub content_hash: String,
}

/// Creates, verifies, or removes only the exact scoped observer profile.
#[derive(Clone, Debug)]
pub struct ObserverProfile {
    codex_home: PathBuf,
    hook_executable: PathBuf,
}

impl ObserverProfile {
    /// Creates a profile manager for one user's normal `CODEX_HOME`.
    #[must_use]
    pub fn new(codex_home: impl Into<PathBuf>, hook_executable: impl Into<PathBuf>) -> Self {
        Self {
            codex_home: codex_home.into(),
            hook_executable: hook_executable.into(),
        }
    }

    /// Returns the only profile path `WSNav` is allowed to manage.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.codex_home
            .join(format!("{OBSERVER_PROFILE_NAME}.config.toml"))
    }

    /// Renders the profile containing only passive lifecycle hooks.
    #[must_use]
    pub fn rendered(&self) -> String {
        let command = shell_quote(&self.hook_executable);
        format!(
            "{PROFILE_MARKER}[features]\nhooks = true\n\n[[hooks.SessionStart]]\nmatcher = \"startup|resume|clear|compact\"\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n\n[[hooks.UserPromptSubmit]]\n[[hooks.UserPromptSubmit.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n\n[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n\n[[hooks.SessionEnd]]\nmatcher = \"other\"\n[[hooks.SessionEnd.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n"
        )
    }

    /// Installs a new profile or verifies an exact already-owned profile.
    ///
    /// # Errors
    ///
    /// Returns an error rather than overwriting a foreign, modified, or
    /// malformed profile path, or if the atomic private write fails.
    pub fn install(
        &self,
        owner_id: String,
        existing: Option<&ProfileOwnership>,
    ) -> Result<ProfileOwnership, ProfileError> {
        let path = self.path();
        let content = self.rendered();
        let expected_hash = hash(&content);
        if path.exists() {
            let Some(existing) = existing else {
                return Err(ProfileError::ForeignPath(path));
            };
            if existing.canonical_path != path || existing.hook_executable != self.hook_executable {
                return Err(ProfileError::OwnershipMismatch);
            }
            let actual = hash_file(&path)?;
            if actual != existing.content_hash || actual != expected_hash {
                return Err(ProfileError::ModifiedPath(path));
            }
            return Ok(existing.clone());
        }
        if existing.is_some() {
            return Err(ProfileError::MissingOwnedPath(path));
        }
        atomic_private_write(&path, content.as_bytes())?;
        Ok(ProfileOwnership {
            canonical_path: path,
            owner_id,
            hook_executable: self.hook_executable.clone(),
            content_hash: expected_hash,
        })
    }

    /// Removes an exact, unchanged owned profile and nothing else.
    ///
    /// # Errors
    ///
    /// Returns an error when ownership is absent or mismatched, the profile was
    /// modified, or the file cannot be removed.
    pub fn remove(&self, ownership: &ProfileOwnership) -> Result<(), ProfileError> {
        let path = self.path();
        if ownership.canonical_path != path || ownership.hook_executable != self.hook_executable {
            return Err(ProfileError::OwnershipMismatch);
        }
        if !path.is_file() {
            return Err(ProfileError::MissingOwnedPath(path));
        }
        if hash_file(&path)? != ownership.content_hash {
            return Err(ProfileError::ModifiedPath(path));
        }
        fs::remove_file(path).map_err(ProfileError::Io)
    }
}

fn shell_quote(executable: &Path) -> String {
    let escaped = executable.to_string_lossy().replace('\'', "'\\\"'\\\"'");
    let command = format!("'{escaped}' _hook");
    toml_string(&command)
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("a Rust string always serializes as JSON")
}

fn hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn hash_file(path: &Path) -> Result<String, ProfileError> {
    fs::read(path)
        .map(|content| format!("{:x}", Sha256::digest(content)))
        .map_err(ProfileError::Io)
}

fn atomic_private_write(path: &Path, content: &[u8]) -> Result<(), ProfileError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProfileError::InvalidPath(path.into()))?;
    fs::create_dir_all(parent).map_err(ProfileError::Io)?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProfileError::Clock)?
        .as_nanos();
    let temporary = parent.join(format!(".wsnav-observer-{suffix}.tmp"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(ProfileError::Io)?;
    set_mode(&temporary, 0o600)?;
    file.write_all(content).map_err(ProfileError::Io)?;
    file.sync_all().map_err(ProfileError::Io)?;
    fs::rename(&temporary, path).map_err(ProfileError::Io)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ProfileError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(ProfileError::Io)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), ProfileError> {
    Ok(())
}

/// Scoped-profile ownership failures.
#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("system clock is before the Unix epoch")]
    Clock,
    #[error("refusing to overwrite foreign observer profile at {0}")]
    ForeignPath(PathBuf),
    #[error("I/O while handling observer profile: {0}")]
    Io(std::io::Error),
    #[error("invalid observer profile path {0}")]
    InvalidPath(PathBuf),
    #[error("owned observer profile is missing at {0}")]
    MissingOwnedPath(PathBuf),
    #[error("owned observer profile was modified at {0}")]
    ModifiedPath(PathBuf),
    #[error("observer profile ownership does not match this manager")]
    OwnershipMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(root: &Path) -> ObserverProfile {
        ObserverProfile::new(root.join("codex-home"), root.join("bin/wsnav"))
    }

    #[test]
    fn profile_is_scoped_to_passive_lifecycle_hooks() {
        let temporary = tempfile::tempdir().unwrap();
        let rendered = manager(temporary.path()).rendered();

        assert!(rendered.contains("[features]\nhooks = true"));
        assert!(rendered.contains("hooks.SessionStart"));
        assert!(rendered.contains("hooks.UserPromptSubmit"));
        assert!(rendered.contains("hooks.Stop"));
        assert!(rendered.contains("hooks.SessionEnd"));
        assert!(!rendered.contains("model"));
        assert!(!rendered.contains("permissions"));
    }

    #[test]
    fn foreign_and_modified_profiles_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        fs::create_dir_all(manager.path().parent().unwrap()).unwrap();
        fs::write(manager.path(), "foreign = true\n").unwrap();
        assert!(matches!(
            manager.install("owner".to_owned(), None),
            Err(ProfileError::ForeignPath(_))
        ));

        fs::remove_file(manager.path()).unwrap();
        let ownership = manager.install("owner".to_owned(), None).unwrap();
        fs::write(manager.path(), "modified = true\n").unwrap();
        assert!(matches!(
            manager.remove(&ownership),
            Err(ProfileError::ModifiedPath(_))
        ));
    }

    #[test]
    fn exact_owned_profile_is_idempotent_and_removable() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let ownership = manager.install("owner".to_owned(), None).unwrap();

        assert_eq!(
            manager
                .install("different-owner".to_owned(), Some(&ownership))
                .unwrap(),
            ownership
        );
        manager.remove(&ownership).unwrap();
        assert!(!manager.path().exists());
    }
}
