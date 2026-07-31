//! Bounded, read-only Git repository registration metadata.
//!
//! A selected checkout, its shared repository command path, and its optional
//! cross-host presentation identity are deliberately separate. Remote URLs are
//! normalized in memory and discarded; only a versioned SHA-256 fingerprint is
//! returned to callers.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::process::{BoundedProcessError, output_bounded};
use crate::state::{HostRegistry, StateError};

const MAX_GIT_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_REMOTES: usize = 32;
const MAX_REMOTE_URL_BYTES: usize = 4096;
const FINGERPRINT_VERSION: &str = "git-remote-v1";

/// Exact host-private metadata for one external Workstream registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryRegistration {
    /// Root of the exact external worktree selected by the operator.
    pub checkout_path: PathBuf,
    /// Primary worktree used as the stable project-level Git command anchor.
    pub repository_path: PathBuf,
    /// Absolute common Git directory. This never crosses the host protocol.
    pub repository_identity: String,
    /// Exact commit selected as the location's default managed-worktree base.
    pub default_base_ref: String,
    /// Bounded label derived from the primary worktree basename.
    pub display_name: String,
    /// Opaque identity of one unambiguous canonical fetch remote.
    pub remote_identity_fingerprint: Option<String>,
}

/// Inspects a local non-bare Git checkout without contacting a network.
///
/// `checkout` may name the worktree root or any directory below it. The
/// selected worktree remains the external Workstream checkout, while the first
/// non-bare entry from `git worktree list --porcelain` becomes the project-level
/// command path.
///
/// # Errors
///
/// Returns an error when Git cannot provide bounded, unambiguous local
/// metadata or the selected checkout is not a usable non-bare worktree.
pub fn inspect(checkout: &Path) -> Result<RepositoryRegistration, RepositoryError> {
    let checkout = checkout
        .canonicalize()
        .map_err(RepositoryError::Canonicalize)?;
    let checkout_path = PathBuf::from(git_single_line(
        &checkout,
        ["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )?);
    let checkout_path = checkout_path
        .canonicalize()
        .map_err(RepositoryError::Canonicalize)?;
    let common_dir = PathBuf::from(git_single_line(
        &checkout_path,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?);
    let common_dir = common_dir
        .canonicalize()
        .map_err(RepositoryError::Canonicalize)?;
    let worktrees = run_git(
        &checkout_path,
        [
            OsString::from("worktree"),
            OsString::from("list"),
            OsString::from("--porcelain"),
        ],
    )?;
    if !worktrees.status.success() {
        return Err(RepositoryError::GitRejected);
    }
    let repository_path = primary_worktree(&worktrees.stdout)?
        .canonicalize()
        .map_err(RepositoryError::Canonicalize)?;
    let default_base_ref = git_single_line(&checkout_path, ["rev-parse", "HEAD"])?;
    if !(40..=128).contains(&default_base_ref.len())
        || !default_base_ref
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RepositoryError::InvalidGitOutput);
    }
    let display_name = repository_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .ok_or(RepositoryError::InvalidGitOutput)?
        .chars()
        .take(64)
        .collect::<String>();
    let remote_identity_fingerprint = discover_remote_fingerprint(&checkout_path)?;

    Ok(RepositoryRegistration {
        checkout_path,
        repository_path,
        repository_identity: common_dir.to_string_lossy().into_owned(),
        default_base_ref,
        display_name,
        remote_identity_fingerprint,
    })
}

/// Completes the one-time D6.1 metadata migration for existing locations.
/// Repository inspection failure is a truthful no-fingerprint outcome rather
/// than a reason to hide otherwise usable Workstreams.
///
/// # Errors
///
/// Returns an error only when host registry state cannot be read or updated.
pub fn refresh_pending_metadata(registry: &mut HostRegistry) -> Result<(), StateError> {
    for pending in registry.pending_repository_metadata()? {
        if let Ok(metadata) = inspect(&pending.repository_path) {
            registry.record_repository_metadata(
                pending.location_id,
                &metadata.repository_path,
                &metadata.display_name,
                metadata.remote_identity_fingerprint.as_deref(),
            )?;
            continue;
        }
        let display_name = pending
            .repository_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("local project");
        registry.record_repository_metadata(
            pending.location_id,
            &pending.repository_path,
            display_name,
            None,
        )?;
    }
    Ok(())
}

fn discover_remote_fingerprint(repository: &Path) -> Result<Option<String>, RepositoryError> {
    let origin = remote_urls(repository, "origin")?;
    if !origin.is_empty() {
        return fingerprint_unambiguous(origin.iter().map(String::as_str));
    }

    let output = run_git(repository, [OsString::from("remote")])?;
    if !output.status.success() {
        return Err(RepositoryError::GitRejected);
    }
    let names = bounded_lines(&output.stdout)?;
    if names.len() > MAX_REMOTES {
        return Ok(None);
    }
    let mut urls = Vec::new();
    for name in names {
        if name == "origin" {
            continue;
        }
        urls.extend(remote_urls(repository, name)?);
    }
    fingerprint_unambiguous(urls.iter().map(String::as_str))
}

fn remote_urls(repository: &Path, remote: &str) -> Result<Vec<String>, RepositoryError> {
    if remote.is_empty() || remote.contains(['\0', '\n', '\r']) {
        return Err(RepositoryError::InvalidGitOutput);
    }
    let output = run_git(
        repository,
        [
            OsString::from("config"),
            OsString::from("--get-all"),
            OsString::from(format!("remote.{remote}.url")),
        ],
    )?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    bounded_lines(&output.stdout).map(|lines| lines.into_iter().map(str::to_owned).collect())
}

fn fingerprint_unambiguous<'a>(
    urls: impl IntoIterator<Item = &'a str>,
) -> Result<Option<String>, RepositoryError> {
    let mut identities = BTreeSet::new();
    for url in urls {
        let Some(identity) = normalize_remote_identity(url) else {
            return Ok(None);
        };
        identities.insert(identity);
    }
    if identities.len() != 1 {
        return Ok(None);
    }
    let identity = identities
        .into_iter()
        .next()
        .ok_or(RepositoryError::InvalidGitOutput)?;
    let mut hash = Sha256::new();
    hash.update(FINGERPRINT_VERSION.as_bytes());
    hash.update([0]);
    hash.update(identity.as_bytes());
    Ok(Some(format!(
        "{FINGERPRINT_VERSION}:{}",
        hex(&hash.finalize())
    )))
}

fn normalize_remote_identity(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_REMOTE_URL_BYTES
        || value.contains(['\0', '\n', '\r'])
        || value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
    {
        return None;
    }

    let (host, path) = if let Some((scheme, remainder)) = value.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "git" | "http" | "https" | "ssh") {
            return None;
        }
        let remainder = remainder.split(['?', '#']).next()?;
        let (authority, path) = remainder.split_once('/')?;
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let host = normalize_authority(authority, &scheme)?;
        (host, path)
    } else {
        let (authority, path) = value.split_once(':')?;
        if authority.contains('/')
            || authority.len() == 1 && authority.as_bytes()[0].is_ascii_alphabetic()
        {
            return None;
        }
        let authority = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if authority.is_empty() || authority.contains(['/', '\\']) {
            return None;
        }
        (authority.to_ascii_lowercase(), path)
    };

    let path = path
        .split(['?', '#'])
        .next()?
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or_else(|| {
            path.split(['?', '#'])
                .next()
                .unwrap_or_default()
                .trim_matches('/')
        });
    if host.is_empty()
        || path.is_empty()
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return None;
    }
    let path = if host == "github.com" {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    };
    Some(format!("{host}/{path}"))
}

fn normalize_authority(authority: &str, scheme: &str) -> Option<String> {
    if authority.is_empty() || authority.contains(['/', '\\']) {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let closing = authority.find(']')?;
        let host = &authority[..=closing];
        let suffix = &authority[closing + 1..];
        let port = suffix.strip_prefix(':').filter(|port| !port.is_empty());
        if !suffix.is_empty() && port.is_none() {
            return None;
        }
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if port.bytes().all(|byte| byte.is_ascii_digit()) {
            (host, Some(port))
        } else {
            (authority, None)
        }
    } else {
        (authority, None)
    };
    if host.is_empty() {
        return None;
    }
    let default_port = matches!(
        (scheme, port),
        ("ssh", Some("22")) | ("http", Some("80")) | ("https", Some("443")) | ("git", Some("9418"))
    );
    Some(if port.is_none() || default_port {
        host.to_ascii_lowercase()
    } else {
        format!("{}:{}", host.to_ascii_lowercase(), port?)
    })
}

fn primary_worktree(output: &[u8]) -> Result<PathBuf, RepositoryError> {
    let output = std::str::from_utf8(output).map_err(|_| RepositoryError::InvalidGitOutput)?;
    let first = output
        .split("\n\n")
        .find(|record| !record.trim().is_empty())
        .ok_or(RepositoryError::InvalidGitOutput)?;
    if first.lines().any(|line| line == "bare") {
        return Err(RepositoryError::BareRepository);
    }
    let path = first
        .lines()
        .find_map(|line| line.strip_prefix("worktree "))
        .filter(|path| !path.is_empty() && !path.contains(['\0', '\n', '\r']))
        .ok_or(RepositoryError::InvalidGitOutput)?;
    Ok(PathBuf::from(path))
}

fn git_single_line(
    repository: &Path,
    arguments: impl IntoIterator<Item = &'static str>,
) -> Result<String, RepositoryError> {
    let output = run_git(repository, arguments.into_iter().map(OsString::from))?;
    if !output.status.success() {
        return Err(RepositoryError::GitRejected);
    }
    let lines = bounded_lines(&output.stdout)?;
    if lines.len() != 1 {
        return Err(RepositoryError::InvalidGitOutput);
    }
    Ok(lines[0].to_owned())
}

fn bounded_lines(output: &[u8]) -> Result<Vec<&str>, RepositoryError> {
    let output = std::str::from_utf8(output).map_err(|_| RepositoryError::InvalidGitOutput)?;
    if output.contains('\r') {
        return Err(RepositoryError::InvalidGitOutput);
    }
    Ok(output
        .strip_suffix('\n')
        .unwrap_or(output)
        .split('\n')
        .filter(|line| !line.is_empty())
        .collect())
}

fn run_git(
    repository: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Output, RepositoryError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repository).args(arguments);
    output_bounded(&mut command, MAX_GIT_OUTPUT_BYTES, MAX_GIT_OUTPUT_BYTES)
        .map_err(RepositoryError::Process)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}

/// Safe repository-inspection diagnostics that never include Git output or a
/// repository path.
#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("bare repositories cannot be registered as Workstreams")]
    BareRepository,
    #[error("could not resolve the Git checkout path")]
    Canonicalize(#[source] std::io::Error),
    #[error("Git rejected repository inspection")]
    GitRejected,
    #[error("Git returned invalid bounded repository metadata")]
    InvalidGitOutput,
    #[error("could not execute bounded Git repository inspection")]
    Process(#[source] BoundedProcessError),
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command};

    use super::*;

    #[test]
    fn transport_variants_have_one_remote_fingerprint() {
        let ssh = fingerprint_unambiguous(["git@GitHub.com:Owner/Cubey.git"]).unwrap();
        let https = fingerprint_unambiguous(["https://github.com/owner/cubey.git"]).unwrap();
        let ssh_url = fingerprint_unambiguous(["ssh://git@github.com:22/OWNER/CUBEY.git"]).unwrap();

        assert_eq!(ssh, https);
        assert_eq!(https, ssh_url);
    }

    #[test]
    fn credentials_queries_and_fragments_do_not_affect_identity() {
        let credentialed = fingerprint_unambiguous([
            "https://token:secret@github.com/owner/cubey.git?access_token=other#fragment",
        ])
        .unwrap();
        let clean = fingerprint_unambiguous(["https://github.com/owner/cubey"]).unwrap();

        assert_eq!(credentialed, clean);
    }

    #[test]
    fn local_and_ambiguous_remotes_do_not_produce_identity() {
        assert_eq!(fingerprint_unambiguous(["../cubey.git"]).unwrap(), None);
        assert_eq!(
            fingerprint_unambiguous(["file:///srv/cubey.git"]).unwrap(),
            None
        );
        assert_eq!(
            fingerprint_unambiguous([
                "git@github.com:owner/cubey.git",
                "git@github.com:owner/other.git",
            ])
            .unwrap(),
            None
        );
        assert_eq!(
            fingerprint_unambiguous(["git@github.com:owner/cubey.git", "../private-mirror.git",])
                .unwrap(),
            None
        );
    }

    #[test]
    fn linked_worktree_registration_separates_checkout_and_primary_repository() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("cubey");
        let linked = temporary.path().join("cubey-worktree1");
        fs::create_dir(&repository).unwrap();
        run(&repository, ["init", "-q"]);
        run(&repository, ["config", "user.name", "WSNav Test"]);
        run(
            &repository,
            ["config", "user.email", "wsnav@example.invalid"],
        );
        fs::write(repository.join("README.md"), "fixture\n").unwrap();
        run(&repository, ["add", "README.md"]);
        run(&repository, ["commit", "-qm", "fixture"]);
        run(
            &repository,
            ["remote", "add", "origin", "git@github.com:owner/cubey.git"],
        );
        run(
            &repository,
            [
                "worktree",
                "add",
                "-qb",
                "fixture-linked",
                linked.to_str().unwrap(),
            ],
        );
        fs::create_dir(linked.join("nested")).unwrap();

        let registration = inspect(&linked.join("nested")).unwrap();

        assert_eq!(registration.checkout_path, linked.canonicalize().unwrap());
        assert_eq!(
            registration.repository_path,
            repository.canonicalize().unwrap()
        );
        assert_eq!(registration.display_name, "cubey");
        assert!(registration.repository_identity.ends_with("cubey/.git"));
        assert!(registration.remote_identity_fingerprint.is_some());
    }

    fn run<'a>(repository: &Path, arguments: impl IntoIterator<Item = &'a str>) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
