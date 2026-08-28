//! D17 shell-promotion broker.
//!
//! This crate-private seam joins the presentation marker, live private-shell
//! proof, provider grammar, Git worktree inspection, and schema-14 state
//! reservation. It deliberately does not create a provider process, attach a
//! terminal, or invoke a helper.

use std::{ffi::OsString, path::Path};

use thiserror::Error;

use crate::{
    domain::{IdGenerator, ProviderKind, Revision},
    onboarding::{LaunchCapability, ShellCommandDecision, classify_shell_command},
    provisional::{ProvisionalPhase, ProvisionalSlot, SlotError, read_marker, update_marker},
    repository::{RepositoryError, RepositoryRegistration, inspect_containing_worktree},
    runtime::{PrivateRuntime, ProcessGroupProbe},
    state::d16::{OnboardingOwnership, OnboardingPreparation, OnboardingPrepareRequest},
    state::{D16State, ProvisionalLease, StateError},
};

/// Read-only Git discovery used by the broker. Keeping this dependency narrow
/// lets the complete authority handoff run against disposable fixtures without
/// consulting an operator repository.
pub(crate) trait WorktreeInspector {
    fn inspect_containing_worktree(
        &self,
        cwd: &Path,
    ) -> Result<RepositoryRegistration, RepositoryError>;
}

/// Exact presentation identity carried from the private ownership marker into
/// every shell-gate and helper fence. A presentation path is discovery only:
/// the broker accepts a provisional slot only when this opaque identity still
/// matches its marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PresentationBinding {
    presentation_id: uuid::Uuid,
    presentation_revision: Revision,
}

impl PresentationBinding {
    /// Builds a bounded binding recovered from a D17 presentation marker.
    /// Callers cannot construct a nil or pre-initial presentation identity.
    pub(crate) fn new(
        presentation_id: uuid::Uuid,
        presentation_revision: Revision,
    ) -> Result<Self, BrokerError> {
        if presentation_id.is_nil() || presentation_revision.value() < Revision::INITIAL.value() {
            return Err(BrokerError::PresentationBindingInvalid);
        }
        Ok(Self {
            presentation_id,
            presentation_revision,
        })
    }

    fn matches(self, slot: &ProvisionalSlot) -> bool {
        self.presentation_id == slot.presentation_id()
            && self.presentation_revision == slot.presentation_revision()
    }

    /// Refuses a slot that was not materialized by this exact presentation
    /// context before any state mutation or provider effect may occur.
    pub(crate) fn validate_slot(self, slot: &ProvisionalSlot) -> Result<(), BrokerError> {
        self.matches(slot)
            .then_some(())
            .ok_or(BrokerError::PresentationBindingMismatch)
    }
}

/// The production read-only worktree inspector.
pub(crate) struct SystemWorktreeInspector;

impl WorktreeInspector for SystemWorktreeInspector {
    fn inspect_containing_worktree(
        &self,
        cwd: &Path,
    ) -> Result<RepositoryRegistration, RepositoryError> {
        inspect_containing_worktree(cwd)
    }
}

/// All transient authority required to prepare one brokered native launch.
/// The provider command remains only in memory, and the token is returned only
/// through [`PreparedHandoff`]'s crate-private channel.
pub(crate) struct PrepareContext<'a, 'runtime> {
    pub(crate) presentation_directory: &'a Path,
    pub(crate) presentation_binding: PresentationBinding,
    pub(crate) runtime: &'a PrivateRuntime<'runtime>,
    pub(crate) process_group_probe: &'a dyn ProcessGroupProbe,
    pub(crate) provider: ProviderKind,
    pub(crate) arguments: &'a [OsString],
    pub(crate) now_monotonic_millis: i64,
    pub(crate) expiry_monotonic_millis: i64,
    pub(crate) boot_provenance: &'a str,
    pub(crate) id_generator: &'a dyn IdGenerator,
    pub(crate) worktree_inspector: &'a dyn WorktreeInspector,
}

/// The one live token emitted after a durable reservation and exact marker
/// handoff. Its Debug output cannot reveal the token.
pub(crate) struct PreparedHandoff {
    #[cfg(test)]
    operation_id: crate::domain::OperationId,
    capability: LaunchCapability,
}

impl std::fmt::Debug for PreparedHandoff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedHandoff")
            .field("operation_id", &"<opaque>")
            .field("capability", &self.capability)
            .finish()
    }
}

impl PreparedHandoff {
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn operation_id(&self) -> crate::domain::OperationId {
        self.operation_id
    }

    #[must_use]
    pub(crate) fn capability(&self) -> &LaunchCapability {
        &self.capability
    }
}

/// Bounded broker outcomes. No error contains a cwd, command line, token,
/// provider payload, or other terminal content.
#[derive(Debug, Error)]
pub(crate) enum BrokerError {
    #[error("provisional shell evidence is unavailable")]
    Slot(#[from] SlotError),
    #[error("provider command is not eligible for managed fresh-session onboarding")]
    Command,
    #[error("Git worktree inspection is unavailable")]
    Repository(#[from] RepositoryError),
    #[error("D17 state reservation is unavailable")]
    State(#[from] StateError),
    #[error("provisional lease generation does not match the marker")]
    LeaseGenerationMismatch,
    #[error("D17 presentation binding is invalid")]
    PresentationBindingInvalid,
    #[error("D17 provisional slot does not match this presentation")]
    PresentationBindingMismatch,
    #[error("an unresolved onboarding operation already owns this provisional slot")]
    ExistingOperation,
}

/// Prepares exactly one managed fresh-session launch under the stable D17
/// lease. The caller must still arrange a private helper handoff; this seam
/// performs no provider effect.
pub(crate) fn prepare(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
) -> Result<PreparedHandoff, BrokerError> {
    let (slot, request) = request_from_context(state, provisional_lease, context)?;
    let OnboardingPreparation::Issued(reservation) =
        state.prepare_d17_onboarding_current(provisional_lease, &request, context.id_generator)?
    else {
        return Err(BrokerError::ExistingOperation);
    };

    provisional_lease.revalidate_for_mutation(state.root())?;
    let operation_id = reservation.operation_id();
    let mut handoff_slot = slot.clone();
    handoff_slot.issue_handoff(operation_id.as_uuid())?;
    update_marker(
        state.root(),
        context.presentation_directory,
        &slot,
        &handoff_slot,
    )?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    Ok(PreparedHandoff {
        #[cfg(test)]
        operation_id,
        capability: reservation.into_capability(),
    })
}

/// Atomically consumes a revalidated capability and then removes provisional
/// cleanup authority from the marker. This still performs no provider effect:
/// a marker-update failure after durable ownership is recovery evidence and
/// leaves the provider launch fenced.
pub(crate) fn consume(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    token: &str,
    now_monotonic_millis: i64,
) -> Result<OnboardingOwnership, BrokerError> {
    let (slot, request) = request_from_context(state, provisional_lease, context)?;
    if slot.phase() != ProvisionalPhase::HandoffIssued {
        return Err(SlotError::HandoffUnavailable.into());
    }
    let ownership = state.consume_d17_onboarding_current(
        provisional_lease,
        &request,
        token,
        now_monotonic_millis,
    )?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    let mut owned_slot = slot.clone();
    owned_slot.consume_handoff(ownership.operation_id.as_uuid())?;
    update_marker(
        state.root(),
        context.presentation_directory,
        &slot,
        &owned_slot,
    )?;
    provisional_lease.revalidate_for_mutation(state.root())?;
    Ok(ownership)
}

/// Rebuilds the complete state request from live marker, shell, grammar, and
/// worktree evidence. The helper repeats this before every post-consume phase
/// transition so stale provider preparation cannot advance a changed slot.
pub(crate) fn request_from_context(
    state: &D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
) -> Result<(ProvisionalSlot, OnboardingPrepareRequest), BrokerError> {
    provisional_lease.revalidate_for_mutation(state.root())?;
    let slot = read_marker(state.root(), context.presentation_directory)?;
    context.presentation_binding.validate_slot(&slot)?;
    if slot.lease_generation() != provisional_lease.lease_generation() {
        return Err(BrokerError::LeaseGenerationMismatch);
    }
    let shell = slot.revalidate_live_shell(context.runtime, context.process_group_probe)?;
    let ShellCommandDecision::ManagedFresh(launch) =
        classify_shell_command(context.provider, context.arguments)
            .map_err(|_| BrokerError::Command)?
    else {
        return Err(BrokerError::Command);
    };
    let repository = context
        .worktree_inspector
        .inspect_containing_worktree(&shell.cwd)?;
    let request = OnboardingPrepareRequest {
        request_key: request_key(&slot),
        presentation_id: slot.presentation_id(),
        presentation_revision: slot.presentation_revision(),
        slot_generation: slot.slot_generation(),
        candidate_runtime_id: slot.candidate_runtime_id(),
        runtime_paths: slot.runtime_paths().clone(),
        provider: launch.provider(),
        repository,
        shell_cwd: shell.cwd,
        shell_pid: shell.shell_pid,
        shell_birth: shell.shell_birth,
        shell_process_group: shell.shell_process_group,
        shell_session: shell.shell_session,
        argv_digest: launch.argv_digest().to_owned(),
        boot_provenance: context.boot_provenance.to_owned(),
        now_monotonic_millis: context.now_monotonic_millis,
        expiry_monotonic_millis: context.expiry_monotonic_millis,
    };
    Ok((slot, request))
}

fn request_key(slot: &ProvisionalSlot) -> String {
    format!(
        "d17-slot-v1:{}:{}",
        slot.presentation_id(),
        slot.slot_generation()
    )
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::BTreeMap,
        ffi::OsString,
        fs::{self, OpenOptions},
        path::{Path, PathBuf},
        str::FromStr,
        sync::atomic::{AtomicU64, Ordering},
    };

    use uuid::Uuid;

    use super::{
        BrokerError, PrepareContext, PresentationBinding, WorktreeInspector, consume, prepare,
    };
    use crate::{
        d17_helper::{advance_codex_to_provider_exec_fence, begin_provider_preparation},
        d17_reconcile::{ProviderExecutableProbe, ReconcileError, prove_provider_exec},
        domain::{IdGenerator, ProviderKind, Revision, RuntimeId},
        provisional::{
            PROVISIONAL_MARKER_FILE, ProvisionalPhase, ProvisionalSlot, SlotGeneration,
            materialize_private_shell, read_marker, retire_provider_exec_proven_marker,
        },
        repository::{RepositoryError, RepositoryRegistration},
        runtime::{
            NativeLaunch, PrivateRuntime, ProcessGroupInfo, ProcessGroupProbe, ProcessProbe,
            ProcessProbeError, RuntimeError, TmuxClient, TmuxInvocation, TmuxResponse,
        },
        state::d16::OnboardingProviderExecutableIdentity,
        state::{
            StateRoot, TRANSITION_LOCK_FILE, acquire_transition_lease, fresh_create,
            open_cutover_transition, open_d17_current_only,
        },
    };

    #[derive(Default)]
    struct SequenceIds(AtomicU64);

    impl IdGenerator for SequenceIds {
        fn uuid(&self) -> Uuid {
            Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed) + 1))
        }
    }

    struct ShellProbe;

    impl ProcessProbe for ShellProbe {
        fn process_birth(&self, pid: u32) -> Option<String> {
            (pid == 4242).then(|| "birth-4242".to_owned())
        }
    }

    struct ShellGroup;

    impl ProcessGroupProbe for ShellGroup {
        fn process_group_checked(
            &self,
            pid: u32,
        ) -> Result<Option<ProcessGroupInfo>, ProcessProbeError> {
            Ok((pid == 4242).then_some(ProcessGroupInfo {
                process_group_id: 4242,
                session_id: 31337,
            }))
        }

        fn process_group_members_checked(
            &self,
            _group: &ProcessGroupInfo,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            Ok(vec![4242])
        }

        fn process_group_members_by_id_checked(
            &self,
            _process_group_id: u32,
        ) -> Result<Vec<u32>, ProcessProbeError> {
            Ok(vec![4242])
        }
    }

    struct ShellTmux {
        cwd: RefCell<PathBuf>,
    }

    impl TmuxClient for ShellTmux {
        fn invoke(&self, invocation: &TmuxInvocation) -> Result<TmuxResponse, RuntimeError> {
            let command = invocation
                .arguments
                .first()
                .map(OsString::as_os_str)
                .and_then(|value| value.to_str())
                .ok_or_else(|| RuntimeError::TmuxRejected("invalid fixture command".to_owned()))?;
            let stdout = match command {
                "display-message" => {
                    match invocation.arguments.last().and_then(|value| value.to_str()) {
                        Some("#{pane_id}") => "%17\n".to_owned(),
                        Some("#{pane_pid}") => "4242\n".to_owned(),
                        Some("#{pane_current_path}") => {
                            format!("{}\n", self.cwd.borrow().display())
                        }
                        _ => {
                            return Err(RuntimeError::TmuxRejected(
                                "invalid fixture field".to_owned(),
                            ));
                        }
                    }
                }
                "new-session" | "has-session" => String::new(),
                _ => {
                    return Err(RuntimeError::TmuxRejected(
                        "unexpected fixture command".to_owned(),
                    ));
                }
            };
            Ok(TmuxResponse {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }

    struct FixtureWorktreeInspector {
        registration: RepositoryRegistration,
    }

    struct FixtureExecutableProbe {
        identity: OnboardingProviderExecutableIdentity,
    }

    impl ProviderExecutableProbe for FixtureExecutableProbe {
        fn executable_identity_for_pid(
            &self,
            pid: u32,
        ) -> Result<Option<OnboardingProviderExecutableIdentity>, ReconcileError> {
            Ok((pid == 4242).then_some(self.identity))
        }
    }

    struct UnavailableExecutableProbe;

    impl ProviderExecutableProbe for UnavailableExecutableProbe {
        fn executable_identity_for_pid(
            &self,
            _pid: u32,
        ) -> Result<Option<OnboardingProviderExecutableIdentity>, ReconcileError> {
            Err(ReconcileError::ProviderExecutableMismatch)
        }
    }

    impl WorktreeInspector for FixtureWorktreeInspector {
        fn inspect_containing_worktree(
            &self,
            cwd: &Path,
        ) -> Result<RepositoryRegistration, RepositoryError> {
            if cwd.starts_with(&self.registration.project_root) {
                Ok(self.registration.clone())
            } else {
                Err(RepositoryError::InvalidGitOutput)
            }
        }
    }

    fn transition_lease(path: &Path) -> crate::state::TransitionLease {
        let lock_path = path.join(TRANSITION_LOCK_FILE);
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        drop(lock);
        acquire_transition_lease(path).unwrap()
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the complete D17 authority handoff is one auditable fixture"
    )]
    fn broker_reserves_and_consumes_once_after_exact_marker_shell_and_grammar_proof() {
        let temporary = tempfile::tempdir().unwrap();
        let state_path = temporary.path().join("state");
        let repository_root = temporary.path().join("worktree");
        let shell_cwd = repository_root.join("nested");
        fs::create_dir(&repository_root).unwrap();
        fs::create_dir(&shell_cwd).unwrap();
        let root = StateRoot::select(&state_path);
        drop(fresh_create(&state_path, &SequenceIds::default()).unwrap());
        let transition = transition_lease(&state_path);
        let mut migrating = open_cutover_transition(&root, &transition).unwrap();
        migrating.migrate_schema13_to14(&transition).unwrap();
        drop(migrating);
        drop(transition);
        fs::remove_file(state_path.join(TRANSITION_LOCK_FILE)).unwrap();

        let mut state = open_d17_current_only(&root).unwrap();
        let provisional_lease = state.acquire_d17_provisional_lease().unwrap();
        let presentation = state_path.join("presentation");
        fs::create_dir(&presentation).unwrap();
        let slot = ProvisionalSlot::materializing(
            &state_path,
            Uuid::from_u128(801),
            Revision::INITIAL,
            provisional_lease.lease_generation(),
            RuntimeId::from_str("01234567-0000-0000-0000-000000000801").unwrap(),
            SlotGeneration::new(Uuid::from_u128(802)),
            &shell_cwd,
        )
        .unwrap();
        let tmux = ShellTmux {
            cwd: RefCell::new(shell_cwd.clone()),
        };
        let process_probe = ShellProbe;
        let runtime = PrivateRuntime::new(&tmux, &process_probe, slot.runtime_paths().clone());
        let launch = NativeLaunch {
            cwd: shell_cwd.clone(),
            program: vec![OsString::from("synthetic-provisional-shell")],
            environment: BTreeMap::new(),
        };
        materialize_private_shell(
            &state_path,
            &presentation,
            &slot,
            &runtime,
            &launch,
            &ShellGroup,
        )
        .unwrap();

        let inspector = FixtureWorktreeInspector {
            registration: RepositoryRegistration {
                project_root: repository_root.canonicalize().unwrap(),
                display_name: "worktree".to_owned(),
                remote_identity_fingerprint: None,
                remote_identity_display: None,
            },
        };
        let ids = SequenceIds::default();
        let expected_executable_identity =
            OnboardingProviderExecutableIdentity::new(17, 19).unwrap();
        let unexpected_executable_identity =
            OnboardingProviderExecutableIdentity::new(23, 29).unwrap();
        let context = PrepareContext {
            presentation_directory: &presentation,
            presentation_binding: PresentationBinding::new(
                slot.presentation_id(),
                slot.presentation_revision(),
            )
            .unwrap(),
            runtime: &runtime,
            process_group_probe: &ShellGroup,
            provider: ProviderKind::Codex,
            arguments: &[OsString::from("--model"), OsString::from("gpt-5.6")],
            now_monotonic_millis: 10,
            expiry_monotonic_millis: 1_010,
            boot_provenance: "d17-boot-v1:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            id_generator: &ids,
            worktree_inspector: &inspector,
        };
        let mismatched_presentation = PrepareContext {
            presentation_directory: &presentation,
            presentation_binding: PresentationBinding::new(Uuid::from_u128(999), Revision::INITIAL)
                .unwrap(),
            runtime: &runtime,
            process_group_probe: &ShellGroup,
            provider: ProviderKind::Codex,
            arguments: &[OsString::from("--model"), OsString::from("gpt-5.6")],
            now_monotonic_millis: 10,
            expiry_monotonic_millis: 1_010,
            boot_provenance: "d17-boot-v1:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            id_generator: &ids,
            worktree_inspector: &inspector,
        };
        assert!(matches!(
            prepare(&mut state, &provisional_lease, &mismatched_presentation),
            Err(BrokerError::PresentationBindingMismatch)
        ));

        let handoff = prepare(&mut state, &provisional_lease, &context).unwrap();
        let token = handoff.capability().token().to_owned();
        assert!(!format!("{handoff:?}").contains(&token));
        assert!(read_marker(&state_path, &presentation).is_ok());
        assert!(matches!(
            prepare(&mut state, &provisional_lease, &context),
            Err(BrokerError::ExistingOperation)
        ));
        let after_reboot_context = PrepareContext {
            presentation_directory: &presentation,
            presentation_binding: PresentationBinding::new(
                slot.presentation_id(),
                slot.presentation_revision(),
            )
            .unwrap(),
            runtime: &runtime,
            process_group_probe: &ShellGroup,
            provider: ProviderKind::Codex,
            arguments: &[OsString::from("--model"), OsString::from("gpt-5.6")],
            now_monotonic_millis: 11,
            expiry_monotonic_millis: 1_011,
            boot_provenance: "d17-boot-v1:sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            id_generator: &ids,
            worktree_inspector: &inspector,
        };
        assert!(
            begin_provider_preparation(
                &mut state,
                &provisional_lease,
                &after_reboot_context,
                &token,
                11,
                expected_executable_identity,
            )
            .is_err(),
            "a helper after a different boot must not consume the handoff"
        );
        let exec_fence = advance_codex_to_provider_exec_fence(
            &mut state,
            &provisional_lease,
            &context,
            &token,
            11,
            expected_executable_identity,
        )
        .unwrap();
        assert_eq!(exec_fence.operation_id(), handoff.operation_id());
        assert_eq!(exec_fence.runtime_id(), slot.candidate_runtime_id());
        assert!(matches!(
            consume(&mut state, &provisional_lease, &context, &token, 11),
            Err(BrokerError::Slot(
                crate::provisional::SlotError::HandoffUnavailable
            ))
        ));
        assert!(matches!(
            prove_provider_exec(
                &mut state,
                &provisional_lease,
                &presentation,
                &runtime,
                &ShellGroup,
                &FixtureExecutableProbe {
                    identity: unexpected_executable_identity,
                },
            ),
            Err(ReconcileError::ProviderCwdMismatch),
        ));
        assert_eq!(
            read_marker(&state_path, &presentation).unwrap().phase(),
            ProvisionalPhase::RuntimeOwnedLaunching,
            "a cwd mismatch must not advance the durable or marker proof phase"
        );
        *tmux.cwd.borrow_mut() = repository_root.canonicalize().unwrap();
        assert!(matches!(
            prove_provider_exec(
                &mut state,
                &provisional_lease,
                &presentation,
                &runtime,
                &ShellGroup,
                &FixtureExecutableProbe {
                    identity: unexpected_executable_identity,
                },
            ),
            Err(ReconcileError::ProviderExecutableMismatch),
        ));
        assert_eq!(
            read_marker(&state_path, &presentation).unwrap().phase(),
            ProvisionalPhase::RuntimeOwnedLaunching,
            "an executable mismatch must not advance the durable or marker proof phase"
        );
        prove_provider_exec(
            &mut state,
            &provisional_lease,
            &presentation,
            &runtime,
            &ShellGroup,
            &FixtureExecutableProbe {
                identity: expected_executable_identity,
            },
        )
        .unwrap();
        assert_eq!(
            read_marker(&state_path, &presentation).unwrap().phase(),
            ProvisionalPhase::ProviderExecProven
        );
        let marker_path = presentation.join(PROVISIONAL_MARKER_FILE);
        assert!(marker_path.is_file());
        let stale_marker = fs::read_to_string(&marker_path).unwrap().replacen(
            "\"provider_exec_proven\"",
            "\"runtime_owned_launching\"",
            1,
        );
        assert!(stale_marker.contains("\"runtime_owned_launching\""));
        fs::write(&marker_path, stale_marker).unwrap();
        prove_provider_exec(
            &mut state,
            &provisional_lease,
            &presentation,
            &runtime,
            &ShellGroup,
            &UnavailableExecutableProbe,
        )
        .unwrap();
        assert_eq!(
            read_marker(&state_path, &presentation).unwrap().phase(),
            ProvisionalPhase::ProviderExecProven,
            "a state-before-marker crash is repaired without probing the provider again"
        );
        let proven = read_marker(&state_path, &presentation).unwrap();
        retire_provider_exec_proven_marker(&state, &provisional_lease, &presentation, &proven)
            .unwrap();
        assert!(matches!(
            read_marker(&state_path, &presentation),
            Err(crate::provisional::SlotError::MarkerUnavailable)
        ));
        assert!(matches!(
            runtime.probe().unwrap(),
            crate::runtime::RuntimeProbe::Live { .. }
        ));
    }
}
