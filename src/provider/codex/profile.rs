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
pub const OBSERVER_PROFILE_SCHEMA_VERSION: u8 = 2;
const PROFILE_MARKER: &str = "# Managed by Workstream Navigator. Do not edit manually.\n";
const MAX_PROVIDER_PREFIX_BYTES: usize = 4096;
const MAX_PROVIDER_SCALAR_BYTES: usize = 256;

const PROVIDER_SETTINGS: [&str; 2] = ["model", "model_reasoning_effort"];

/// The immutable record retained by host state for an owned profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileOwnership {
    pub canonical_path: PathBuf,
    pub owner_id: String,
    pub profile_schema_version: u8,
    pub hook_executable: PathBuf,
    pub content_hash: String,
}

/// Creates, verifies, or removes only the exact scoped observer declaration
/// and native trust, preserving any validated provider-owned prefix.
#[derive(Clone, Debug)]
pub struct ObserverProfile {
    codex_home: PathBuf,
    hook_executable: PathBuf,
    state_root: PathBuf,
}

impl ObserverProfile {
    /// Creates a profile manager for one user's normal `CODEX_HOME`.
    #[must_use]
    pub fn new(
        codex_home: impl Into<PathBuf>,
        hook_executable: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            codex_home: codex_home.into(),
            hook_executable: hook_executable.into(),
            state_root: state_root.into(),
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
        let command = hook_command(&self.hook_executable, &self.state_root);
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
            if existing.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
                return Err(ProfileError::UpdateRequired);
            }
            self.verify_owned_document(existing, &expected_hash)?;
            return Ok(existing.clone());
        }
        if existing.is_some() {
            return Err(ProfileError::MissingOwnedPath(path));
        }
        atomic_private_write(&path, content.as_bytes())?;
        Ok(ProfileOwnership {
            canonical_path: path,
            owner_id,
            profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
            hook_executable: self.hook_executable.clone(),
            content_hash: expected_hash,
        })
    }

    /// Removes the exact, unchanged observer declaration and native trust.
    /// A validated provider-owned prefix, when present, remains at the path.
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
        let declaration = self.declaration_for(ownership)?;
        let document = self.read_owned_document(ownership, &hash(&declaration))?;
        if document.provider_prefix.is_empty() {
            fs::remove_file(path).map_err(ProfileError::Io)
        } else {
            atomic_private_write(&path, document.provider_prefix.as_bytes())
        }
    }

    /// Verifies that Codex's native `/hooks` review trusted every generated
    /// lifecycle hook. A syntactically exact but unreviewed profile remains
    /// `trust_pending` and cannot enable managed launches.
    ///
    /// # Errors
    ///
    /// Returns an error if profile ownership changed or native trust is absent
    /// or incomplete.
    pub fn verify_native_trust(&self, ownership: &ProfileOwnership) -> Result<(), ProfileError> {
        if ownership.profile_schema_version != OBSERVER_PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UpdateRequired);
        }
        let suffix = self.owned_native_suffix(ownership, &hash(&self.rendered()))?;
        if suffix.is_some_and(|suffix| has_complete_hook_trust(&suffix, &self.path())) {
            Ok(())
        } else {
            Err(ProfileError::NativeTrustPending)
        }
    }

    /// Replaces an exact owned declaration with this executable's declaration.
    /// Codex's co-located trust records are deliberately discarded because they
    /// hash the old declaration. The returned ownership must be recorded as
    /// `trust_pending` until native review completes again.
    ///
    /// # Errors
    ///
    /// Returns an error rather than replacing a foreign, missing, or modified
    /// profile. A no-op update retains the existing native trust state.
    pub fn update(&self, existing: &ProfileOwnership) -> Result<ProfileOwnership, ProfileError> {
        let path = self.path();
        if existing.canonical_path != path {
            return Err(ProfileError::OwnershipMismatch);
        }
        let rendered = self.rendered();
        let content_hash = hash(&rendered);
        if existing.profile_schema_version == OBSERVER_PROFILE_SCHEMA_VERSION
            && existing.hook_executable == self.hook_executable
            && existing.content_hash == content_hash
        {
            self.verify_owned_document(existing, &content_hash)?;
            return Ok(existing.clone());
        }

        let previous = Self::new(
            self.codex_home.clone(),
            existing.hook_executable.clone(),
            self.state_root.clone(),
        );
        let previous_declaration = previous.declaration_for(existing)?;
        let document = previous.read_owned_document(existing, &hash(&previous_declaration))?;
        let mut content = document.provider_prefix;
        content.push_str(&rendered);
        atomic_private_write(&path, content.as_bytes())?;
        Ok(ProfileOwnership {
            canonical_path: path,
            owner_id: existing.owner_id.clone(),
            profile_schema_version: OBSERVER_PROFILE_SCHEMA_VERSION,
            hook_executable: self.hook_executable.clone(),
            content_hash,
        })
    }

    /// Verifies the three-region profile contract: an optional validated
    /// provider prefix, the byte-exact `WSNav` declaration, and the narrow
    /// Codex-owned native trust suffix. Provider bytes remain opaque to `WSNav`.
    fn verify_owned_document(
        &self,
        ownership: &ProfileOwnership,
        expected_hash: &str,
    ) -> Result<(), ProfileError> {
        self.read_owned_document(ownership, expected_hash)
            .map(|_| ())
    }

    fn owned_native_suffix(
        &self,
        ownership: &ProfileOwnership,
        expected_hash: &str,
    ) -> Result<Option<toml::Table>, ProfileError> {
        self.read_owned_document(ownership, expected_hash)
            .map(|document| document.native_suffix)
    }

    fn read_owned_document(
        &self,
        ownership: &ProfileOwnership,
        expected_hash: &str,
    ) -> Result<OwnedDocument, ProfileError> {
        let path = self.path();
        if ownership.canonical_path != path || ownership.content_hash != expected_hash {
            return Err(ProfileError::OwnershipMismatch);
        }
        let content = fs::read(&path).map_err(ProfileError::Io)?;
        let content =
            std::str::from_utf8(&content).map_err(|_| ProfileError::ModifiedPath(path.clone()))?;
        let (provider_prefix, declaration_and_suffix) = split_provider_prefix(content, &path)?;
        let declared = self.declaration_for(ownership)?;
        let Some(native_suffix) = declaration_and_suffix.strip_prefix(&declared) else {
            return Err(ProfileError::ModifiedPath(path));
        };
        if native_suffix.is_empty() {
            return Ok(OwnedDocument {
                provider_prefix: provider_prefix.to_owned(),
                native_suffix: None,
            });
        }
        let suffix = native_suffix
            .parse::<toml::Table>()
            .map_err(|_| ProfileError::ModifiedPath(self.path()))?;
        if accepts_native_trust_suffix(&suffix, &self.path()) {
            Ok(OwnedDocument {
                provider_prefix: provider_prefix.to_owned(),
                native_suffix: Some(suffix),
            })
        } else {
            Err(ProfileError::ModifiedPath(self.path()))
        }
    }

    fn declaration_for(&self, ownership: &ProfileOwnership) -> Result<String, ProfileError> {
        match ownership.profile_schema_version {
            1 => Ok(legacy_rendered(&ownership.hook_executable)),
            OBSERVER_PROFILE_SCHEMA_VERSION => {
                if ownership.hook_executable != self.hook_executable {
                    return Err(ProfileError::OwnershipMismatch);
                }
                Ok(self.rendered())
            }
            _ => Err(ProfileError::OwnershipMismatch),
        }
    }
}

/// The validated three-region profile split. The provider prefix remains raw
/// bytes; `WSNav` owns only its exact declaration and validates native trust.
struct OwnedDocument {
    provider_prefix: String,
    native_suffix: Option<toml::Table>,
}

fn split_provider_prefix<'a>(
    content: &'a str,
    path: &Path,
) -> Result<(&'a str, &'a str), ProfileError> {
    let mut markers = content.match_indices(PROFILE_MARKER);
    let Some((marker_index, _)) = markers.next() else {
        return Err(ProfileError::ModifiedPath(path.to_owned()));
    };
    if markers.next().is_some() || (marker_index > 0 && !content[..marker_index].ends_with('\n')) {
        return Err(ProfileError::ModifiedPath(path.to_owned()));
    }
    let provider_prefix = &content[..marker_index];
    if provider_prefix.len() > MAX_PROVIDER_PREFIX_BYTES {
        return Err(ProfileError::ModifiedPath(path.to_owned()));
    }
    validate_provider_prefix(provider_prefix, path)?;
    Ok((provider_prefix, &content[marker_index..]))
}

fn validate_provider_prefix(prefix: &str, path: &Path) -> Result<(), ProfileError> {
    if prefix.is_empty() {
        return Ok(());
    }
    let table = prefix
        .parse::<toml::Table>()
        .map_err(|_| ProfileError::ModifiedPath(path.to_owned()))?;
    if table.is_empty()
        || table
            .keys()
            .any(|key| !PROVIDER_SETTINGS.contains(&key.as_str()))
    {
        return Err(ProfileError::ModifiedPath(path.to_owned()));
    }
    if table.values().any(|value| {
        value
            .as_str()
            .is_none_or(|value| value.is_empty() || value.len() > MAX_PROVIDER_SCALAR_BYTES)
    }) {
        return Err(ProfileError::ModifiedPath(path.to_owned()));
    }
    Ok(())
}

fn legacy_rendered(executable: &Path) -> String {
    let command = legacy_hook_command(executable);
    format!(
        "{PROFILE_MARKER}[features]\nhooks = true\n\n[[hooks.SessionStart]]\nmatcher = \"startup|resume|clear|compact\"\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n\n[[hooks.UserPromptSubmit]]\n[[hooks.UserPromptSubmit.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n\n[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n\n[[hooks.SessionEnd]]\nmatcher = \"other\"\n[[hooks.SessionEnd.hooks]]\ntype = \"command\"\ncommand = {command}\ntimeout = 3\n"
    )
}

fn hook_command(executable: &Path, state_root: &Path) -> String {
    let executable = shell_quote_argument(executable);
    let state_root = shell_quote_argument(state_root);
    toml_string(&format!("{executable} --state-root {state_root} _hook"))
}

fn legacy_hook_command(executable: &Path) -> String {
    let executable = shell_quote_argument(executable);
    toml_string(&format!("{executable} _hook"))
}

fn shell_quote_argument(value: &Path) -> String {
    let escaped = value.to_string_lossy().replace('\'', "'\\\"'\\\"'");
    format!("'{escaped}'")
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("a Rust string always serializes as JSON")
}

fn hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// Accept only the narrow native state Codex appends after a `/hooks` review.
/// The generated hook declaration before this suffix must remain byte exact.
fn accepts_native_trust_suffix(suffix: &toml::Table, profile_path: &Path) -> bool {
    if suffix.is_empty() || suffix.keys().any(|key| key != "hooks" && key != "projects") {
        return false;
    }

    let hooks_valid = suffix
        .get("hooks")
        .is_none_or(|value| accepts_hook_state(value, profile_path));
    let projects_valid = suffix.get("projects").is_none_or(accepts_project_trust);
    hooks_valid && projects_valid
}

fn accepts_hook_state(value: &toml::Value, profile_path: &Path) -> bool {
    let Some(hooks) = value.as_table() else {
        return false;
    };
    if hooks.len() != 1 {
        return false;
    }
    let Some(state) = hooks.get("state").and_then(toml::Value::as_table) else {
        return false;
    };
    if state.is_empty() {
        return false;
    }

    let expected_prefix = format!("{}:", profile_path.display());
    state.iter().all(|(key, record)| {
        let Some(hook) = key.strip_prefix(&expected_prefix) else {
            return false;
        };
        matches!(
            hook,
            "session_start:0:0" | "user_prompt_submit:0:0" | "stop:0:0" | "session_end:0:0"
        ) && record.as_table().is_some_and(|table| {
            table.len() == 1
                && table
                    .get("trusted_hash")
                    .and_then(toml::Value::as_str)
                    .is_some_and(is_sha256)
        })
    })
}

fn has_complete_hook_trust(suffix: &toml::Table, profile_path: &Path) -> bool {
    let Some(state) = suffix
        .get("hooks")
        .and_then(toml::Value::as_table)
        .and_then(|hooks| hooks.get("state"))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    let prefix = format!("{}:", profile_path.display());
    let expected = [
        "session_start:0:0",
        "user_prompt_submit:0:0",
        "stop:0:0",
        "session_end:0:0",
    ];
    state.len() == expected.len()
        && expected.iter().all(|entry| {
            state
                .get(&format!("{prefix}{entry}"))
                .and_then(toml::Value::as_table)
                .and_then(|record| record.get("trusted_hash"))
                .and_then(toml::Value::as_str)
                .is_some_and(is_sha256)
        })
}

fn accepts_project_trust(value: &toml::Value) -> bool {
    let Some(projects) = value.as_table() else {
        return false;
    };
    !projects.is_empty()
        && projects.iter().all(|(project_path, record)| {
            !project_path.is_empty()
                && record.as_table().is_some_and(|table| {
                    table.len() == 1
                        && table.get("trust_level").and_then(toml::Value::as_str) == Some("trusted")
                })
        })
}

fn is_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
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
    #[error("native observer-hook trust is incomplete or absent")]
    NativeTrustPending,
    #[error("observer profile needs an explicit update before it can run")]
    UpdateRequired,
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn manager(root: &Path) -> ObserverProfile {
        ObserverProfile::new(
            root.join("codex-home"),
            root.join("bin/wsnav"),
            root.join("state"),
        )
    }

    fn complete_native_hook_suffix(manager: &ObserverProfile) -> String {
        let mut suffix = String::from("\n[hooks.state]\n");
        for hook in ["session_start", "user_prompt_submit", "stop", "session_end"] {
            let key = toml_string(&format!("{}:{hook}:0:0", manager.path().display()));
            write!(
                suffix,
                "\n[hooks.state.{key}]\ntrusted_hash = \"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n"
            )
            .expect("writing to a string cannot fail");
        }
        suffix
    }

    fn write_owned_document(
        manager: &ObserverProfile,
        prefix: &str,
        suffix: &str,
    ) -> ProfileOwnership {
        let ownership = manager.install("owner".to_owned(), None).unwrap();
        fs::write(
            manager.path(),
            format!("{prefix}{}{suffix}", manager.rendered()),
        )
        .unwrap();
        ownership
    }

    #[test]
    fn profile_is_scoped_to_passive_lifecycle_hooks() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let rendered = manager.rendered();

        assert!(rendered.contains("[features]\nhooks = true"));
        assert!(rendered.contains("hooks.SessionStart"));
        assert!(rendered.contains("hooks.UserPromptSubmit"));
        assert!(rendered.contains("hooks.Stop"));
        assert!(rendered.contains("hooks.SessionEnd"));
        assert!(rendered.contains("--state-root"));
        assert!(rendered.contains(&format!("'{}'", manager.state_root.display())));
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

    #[test]
    fn native_hook_trust_suffix_is_preserved_as_provider_owned_state() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let ownership = manager.install("owner".to_owned(), None).unwrap();
        let profile_key = toml_string(&format!("{}:stop:0:0", manager.path().display()));
        let project_key = toml_string(&temporary.path().join("project").display().to_string());
        let native_suffix = format!(
            "\n[hooks.state]\n\n[hooks.state.{profile_key}]\ntrusted_hash = \"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n\n[projects.{project_key}]\ntrust_level = \"trusted\"\n"
        );
        fs::write(
            manager.path(),
            format!("{}{native_suffix}", manager.rendered()),
        )
        .unwrap();

        assert_eq!(
            manager
                .install("different-owner".to_owned(), Some(&ownership))
                .unwrap(),
            ownership
        );
        manager.remove(&ownership).unwrap();
        assert!(!manager.path().exists());
    }

    #[test]
    fn native_trust_requires_every_generated_lifecycle_hook() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let ownership = manager.install("owner".to_owned(), None).unwrap();
        assert!(matches!(
            manager.verify_native_trust(&ownership),
            Err(ProfileError::NativeTrustPending)
        ));
        fs::write(
            manager.path(),
            format!(
                "{}{}",
                manager.rendered(),
                complete_native_hook_suffix(&manager)
            ),
        )
        .unwrap();

        manager.verify_native_trust(&ownership).unwrap();
    }

    #[test]
    fn provider_prefix_is_preserved_for_trust_noop_update_and_remove() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let prefix =
            "# provider-owned\nmodel = \"gpt-5.6-luna\"\nmodel_reasoning_effort = \"medium\"\n";
        let suffix = complete_native_hook_suffix(&manager);
        let ownership = write_owned_document(&manager, prefix, &suffix);
        let original = fs::read_to_string(manager.path()).unwrap();

        assert_eq!(
            manager
                .install("different-owner".to_owned(), Some(&ownership))
                .unwrap(),
            ownership
        );
        manager.verify_native_trust(&ownership).unwrap();
        assert_eq!(manager.update(&ownership).unwrap(), ownership);
        assert_eq!(fs::read_to_string(manager.path()).unwrap(), original);

        manager.remove(&ownership).unwrap();
        assert_eq!(fs::read_to_string(manager.path()).unwrap(), prefix);
        assert!(matches!(
            manager.install("new-owner".to_owned(), None),
            Err(ProfileError::ForeignPath(_))
        ));
    }

    #[test]
    fn reasoning_effort_only_provider_prefix_is_accepted_and_preserved() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let prefix = "model_reasoning_effort = \"high\"\n";
        let suffix = complete_native_hook_suffix(&manager);
        let ownership = write_owned_document(&manager, prefix, &suffix);
        let original = fs::read_to_string(manager.path()).unwrap();

        manager.verify_native_trust(&ownership).unwrap();
        assert_eq!(manager.update(&ownership).unwrap(), ownership);
        assert_eq!(fs::read_to_string(manager.path()).unwrap(), original);

        manager.remove(&ownership).unwrap();
        assert_eq!(fs::read_to_string(manager.path()).unwrap(), prefix);
    }

    #[test]
    fn provider_prefix_is_preserved_when_declaration_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let prefix = "model = \"gpt-5.6-luna\"\n";
        let suffix = complete_native_hook_suffix(&manager);
        let ownership = write_owned_document(&manager, prefix, &suffix);
        let updated_manager = ObserverProfile::new(
            temporary.path().join("codex-home"),
            temporary.path().join("bin/wsnav-next"),
            temporary.path().join("state"),
        );

        let updated = updated_manager.update(&ownership).unwrap();
        assert_eq!(
            fs::read_to_string(updated_manager.path()).unwrap(),
            format!("{prefix}{}", updated_manager.rendered())
        );
        assert!(matches!(
            updated_manager.verify_native_trust(&updated),
            Err(ProfileError::NativeTrustPending)
        ));
    }

    #[test]
    fn provider_prefix_validation_fails_closed() {
        let invalid_prefixes = [
            "# provider comment\n",
            "model = \"\"\n",
            "model = 1\n",
            "model = \"first\"\nmodel = \"second\"\n",
            "temperature = \"high\"\n",
            "[model]\nvalue = \"nested\"\n",
            "model =\n",
        ];
        for prefix in invalid_prefixes {
            let temporary = tempfile::tempdir().unwrap();
            let profile = manager(temporary.path());
            let ownership = write_owned_document(&profile, prefix, "");
            assert!(matches!(
                profile.verify_native_trust(&ownership),
                Err(ProfileError::ModifiedPath(_))
            ));
        }

        let temporary = tempfile::tempdir().unwrap();
        let profile = manager(temporary.path());
        let overlong = format!(
            "model = \"{}\"\n",
            "x".repeat(MAX_PROVIDER_SCALAR_BYTES + 1)
        );
        let ownership = write_owned_document(&profile, &overlong, "");
        assert!(matches!(
            profile.verify_native_trust(&ownership),
            Err(ProfileError::ModifiedPath(_))
        ));

        let temporary = tempfile::tempdir().unwrap();
        let profile = manager(temporary.path());
        let oversized = format!(
            "model = \"x\"\n#{}\n",
            "x".repeat(MAX_PROVIDER_PREFIX_BYTES)
        );
        let ownership = write_owned_document(&profile, &oversized, "");
        assert!(matches!(
            profile.verify_native_trust(&ownership),
            Err(ProfileError::ModifiedPath(_))
        ));

        let temporary = tempfile::tempdir().unwrap();
        let profile = manager(temporary.path());
        let ownership = profile.install("owner".to_owned(), None).unwrap();
        fs::write(
            profile.path(),
            format!("{}model = \"misplaced\"\n", profile.rendered()),
        )
        .unwrap();
        assert!(matches!(
            profile.verify_native_trust(&ownership),
            Err(ProfileError::ModifiedPath(_))
        ));

        let temporary = tempfile::tempdir().unwrap();
        let profile = manager(temporary.path());
        let ownership = profile.install("owner".to_owned(), None).unwrap();
        fs::write(
            profile.path(),
            format!("{PROFILE_MARKER}{}", profile.rendered()),
        )
        .unwrap();
        assert!(matches!(
            profile.verify_native_trust(&ownership),
            Err(ProfileError::ModifiedPath(_))
        ));
    }

    #[test]
    fn provider_prefix_requires_a_line_anchored_managed_marker() {
        for prefix in [
            "model = \"gpt-5.6-luna\"",
            "model = \"gpt-5.6-luna\" # provider comment ",
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let profile = manager(temporary.path());
            let ownership = write_owned_document(&profile, prefix, "");
            assert!(matches!(
                profile.verify_native_trust(&ownership),
                Err(ProfileError::ModifiedPath(_))
            ));
        }
    }

    #[test]
    fn declaration_update_discards_old_native_trust_and_requires_review_again() {
        let temporary = tempfile::tempdir().unwrap();
        let original = manager(temporary.path());
        let ownership = original.install("owner".to_owned(), None).unwrap();
        fs::write(
            original.path(),
            format!(
                "{}{}",
                original.rendered(),
                complete_native_hook_suffix(&original)
            ),
        )
        .unwrap();
        original.verify_native_trust(&ownership).unwrap();

        let updated_manager = ObserverProfile::new(
            temporary.path().join("codex-home"),
            temporary.path().join("bin/wsnav-next"),
            temporary.path().join("state"),
        );
        let updated = updated_manager.update(&ownership).unwrap();

        assert_ne!(updated, ownership);
        assert_eq!(
            fs::read_to_string(updated_manager.path()).unwrap(),
            updated_manager.rendered()
        );
        assert!(matches!(
            updated_manager.verify_native_trust(&updated),
            Err(ProfileError::NativeTrustPending)
        ));
    }

    #[test]
    fn legacy_profile_requires_an_explicit_update_then_native_review() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        fs::create_dir_all(manager.path().parent().unwrap()).unwrap();
        let legacy = ProfileOwnership {
            canonical_path: manager.path(),
            owner_id: "owner".to_owned(),
            profile_schema_version: 1,
            hook_executable: temporary.path().join("bin/wsnav"),
            content_hash: hash(&legacy_rendered(&temporary.path().join("bin/wsnav"))),
        };
        fs::write(manager.path(), legacy_rendered(&legacy.hook_executable)).unwrap();

        assert!(matches!(
            manager.install("owner".to_owned(), Some(&legacy)),
            Err(ProfileError::UpdateRequired)
        ));

        let updated = manager.update(&legacy).unwrap();
        assert_eq!(
            updated.profile_schema_version,
            OBSERVER_PROFILE_SCHEMA_VERSION
        );
        assert_eq!(
            fs::read_to_string(manager.path()).unwrap(),
            manager.rendered()
        );
        assert!(matches!(
            manager.verify_native_trust(&updated),
            Err(ProfileError::NativeTrustPending)
        ));
    }

    #[test]
    fn exact_legacy_profile_can_still_be_removed_without_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        fs::create_dir_all(manager.path().parent().unwrap()).unwrap();
        let legacy = ProfileOwnership {
            canonical_path: manager.path(),
            owner_id: "owner".to_owned(),
            profile_schema_version: 1,
            hook_executable: temporary.path().join("bin/wsnav"),
            content_hash: hash(&legacy_rendered(&temporary.path().join("bin/wsnav"))),
        };
        fs::write(manager.path(), legacy_rendered(&legacy.hook_executable)).unwrap();

        manager.remove(&legacy).unwrap();
        assert!(!manager.path().exists());
    }

    #[test]
    fn non_native_suffix_remains_a_hard_ownership_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = manager(temporary.path());
        let ownership = manager.install("owner".to_owned(), None).unwrap();
        fs::write(
            manager.path(),
            format!("{}\n[model]\nname = \"foreign\"\n", manager.rendered()),
        )
        .unwrap();

        assert!(matches!(
            manager.remove(&ownership),
            Err(ProfileError::ModifiedPath(_))
        ));
    }
}
