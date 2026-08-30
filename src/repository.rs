//! Bounded, read-only Git repository discovery metadata.
//!
//! Current onboarding preserves the exact containing worktree root. Remote
//! URLs are normalized in memory and discarded; only a versioned SHA-256
//! fingerprint and credential-free canonical display label are returned to
//! callers.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::process::{BoundedProcessError, output_bounded};

const MAX_GIT_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_REMOTES: usize = 32;
const MAX_REMOTE_URL_BYTES: usize = 4096;
const MAX_REMOTE_DISPLAY_BYTES: usize = 256;
const FINGERPRINT_VERSION: &str = "git-remote-v1";

/// Exact host-private metadata discovered for one onboarding Location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryDiscovery {
    /// Canonical containing Git worktree used as the project root for every
    /// Navigator-launched provider session.
    pub project_root: PathBuf,
    /// Bounded label derived from the project-root basename.
    pub display_name: String,
    /// Opaque identity of one unambiguous canonical fetch remote.
    pub remote_identity_fingerprint: Option<String>,
    /// Credential-free normalized fetch-remote label for display only.
    pub remote_identity_display: Option<String>,
}

/// Resolves the exact non-bare worktree containing a shell's current
/// directory without registering it or contacting a remote.
///
/// Current promotion retains the linked worktree as its own immutable launch
/// Location; no worktree is created, removed, or adopted here.
///
/// # Errors
///
/// Returns an error when `checkout` cannot be proved to be inside one
/// canonical non-bare worktree. The command environment is stripped of the
/// Git path overrides that could otherwise redirect discovery away from the
/// shell's actual directory.
pub fn discover_containing_worktree(
    checkout: &Path,
) -> Result<RepositoryDiscovery, RepositoryError> {
    let checkout = checkout
        .canonicalize()
        .map_err(RepositoryError::Canonicalize)?;
    let bare = git_single_line_isolated(&checkout, ["rev-parse", "--is-bare-repository"])?;
    match bare.as_str() {
        "false" => {}
        "true" => return Err(RepositoryError::BareRepository),
        _ => return Err(RepositoryError::InvalidGitOutput),
    }
    let project_root = PathBuf::from(git_single_line_isolated(
        &checkout,
        ["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )?)
    .canonicalize()
    .map_err(RepositoryError::Canonicalize)?;
    if !checkout.starts_with(&project_root) {
        return Err(RepositoryError::InvalidGitOutput);
    }
    let display_name = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .ok_or(RepositoryError::InvalidGitOutput)?
        .chars()
        .take(64)
        .collect::<String>();
    let remote_identity = discover_remote_identity(&project_root)?;
    Ok(RepositoryDiscovery {
        project_root,
        display_name,
        remote_identity_fingerprint: remote_identity
            .as_ref()
            .map(|identity| identity.fingerprint.clone()),
        remote_identity_display: remote_identity.map(|identity| identity.display),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RemoteIdentity {
    fingerprint: String,
    display: String,
}

fn discover_remote_identity(repository: &Path) -> Result<Option<RemoteIdentity>, RepositoryError> {
    let origin = remote_urls(repository, "origin")?;
    if !origin.is_empty() {
        return remote_identity_unambiguous(origin.iter().map(String::as_str));
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
    remote_identity_unambiguous(urls.iter().map(String::as_str))
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

fn remote_identity_unambiguous<'a>(
    urls: impl IntoIterator<Item = &'a str>,
) -> Result<Option<RemoteIdentity>, RepositoryError> {
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
    let display = identities
        .into_iter()
        .next()
        .ok_or(RepositoryError::InvalidGitOutput)?;
    if display.len() > MAX_REMOTE_DISPLAY_BYTES {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    hash.update(FINGERPRINT_VERSION.as_bytes());
    hash.update([0]);
    hash.update(display.as_bytes());
    Ok(Some(RemoteIdentity {
        fingerprint: format!("{FINGERPRINT_VERSION}:{}", hex(&hash.finalize())),
        display,
    }))
}

fn normalize_remote_identity(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_REMOTE_URL_BYTES
        || value.chars().any(char::is_control)
        || value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
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
        || path.contains(['\\', '@'])
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

fn git_single_line_isolated(
    repository: &Path,
    arguments: impl IntoIterator<Item = &'static str>,
) -> Result<String, RepositoryError> {
    let output = run_git_isolated(repository, arguments.into_iter().map(OsString::from))?;
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

fn run_git_isolated(
    repository: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Output, RepositoryError> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CEILING_DIRECTORIES");
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
    use std::process::Command;

    use super::*;

    #[test]
    fn transport_variants_have_one_safe_remote_identity() {
        let ssh = remote_identity_unambiguous(["git@GitHub.com:Owner/Cubey.git"])
            .unwrap()
            .unwrap();
        let https = remote_identity_unambiguous(["https://github.com/owner/cubey.git"])
            .unwrap()
            .unwrap();
        let ssh_url = remote_identity_unambiguous(["ssh://git@github.com:22/OWNER/CUBEY.git"])
            .unwrap()
            .unwrap();

        assert_eq!(ssh, https);
        assert_eq!(https, ssh_url);
        assert_eq!(ssh.display, "github.com/owner/cubey");
    }

    #[test]
    fn credentials_queries_and_fragments_do_not_affect_or_leak_from_identity() {
        let credentialed = remote_identity_unambiguous([
            "https://token:secret@github.com/owner/cubey.git?access_token=other#fragment",
        ])
        .unwrap()
        .unwrap();
        let clean = remote_identity_unambiguous(["https://github.com/owner/cubey"])
            .unwrap()
            .unwrap();

        assert_eq!(credentialed, clean);
        assert_eq!(credentialed.display, "github.com/owner/cubey");
        assert!(!credentialed.display.contains("token"));
        assert!(!credentialed.display.contains("secret"));
        assert!(!credentialed.display.contains('?'));
        assert!(!credentialed.display.contains('#'));
    }

    #[test]
    fn local_and_ambiguous_remotes_do_not_produce_identity() {
        assert_eq!(remote_identity_unambiguous(["../cubey.git"]).unwrap(), None);
        assert_eq!(
            remote_identity_unambiguous(["file:///srv/cubey.git"]).unwrap(),
            None
        );
        assert_eq!(
            remote_identity_unambiguous(["file:/srv/cubey.git"]).unwrap(),
            None
        );
        assert_eq!(
            remote_identity_unambiguous([
                "git@github.com:owner/cubey.git",
                "git@github.com:owner/other.git",
            ])
            .unwrap(),
            None
        );
        assert_eq!(
            remote_identity_unambiguous([
                "git@github.com:owner/cubey.git",
                "../private-mirror.git",
            ])
            .unwrap(),
            None
        );
    }

    #[test]
    fn containing_worktree_rejects_a_bare_repository_without_registration() {
        let temporary = tempfile::tempdir().unwrap();
        let bare = temporary.path().join("bare.git");
        let status = Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&bare)
            .status()
            .unwrap();
        assert!(status.success());

        assert!(matches!(
            discover_containing_worktree(&bare),
            Err(RepositoryError::BareRepository)
        ));
    }
}
