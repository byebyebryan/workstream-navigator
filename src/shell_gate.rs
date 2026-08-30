//! shell-gate authority boundary.
//!
//! The hidden shell child classifies its provider command before it
//! opens state or obtains the provisional lease.  A managed command then has
//! to prove that it was invoked by the exact private shell recorded in the
//! presentation marker before the broker may reserve a Runtime.  This module
//! performs no provider effect.

use std::{ffi::OsString, path::Path};

use thiserror::Error;

use crate::{
    clock::{Clock, ClockError},
    domain::{IdGenerator, ProviderKind},
    onboarding::{ShellCommandDecision, classify_shell_command},
    onboarding_broker::{
        BrokerError, PrepareContext, PreparedHandoff, PresentationBinding, WorktreeInspector,
        prepare,
    },
    provisional::{LiveProvisionalShell, SlotError, read_marker},
    runtime::{PrivateRuntime, ProcessGroupInfo, ProcessGroupProbe, ProcessProbe, TmuxClient},
    state::{CurrentState, ProvisionalLease},
};

const CAPABILITY_LIFETIME_MILLIS: i64 = 60_000;

/// A shell command classified before any state or lease operation.
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
    pub(crate) caller_pid: u32,
    pub(crate) caller_group: ProcessGroupInfo,
}

/// Injected host boundaries used while a managed command is promoted. The
/// presentation path is non-authoritative discovery only: the marker, lease,
/// and live runtime proof below decide ownership.
pub(crate) struct ShellGateContext<'a> {
    pub(crate) presentation_directory: &'a Path,
    pub(crate) presentation_binding: PresentationBinding,
    pub(crate) invocation: ShellGateInvocation,
    pub(crate) tmux: &'a dyn TmuxClient,
    pub(crate) process_probe: &'a dyn ProcessProbe,
    pub(crate) process_group_probe: &'a dyn ProcessGroupProbe,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) id_generator: &'a dyn IdGenerator,
    pub(crate) worktree_inspector: &'a dyn WorktreeInspector,
}

/// Bounded refusal reasons for the shell gate. They deliberately
/// retain neither the command, token, paths, process identifiers, nor host
/// clock values.
#[derive(Debug, Error)]
pub(crate) enum ShellGateError {
    #[error("shell command is not eligible for managed onboarding")]
    Command,
    #[error("shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("shell invocation does not match the private provisional shell")]
    InvocationIdentityMismatch,
    #[error("provisional shell evidence is unavailable")]
    Slot(#[from] SlotError),
    #[error("shell clock is unavailable")]
    Clock(#[from] ClockError),
    #[error("shell state is unavailable")]
    State,
    #[error("shell handoff is unavailable")]
    Broker(#[from] BrokerError),
}

/// Prepares one capability for an already classified fresh command. This
/// repeats all marker/runtime checks under the caller-held lease through the
/// broker; it never starts or inspects a provider process.
pub(crate) fn prepare_managed_shell_gate(
    state: &mut CurrentState,
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
        presentation_binding: context.presentation_binding,
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
    validate_invocation_with(shell, invocation, |caller, shell| {
        is_descendant_of_shell(caller, shell, crate::runtime::process_parent)
    })
}

const MAX_SHELL_ANCESTRY_DEPTH: usize = 8;

fn validate_invocation_with<F>(
    shell: &LiveProvisionalShell,
    invocation: ShellGateInvocation,
    is_descendant: F,
) -> Result<(), ShellGateError>
where
    F: FnOnce(u32, u32) -> bool,
{
    if invocation.shell_leader_pid == 0
        || invocation.caller_pid == 0
        || invocation.caller_group.process_group_id == 0
        || invocation.caller_group.session_id == 0
    {
        return Err(ShellGateError::InvocationIdentityUnavailable);
    }
    if shell.shell_pid != invocation.shell_leader_pid
        || shell.shell_session != invocation.caller_group.session_id
        || !is_descendant(invocation.caller_pid, shell.shell_pid)
    {
        return Err(ShellGateError::InvocationIdentityMismatch);
    }
    Ok(())
}

/// A command substitution runs the gate in a distinct foreground process
/// group, but it remains a short descendant of the exact interactive shell.
/// Bound the walk so an unreadable, cyclic, or unexpectedly deep process tree
/// can never become authority.
fn is_descendant_of_shell<F>(caller_pid: u32, shell_pid: u32, mut parent: F) -> bool
where
    F: FnMut(u32) -> Option<u32>,
{
    let mut current = caller_pid;
    for _ in 0..=MAX_SHELL_ANCESTRY_DEPTH {
        if current == shell_pid {
            return true;
        }
        let Some(next) = parent(current) else {
            return false;
        };
        if next == 0 || next == current {
            return false;
        }
        current = next;
    }
    false
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{
        ShellGateDecision, ShellGateError, ShellGateInvocation, classify_shell_gate,
        is_descendant_of_shell, validate_invocation_with,
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
    fn gate_requires_the_exact_recorded_shell_pid_descendant_and_session() {
        let shell = live_shell();
        let exact = ShellGateInvocation {
            shell_leader_pid: 41,
            caller_pid: 44,
            caller_group: ProcessGroupInfo {
                process_group_id: 44,
                session_id: 43,
            },
        };
        assert!(
            validate_invocation_with(&shell, exact, |caller, expected_shell| {
                is_descendant_of_shell(caller, expected_shell, |pid| match pid {
                    44 => Some(42),
                    42 => Some(41),
                    _ => None,
                })
            })
            .is_ok()
        );
        assert!(matches!(
            validate_invocation_with(
                &shell,
                ShellGateInvocation {
                    shell_leader_pid: 99,
                    caller_pid: 44,
                    caller_group: exact.caller_group,
                },
                |_, _| true,
            ),
            Err(ShellGateError::InvocationIdentityMismatch)
        ));
        assert!(matches!(
            validate_invocation_with(
                &shell,
                ShellGateInvocation {
                    shell_leader_pid: 41,
                    caller_pid: 44,
                    caller_group: ProcessGroupInfo {
                        process_group_id: 44,
                        session_id: 99,
                    },
                },
                |_, _| true,
            ),
            Err(ShellGateError::InvocationIdentityMismatch)
        ));
        assert!(matches!(
            validate_invocation_with(&shell, exact, |_, _| false),
            Err(ShellGateError::InvocationIdentityMismatch)
        ));
    }

    #[test]
    fn shell_descendant_proof_refuses_cycles_and_deep_ancestry() {
        assert!(!is_descendant_of_shell(44, 41, |pid| match pid {
            44 => Some(42),
            42 => Some(44),
            _ => None,
        }));
        assert!(!is_descendant_of_shell(50, 41, |pid| {
            (pid > 41).then_some(pid - 1)
        }));
    }
}
