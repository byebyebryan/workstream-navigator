//! Narrow, shell-free Git worktree operations for managed Workstreams.
//!
//! This module deliberately knows nothing about provider conversations or
//! `SQLite`. Callers supply a durably recorded plan and use the returned bounded
//! evidence to decide whether a Git effect can be committed or requires
//! recovery. It never removes a worktree, a branch, or an existing path.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use thiserror::Error;

const MAX_GIT_OUTPUT_BYTES: usize = 64 * 1024;

/// One exact managed Git worktree effect prepared by the host registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedWorktree {
    pub repository: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

impl ManagedWorktree {
    /// Validates the bounded, non-shell worktree fields before an external Git
    /// command can be launched.
    ///
    /// # Errors
    ///
    /// Returns an error when the branch or commit is unsafe for direct command
    /// arguments, or when the path fields are empty.
    pub fn validate(&self) -> Result<(), WorktreeError> {
        if self.repository.as_os_str().is_empty() || self.path.as_os_str().is_empty() {
            return Err(WorktreeError::InvalidPlan);
        }
        if !is_branch_name(&self.branch) || !is_commit_id(&self.base_commit) {
            return Err(WorktreeError::InvalidPlan);
        }
        Ok(())
    }
}

/// Bounded evidence about the exact target path of one managed worktree plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeEvidence {
    Absent,
    Exact,
    Mismatch,
}

/// Git operations needed for D4 creation and lost-response reconciliation.
pub trait GitWorktree {
    /// Resolves one configured Git ref to an exact locally available commit.
    ///
    /// # Errors
    ///
    /// Returns an error when the ref is unsafe, locally unavailable, or Git
    /// cannot provide one bounded exact commit identifier.
    fn resolve_commit(&self, repository: &Path, reference: &str) -> Result<String, WorktreeError>;

    /// Creates an exact managed worktree with a fresh implementation-owned branch.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is unsafe, the target branch/path already
    /// exists, or Git rejects the requested effect.
    fn create(&self, worktree: &ManagedWorktree) -> Result<(), WorktreeError>;

    /// Inspects only the exact planned path/branch/commit relationship.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot provide bounded worktree evidence.
    fn evidence(&self, worktree: &ManagedWorktree) -> Result<WorktreeEvidence, WorktreeError>;
}

/// System Git implementation using fixed direct argument vectors only.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGitWorktree;

impl GitWorktree for SystemGitWorktree {
    fn resolve_commit(&self, repository: &Path, reference: &str) -> Result<String, WorktreeError> {
        if repository.as_os_str().is_empty() || !is_reference(reference) {
            return Err(WorktreeError::InvalidPlan);
        }
        let output = run_git(
            repository,
            [
                OsString::from("rev-parse"),
                OsString::from("--verify"),
                OsString::from(format!("{reference}^{{commit}}")),
            ],
        )?;
        if !output.status.success() {
            return Err(WorktreeError::ReferenceUnavailable);
        }
        let value = single_line(&output.stdout).ok_or(WorktreeError::InvalidGitOutput)?;
        if !is_commit_id(value) {
            return Err(WorktreeError::InvalidGitOutput);
        }
        Ok(value.to_owned())
    }

    fn create(&self, worktree: &ManagedWorktree) -> Result<(), WorktreeError> {
        worktree.validate()?;
        if worktree.path.exists() || branch_exists(&worktree.repository, &worktree.branch)? {
            return Err(WorktreeError::TargetAlreadyExists);
        }
        let parent = worktree.path.parent().ok_or(WorktreeError::InvalidPlan)?;
        fs::create_dir_all(parent).map_err(WorktreeError::CreateParent)?;
        let output = run_git(
            &worktree.repository,
            [
                OsString::from("worktree"),
                OsString::from("add"),
                OsString::from("-b"),
                OsString::from(&worktree.branch),
                worktree.path.as_os_str().to_owned(),
                OsString::from(&worktree.base_commit),
            ],
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(WorktreeError::CreateRejected)
        }
    }

    fn evidence(&self, worktree: &ManagedWorktree) -> Result<WorktreeEvidence, WorktreeError> {
        worktree.validate()?;
        if !worktree.path.exists() {
            return Ok(WorktreeEvidence::Absent);
        }
        let output = run_git(
            &worktree.repository,
            [
                OsString::from("worktree"),
                OsString::from("list"),
                OsString::from("--porcelain"),
            ],
        )?;
        if !output.status.success() {
            return Err(WorktreeError::InspectRejected);
        }
        match worktree_evidence_from_porcelain(&output.stdout, worktree)? {
            WorktreeEvidence::Absent => Ok(WorktreeEvidence::Mismatch),
            evidence => Ok(evidence),
        }
    }
}

fn branch_exists(repository: &Path, branch: &str) -> Result<bool, WorktreeError> {
    let output = run_git(
        repository,
        [
            OsString::from("show-ref"),
            OsString::from("--verify"),
            OsString::from("--quiet"),
            OsString::from(format!("refs/heads/{branch}")),
        ],
    )?;
    Ok(output.status.success())
}

fn run_git(
    repository: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Output, WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(WorktreeError::Launch)?;
    if output.stdout.len() > MAX_GIT_OUTPUT_BYTES || output.stderr.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(WorktreeError::OutputTooLarge);
    }
    Ok(output)
}

fn worktree_evidence_from_porcelain(
    output: &[u8],
    expected: &ManagedWorktree,
) -> Result<WorktreeEvidence, WorktreeError> {
    let output = std::str::from_utf8(output).map_err(|_| WorktreeError::InvalidGitOutput)?;
    let expected_path = expected
        .path
        .canonicalize()
        .unwrap_or_else(|_| expected.path.clone());
    let expected_branch = format!("refs/heads/{}", expected.branch);
    let mut matching_path = 0_usize;
    let mut exact = false;

    for record in output
        .split("\n\n")
        .filter(|record| !record.trim().is_empty())
    {
        let mut path = None;
        let mut head = None;
        let mut branch = None;
        for line in record.lines() {
            if let Some(value) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(value));
            } else if let Some(value) = line.strip_prefix("HEAD ") {
                head = Some(value);
            } else if let Some(value) = line.strip_prefix("branch ") {
                branch = Some(value);
            }
        }
        let Some(path) = path else {
            return Err(WorktreeError::InvalidGitOutput);
        };
        let actual_path = path.canonicalize().unwrap_or(path);
        if actual_path != expected_path {
            continue;
        }
        matching_path = matching_path.saturating_add(1);
        if head == Some(expected.base_commit.as_str()) && branch == Some(expected_branch.as_str()) {
            exact = true;
        }
    }

    Ok(if matching_path == 0 {
        WorktreeEvidence::Absent
    } else if matching_path == 1 && exact {
        WorktreeEvidence::Exact
    } else {
        WorktreeEvidence::Mismatch
    })
}

fn single_line(output: &[u8]) -> Option<&str> {
    let value = std::str::from_utf8(output).ok()?.strip_suffix('\n')?;
    (!value.is_empty() && !value.contains(['\n', '\r'])).then_some(value)
}

fn is_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.contains(['\0', '\n', '\r'])
        && !value.starts_with('-')
}

fn is_branch_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.starts_with("wsnav/")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/-_.".contains(&byte))
        && !value.contains("..")
        && !value.ends_with(['.', '/'])
}

fn is_commit_id(value: &str) -> bool {
    (40..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Git worktree failures intentionally discard raw Git diagnostics and paths.
#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("managed worktree creation was rejected")]
    CreateRejected,
    #[error("could not create the managed worktree parent")]
    CreateParent(std::io::Error),
    #[error("managed worktree evidence does not match the recorded plan")]
    EvidenceMismatch,
    #[error("Git returned invalid bounded worktree metadata")]
    InvalidGitOutput,
    #[error("managed worktree plan is invalid")]
    InvalidPlan,
    #[error("could not inspect or launch Git")]
    Launch(std::io::Error),
    #[error("Git output exceeded its bound")]
    OutputTooLarge,
    #[error("configured Git base is not locally available")]
    ReferenceUnavailable,
    #[error("Git rejected managed worktree inspection")]
    InspectRejected,
    #[error("managed worktree path or branch already exists")]
    TargetAlreadyExists,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> ManagedWorktree {
        ManagedWorktree {
            repository: PathBuf::from("/repository"),
            path: PathBuf::from("/managed/worktree"),
            branch: "wsnav/workstream/00000000-0000-0000-0000-000000000001".to_owned(),
            base_commit: "a".repeat(40),
        }
    }

    #[test]
    fn managed_plan_accepts_only_owned_branch_and_commit_shapes() {
        assert!(worktree().validate().is_ok());
        let mut invalid = worktree();
        invalid.branch = "main".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(WorktreeError::InvalidPlan)
        ));
        invalid = worktree();
        invalid.base_commit = "HEAD".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(WorktreeError::InvalidPlan)
        ));
    }

    #[test]
    fn porcelain_requires_exact_path_branch_and_commit() {
        let expected = worktree();
        let output = format!(
            "worktree /managed/worktree\nHEAD {}\nbranch refs/heads/{}\n\n",
            expected.base_commit, expected.branch
        );
        assert_eq!(
            worktree_evidence_from_porcelain(output.as_bytes(), &expected).unwrap(),
            WorktreeEvidence::Exact
        );
    }

    #[test]
    fn porcelain_does_not_adopt_a_mismatched_existing_path() {
        let expected = worktree();
        let output = format!(
            "worktree /managed/worktree\nHEAD {}\nbranch refs/heads/main\n\n",
            expected.base_commit
        );
        assert_eq!(
            worktree_evidence_from_porcelain(output.as_bytes(), &expected).unwrap(),
            WorktreeEvidence::Mismatch
        );
    }

    #[test]
    fn porcelain_reports_absence_without_guessing() {
        let expected = worktree();
        let output = format!(
            "worktree /repository\nHEAD {}\nbranch refs/heads/main\n\n",
            expected.base_commit
        );
        assert_eq!(
            worktree_evidence_from_porcelain(output.as_bytes(), &expected).unwrap(),
            WorktreeEvidence::Absent
        );
    }
}
