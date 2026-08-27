//! Dormant D17 post-exec reconciliation boundary.
//!
//! The reconciler can only record exact native exec evidence. Codex may then
//! finish its provider-exec proof directly, while `OpenCode` remains action
//! fenced until the presentation controller has established its exact
//! detached observer; it never launches, signals, attaches, or otherwise
//! controls a provider.

#![allow(
    dead_code,
    reason = "the D17 reconciler remains unreachable until the atomic Navigator cutover"
)]

use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    domain::{OperationId, ProviderKind},
    provisional::{ProvisionalPhase, ProvisionalSlot, SlotError, read_marker, update_marker},
    runtime::{PrivateRuntime, ProcessGroupProbe},
    state::d16::{
        OnboardingProviderExecEvidence, OnboardingProviderExecTarget,
        OnboardingProviderExecutableIdentity,
    },
    state::{D16State, ProvisionalLease, StateError},
};

/// Exact native executable selected before the helper's final `execve`. Its
/// canonical path is private launch input; only its bounded device/inode
/// identity enters the onboarding journal for post-exec proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpectedProviderExecutable {
    provider: ProviderKind,
    canonical_path: PathBuf,
    identity: OnboardingProviderExecutableIdentity,
}

impl ExpectedProviderExecutable {
    pub(crate) fn new(provider: ProviderKind, path: &Path) -> Result<Self, ReconcileError> {
        let canonical_path =
            fs::canonicalize(path).map_err(|_| ReconcileError::ExecutableUnavailable)?;
        let metadata =
            fs::metadata(&canonical_path).map_err(|_| ReconcileError::ExecutableUnavailable)?;
        if !metadata.is_file() {
            return Err(ReconcileError::ExecutableUnavailable);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(ReconcileError::ExecutableUnavailable);
            }
        }
        let identity = identity_from_metadata(&metadata)?;
        Ok(Self {
            provider,
            canonical_path,
            identity,
        })
    }

    /// Resolves only the native executable fixed by the already selected
    /// provider. Every `PATH` component must be absolute so resolution cannot
    /// depend on an ambient helper cwd; no shell, provider, or process is
    /// launched while resolving it.
    pub(crate) fn resolve_from_path(
        provider: ProviderKind,
        search_path: &OsStr,
    ) -> Result<Self, ReconcileError> {
        let executable_name = match provider {
            ProviderKind::Codex => "codex",
            ProviderKind::OpenCode => "opencode",
        };
        for directory in std::env::split_paths(search_path) {
            if !directory.is_absolute() {
                return Err(ReconcileError::ExecutableUnavailable);
            }
            let candidate = directory.join(executable_name);
            match fs::symlink_metadata(&candidate) {
                Ok(_) => return Self::new(provider, &candidate),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(ReconcileError::ExecutableUnavailable),
            }
        }
        Err(ReconcileError::ExecutableUnavailable)
    }

    /// Builds the direct native argv from the already canonical executable and
    /// grammar-normalized fresh-TUI arguments. The original account shell is
    /// never involved in the final exec.
    #[must_use]
    pub(crate) fn native_program(&self, arguments: &[String]) -> Vec<OsString> {
        std::iter::once(self.canonical_path.clone().into_os_string())
            .chain(arguments.iter().map(OsString::from))
            .collect()
    }

    #[must_use]
    pub(crate) const fn identity(&self) -> OnboardingProviderExecutableIdentity {
        self.identity
    }
}

/// Read-only process-executable evidence. A missing process is distinct from
/// an inaccessible or malformed process and neither can prove provider exec.
pub(crate) trait ProviderExecutableProbe {
    fn executable_identity_for_pid(
        &self,
        pid: u32,
    ) -> Result<Option<OnboardingProviderExecutableIdentity>, ReconcileError>;
}

/// Linux read-only proof of the exact file currently executable by one PID.
/// The `/proc/<pid>/exe` metadata remains tied to a live executable even if
/// its original pathname later changes, so reconciliation never resolves an
/// ambient `PATH` or trusts a replacement file.
pub(crate) struct LinuxProviderExecutableProbe;

impl ProviderExecutableProbe for LinuxProviderExecutableProbe {
    fn executable_identity_for_pid(
        &self,
        pid: u32,
    ) -> Result<Option<OnboardingProviderExecutableIdentity>, ReconcileError> {
        if pid == 0 {
            return Err(ReconcileError::ProviderExecutableMismatch);
        }
        let path = PathBuf::from(format!("/proc/{pid}/exe"));
        match fs::metadata(path) {
            Ok(metadata) => identity_from_metadata(&metadata).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(ReconcileError::ProviderExecutableMismatch),
        }
    }
}

/// Bounded reconciliation errors. They intentionally never render private
/// paths, command lines, shell state, tokens, or provider output.
#[derive(Debug, Error)]
pub(crate) enum ReconcileError {
    #[error("D17 provisional slot evidence is unavailable")]
    Slot(#[from] SlotError),
    #[error("D17 provider-exec state is unavailable")]
    State(#[from] StateError),
    #[error("the expected native provider executable is unavailable")]
    ExecutableUnavailable,
    #[error("the exact provisional slot is not ready for provider-exec proof")]
    SlotNotReady,
    #[error("the provisional handoff identity is unavailable")]
    HandoffIdentityUnavailable,
    #[error("the provider identity does not match the reserved D17 Runtime")]
    ProviderIdentityMismatch,
    #[error("the native provider cwd does not match its registered D17 worktree root")]
    ProviderCwdMismatch,
    #[error("the provider executable does not match the expected native executable")]
    ProviderExecutableMismatch,
}

/// Records exact post-exec evidence for one already Runtime-owned provisional
/// slot. The caller supplies only a read-only executable probe; the expected
/// file identity is loaded from the durable preparation record. All marker,
/// pane/process-group, state revision, Runtime generation, cwd, and provider
/// checks are repeated here. Codex advances to final proof and repairs a
/// marker-write failure; `OpenCode` records only its exact process identity and
/// deliberately remains action-fenced for its detached observer controller.
pub(crate) fn prove_provider_exec(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    runtime: &PrivateRuntime<'_>,
    process_group_probe: &dyn ProcessGroupProbe,
    executable_probe: &dyn ProviderExecutableProbe,
) -> Result<(), ReconcileError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let slot = read_marker(state.root(), presentation_directory)?;
    let operation_id = slot
        .handoff_request()
        .map(OperationId::from)
        .ok_or(ReconcileError::HandoffIdentityUnavailable)?;
    if slot.phase() == ProvisionalPhase::ProviderExecProven {
        let target =
            state.d17_onboarding_exec_proven_target_current(provisional_lease, operation_id)?;
        validate_slot_target(&slot, &target)?;
        return Ok(());
    }
    if slot.phase() != ProvisionalPhase::RuntimeOwnedLaunching {
        return Err(ReconcileError::SlotNotReady);
    }
    match state.d17_onboarding_exec_proven_target_current(provisional_lease, operation_id) {
        Ok(target) => {
            validate_slot_target(&slot, &target)?;
            return complete_proven_marker(state, provisional_lease, presentation_directory, &slot);
        }
        Err(StateError::OnboardingOperationUnavailable) => {}
        Err(error) => return Err(error.into()),
    }
    let live = slot.revalidate_live_shell(runtime, process_group_probe)?;
    let target = state.d17_onboarding_exec_proof_target_current(provisional_lease, operation_id)?;
    validate_slot_target(&slot, &target)?;
    if target.project_root() != live.cwd {
        return Err(ReconcileError::ProviderCwdMismatch);
    }
    let actual = executable_probe
        .executable_identity_for_pid(live.shell_pid)?
        .ok_or(ReconcileError::ProviderExecutableMismatch)?;
    if actual != target.executable_identity() {
        return Err(ReconcileError::ProviderExecutableMismatch);
    }
    let evidence = OnboardingProviderExecEvidence::new(live.shell_pid, live.shell_birth)?;
    if target.provider() == ProviderKind::OpenCode {
        state.record_d17_provider_exec_observed_current(
            provisional_lease,
            target.ownership(),
            &evidence,
        )?;
        return Ok(());
    }
    state.record_d17_provider_exec_proven_current(
        provisional_lease,
        target.ownership(),
        &evidence,
    )?;
    complete_proven_marker(state, provisional_lease, presentation_directory, &slot)
}

fn validate_slot_target(
    slot: &ProvisionalSlot,
    target: &OnboardingProviderExecTarget,
) -> Result<(), ReconcileError> {
    if target.ownership().runtime_id != slot.candidate_runtime_id() {
        return Err(ReconcileError::ProviderIdentityMismatch);
    }
    Ok(())
}

fn identity_from_metadata(
    metadata: &fs::Metadata,
) -> Result<OnboardingProviderExecutableIdentity, ReconcileError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        OnboardingProviderExecutableIdentity::new(metadata.dev(), metadata.ino())
            .map_err(|_| ReconcileError::ExecutableUnavailable)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(ReconcileError::ExecutableUnavailable)
    }
}

fn complete_proven_marker(
    state: &D16State,
    provisional_lease: &ProvisionalLease,
    presentation_directory: &Path,
    slot: &ProvisionalSlot,
) -> Result<(), ReconcileError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let mut proven_slot = slot.clone();
    proven_slot.prove_provider_exec()?;
    update_marker(state.root(), presentation_directory, slot, &proven_slot)?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::{Path, PathBuf},
    };

    use super::{ExpectedProviderExecutable, ReconcileError};
    use crate::domain::ProviderKind;

    fn write_executable(directory: &Path, name: &str) -> PathBuf {
        let executable = directory.join(name);
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        }
        executable
    }

    #[test]
    fn provider_executable_resolution_uses_the_first_exact_provider_binary() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let expected = write_executable(&first, "codex");
        write_executable(&second, "codex");
        let search_path = env::join_paths([&first, &second]).unwrap();

        let resolved =
            ExpectedProviderExecutable::resolve_from_path(ProviderKind::Codex, &search_path)
                .unwrap();

        assert_eq!(resolved.canonical_path, expected.canonicalize().unwrap());
        assert_eq!(resolved.provider, ProviderKind::Codex);
        assert_eq!(
            resolved.native_program(&["--model".to_owned(), "gpt-5.6".to_owned()]),
            vec![
                expected.canonicalize().unwrap().into_os_string(),
                "--model".into(),
                "gpt-5.6".into(),
            ]
        );
    }

    #[test]
    fn provider_executable_resolution_refuses_cwd_dependent_or_wrong_provider_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("provider-bin");
        fs::create_dir(&directory).unwrap();
        write_executable(&directory, "codex");
        let absolute_path = env::join_paths([&directory]).unwrap();

        assert!(matches!(
            ExpectedProviderExecutable::resolve_from_path(ProviderKind::OpenCode, &absolute_path),
            Err(ReconcileError::ExecutableUnavailable),
        ));
        assert!(matches!(
            ExpectedProviderExecutable::resolve_from_path(
                ProviderKind::Codex,
                &env::join_paths([Path::new(".")]).unwrap(),
            ),
            Err(ReconcileError::ExecutableUnavailable),
        ));
    }
}
