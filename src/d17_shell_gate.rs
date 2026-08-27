//! Dormant D17 shell-gate authority boundary.
//!
//! The future hidden shell child classifies its provider command before it
//! opens state or obtains the provisional lease.  A managed command then has
//! to prove that it was invoked by the exact private shell recorded in the
//! presentation marker before the broker may reserve a Runtime.  This module
//! performs no provider effect and remains unreachable until the atomic D17
//! Navigator cutover.

#![allow(
    dead_code,
    reason = "the D17 shell gate remains unreachable until the atomic Navigator cutover"
)]

use std::{ffi::OsString, path::Path};

use thiserror::Error;

use crate::{
    d17_broker::{BrokerError, PrepareContext, PreparedHandoff, WorktreeInspector, prepare},
    d17_clock::{ClockError, D17Clock},
    domain::{IdGenerator, ProviderKind},
    onboarding::{ShellCommandDecision, classify_shell_command},
    provisional::{LiveProvisionalShell, SlotError, read_marker},
    runtime::{PrivateRuntime, ProcessGroupInfo, ProcessGroupProbe, ProcessProbe, TmuxClient},
    state::{D16State, ProvisionalLease},
};

const CAPABILITY_LIFETIME_MILLIS: i64 = 60_000;

/// A shell command classified before any D17 state or lease operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ShellGateDecision {
    /// The provider owns this explicitly enumerated non-session command.
    ExplicitlyUnmanaged,
    /// A fresh native TUI command that may proceed to exact shell validation.
    Managed(ManagedShellCommand),
}

/// Normalized gate input retained only until the one-shot handoff is issued.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedShellCommand {
    provider: ProviderKind,
    arguments: Vec<OsString>,
}

impl ManagedShellCommand {
    #[must_use]
    pub(crate) const fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub(crate) fn arguments(&self) -> &[OsString] {
        &self.arguments
    }
}

/// Classifies one account-shell provider function invocation without opening
/// state, creating a lock, inspecting tmux, or invoking a provider.
pub(crate) fn classify_shell_gate(
    provider: ProviderKind,
    arguments: &[OsString],
) -> Result<ShellGateDecision, ShellGateError> {
    match classify_shell_command(provider, arguments).map_err(|_| ShellGateError::Command)? {
        ShellCommandDecision::ExplicitlyUnmanaged => Ok(ShellGateDecision::ExplicitlyUnmanaged),
        ShellCommandDecision::ManagedFresh(_) => {
            Ok(ShellGateDecision::Managed(ManagedShellCommand {
                provider,
                arguments: arguments.to_vec(),
            }))
        }
    }
}

/// The exact child-process evidence that must match the marker's shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShellGateInvocation {
    pub(crate) shell_leader_pid: u32,
    pub(crate) caller_group: ProcessGroupInfo,
}

/// Injected host boundaries used while a managed command is promoted. The
/// presentation path is non-authoritative discovery only: the marker, lease,
/// and live runtime proof below decide ownership.
pub(crate) struct ShellGateContext<'a> {
    pub(crate) presentation_directory: &'a Path,
    pub(crate) invocation: ShellGateInvocation,
    pub(crate) tmux: &'a dyn TmuxClient,
    pub(crate) process_probe: &'a dyn ProcessProbe,
    pub(crate) process_group_probe: &'a dyn ProcessGroupProbe,
    pub(crate) clock: &'a dyn D17Clock,
    pub(crate) id_generator: &'a dyn IdGenerator,
    pub(crate) worktree_inspector: &'a dyn WorktreeInspector,
}

/// Bounded refusal reasons for the dormant shell gate. They deliberately
/// retain neither the command, token, paths, process identifiers, nor host
/// clock values.
#[derive(Debug, Error)]
pub(crate) enum ShellGateError {
    #[error("D17 shell command is not eligible for managed onboarding")]
    Command,
    #[error("D17 shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("D17 shell invocation does not match the private provisional shell")]
    InvocationIdentityMismatch,
    #[error("D17 provisional shell evidence is unavailable")]
    Slot(#[from] SlotError),
    #[error("D17 shell clock is unavailable")]
    Clock(#[from] ClockError),
    #[error("D17 shell state is unavailable")]
    State,
    #[error("D17 shell handoff is unavailable")]
    Broker(#[from] BrokerError),
}

/// Prepares one capability for an already classified fresh command. This
/// repeats all marker/runtime checks under the caller-held lease through the
/// broker; it never starts or inspects a provider process.
pub(crate) fn prepare_managed_shell_gate(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    command: &ManagedShellCommand,
    context: &ShellGateContext<'_>,
) -> Result<PreparedHandoff, ShellGateError> {
    provisional_lease
        .revalidate_for_mutation(state.root())
        .map_err(|_| ShellGateError::State)?;
    let slot = read_marker(state.root(), context.presentation_directory)?;
    let runtime = PrivateRuntime::new(
        context.tmux,
        context.process_probe,
        slot.runtime_paths().clone(),
    );
    let live = slot.revalidate_live_shell(&runtime, context.process_group_probe)?;
    validate_invocation(&live, context.invocation)?;

    let now_monotonic_millis = context.clock.now_monotonic_millis()?;
    let expiry_monotonic_millis = now_monotonic_millis
        .checked_add(CAPABILITY_LIFETIME_MILLIS)
        .ok_or(ClockError::Unavailable)?;
    let boot_provenance = context.clock.boot_provenance()?;
    let broker_context = PrepareContext {
        presentation_directory: context.presentation_directory,
        runtime: &runtime,
        process_group_probe: context.process_group_probe,
        provider: command.provider(),
        arguments: command.arguments(),
        now_monotonic_millis,
        expiry_monotonic_millis,
        boot_provenance: &boot_provenance,
        id_generator: context.id_generator,
        worktree_inspector: context.worktree_inspector,
    };
    prepare(state, provisional_lease, &broker_context).map_err(Into::into)
}

/// Validates the process identity of either the gate child or the exec helper
/// against freshly observed provisional-shell evidence.
pub(crate) fn validate_invocation(
    shell: &LiveProvisionalShell,
    invocation: ShellGateInvocation,
) -> Result<(), ShellGateError> {
    if invocation.shell_leader_pid == 0
        || invocation.caller_group.process_group_id == 0
        || invocation.caller_group.session_id == 0
    {
        return Err(ShellGateError::InvocationIdentityUnavailable);
    }
    if shell.shell_pid != invocation.shell_leader_pid
        || shell.shell_process_group != invocation.caller_group.process_group_id
        || shell.shell_session != invocation.caller_group.session_id
    {
        return Err(ShellGateError::InvocationIdentityMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{
        ShellGateDecision, ShellGateError, ShellGateInvocation, classify_shell_gate,
        validate_invocation,
    };
    use crate::{
        domain::ProviderKind, provisional::LiveProvisionalShell, runtime::ProcessGroupInfo,
    };

    #[test]
    fn explicit_provider_queries_stay_unmanaged_before_any_state_boundary() {
        assert!(matches!(
            classify_shell_gate(ProviderKind::Codex, &[OsString::from("--version")]),
            Ok(ShellGateDecision::ExplicitlyUnmanaged)
        ));
        assert!(matches!(
            classify_shell_gate(ProviderKind::OpenCode, &[OsString::from("providers")]),
            Ok(ShellGateDecision::ExplicitlyUnmanaged)
        ));
    }

    #[test]
    fn only_a_pinned_fresh_tui_shape_reaches_the_managed_gate() {
        let decision = classify_shell_gate(
            ProviderKind::Codex,
            &[OsString::from("--model"), OsString::from("gpt-5.6")],
        )
        .unwrap();
        assert!(matches!(decision, ShellGateDecision::Managed(_)));
        assert!(matches!(
            classify_shell_gate(ProviderKind::Codex, &[OsString::from("resume")]),
            Err(ShellGateError::Command)
        ));
    }

    fn live_shell() -> LiveProvisionalShell {
        LiveProvisionalShell {
            cwd: std::path::PathBuf::from("/disposable/root"),
            pane_id: "%1".to_owned(),
            shell_pid: 41,
            shell_birth: "birth".to_owned(),
            shell_process_group: 42,
            shell_session: 43,
        }
    }

    #[test]
    fn gate_requires_the_exact_recorded_shell_pid_group_and_session() {
        let shell = live_shell();
        let exact = ShellGateInvocation {
            shell_leader_pid: 41,
            caller_group: ProcessGroupInfo {
                process_group_id: 42,
                session_id: 43,
            },
        };
        assert!(validate_invocation(&shell, exact).is_ok());
        assert!(matches!(
            validate_invocation(
                &shell,
                ShellGateInvocation {
                    shell_leader_pid: 99,
                    caller_group: exact.caller_group,
                },
            ),
            Err(ShellGateError::InvocationIdentityMismatch)
        ));
        assert!(matches!(
            validate_invocation(
                &shell,
                ShellGateInvocation {
                    shell_leader_pid: 41,
                    caller_group: ProcessGroupInfo {
                        process_group_id: 42,
                        session_id: 99,
                    },
                },
            ),
            Err(ShellGateError::InvocationIdentityMismatch)
        ));
    }
}
