//! Dormant system adapter for the D17 account-shell gate.
//!
//! This is the only composition point that knows how to reopen inherited
//! account-shell discovery paths against the real private state, tmux, Linux
//! process table, and boot clock. The future hidden CLI may render its opaque
//! capability on stdout, but this module itself never writes terminal output,
//! starts a provider, or exposes a command route.

#![allow(
    dead_code,
    reason = "the D17 account-shell control remains unreachable until the atomic Navigator cutover"
)]

use std::{ffi::OsString, process};

use thiserror::Error;

use crate::{
    d17_account_shell::{AccountShellContext, AccountShellError},
    d17_broker::PreparedHandoff,
    d17_clock::SystemD17Clock,
    d17_shell_gate::{
        ShellGateContext, ShellGateDecision, ShellGateError, ShellGateInvocation,
        classify_shell_gate, prepare_managed_shell_gate,
    },
    domain::{ProviderKind, RandomIdGenerator},
    runtime::{LinuxProcessProbe, ProcessGroupProbe, SystemTmux},
    state::{StateRoot, open_d17_current_only},
};

/// The only two outcomes a shell wrapper needs from the gate. An unmanaged
/// result is intentionally side-effect-free; a managed result contains the
/// opaque one-shot capability for the future helper's private channel.
pub(crate) enum AccountShellGateOutcome {
    ExplicitlyUnmanaged,
    Prepared(PreparedHandoff),
}

impl std::fmt::Debug for AccountShellGateOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExplicitlyUnmanaged => formatter.write_str("ExplicitlyUnmanaged"),
            Self::Prepared(_) => formatter.write_str("Prepared(<opaque>)"),
        }
    }
}

/// Bounded system-adapter failures. No private discovery path, token, command,
/// process identifier, or tmux diagnostic crosses this boundary.
#[derive(Debug, Error)]
pub(crate) enum AccountShellGateError {
    #[error("D17 account-shell context is unavailable")]
    Context(#[from] AccountShellError),
    #[error("D17 shell state is unavailable")]
    State,
    #[error("D17 shell invocation identity is unavailable")]
    InvocationIdentityUnavailable,
    #[error("D17 shell handoff is unavailable")]
    Gate(#[from] ShellGateError),
}

/// Runs the complete real-host gate after a shell wrapper has supplied its own
/// PID. Provider command classification is deliberately first, so an
/// explicitly unmanaged command does not read inherited context, open state,
/// acquire a lease, inspect tmux, or sample the process table.
pub(crate) fn gate_from_account_shell(
    provider: ProviderKind,
    arguments: &[OsString],
    shell_leader_pid: u32,
) -> Result<AccountShellGateOutcome, AccountShellGateError> {
    let ShellGateDecision::Managed(command) = classify_shell_gate(provider, arguments)? else {
        return Ok(AccountShellGateOutcome::ExplicitlyUnmanaged);
    };
    let account_context = AccountShellContext::from_environment()?;
    let root = StateRoot::select(account_context.state_root());
    let mut state = open_d17_current_only(&root).map_err(|_| AccountShellGateError::State)?;
    let provisional_lease = state
        .acquire_d17_provisional_lease()
        .map_err(|_| AccountShellGateError::State)?;
    let process_probe = LinuxProcessProbe;
    let caller_group = process_probe
        .process_group_checked(process::id())
        .map_err(|_| AccountShellGateError::InvocationIdentityUnavailable)?
        .ok_or(AccountShellGateError::InvocationIdentityUnavailable)?;
    let tmux = SystemTmux::default();
    let clock = SystemD17Clock;
    let ids = RandomIdGenerator;
    let context = ShellGateContext {
        presentation_directory: account_context.presentation_directory(),
        invocation: ShellGateInvocation {
            shell_leader_pid,
            caller_group,
        },
        tmux: &tmux,
        process_probe: &process_probe,
        process_group_probe: &process_probe,
        clock: &clock,
        id_generator: &ids,
        worktree_inspector: &crate::d17_broker::SystemWorktreeInspector,
    };
    let prepared = prepare_managed_shell_gate(&mut state, &provisional_lease, &command, &context)?;
    Ok(AccountShellGateOutcome::Prepared(prepared))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{AccountShellGateOutcome, gate_from_account_shell};
    use crate::domain::ProviderKind;

    #[test]
    fn unmanaged_commands_return_before_the_real_host_context_is_required() {
        let outcome =
            gate_from_account_shell(ProviderKind::Codex, &[OsString::from("--version")], 1)
                .unwrap();
        assert!(matches!(
            outcome,
            AccountShellGateOutcome::ExplicitlyUnmanaged
        ));
    }
}
