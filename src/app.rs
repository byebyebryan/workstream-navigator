//! Thin provider-aware CLI orchestration for local Workstreams.

mod cli;
mod dispatch;
mod launch;
mod local;
mod model;
pub(crate) mod observer;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

pub(super) use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

pub(super) use clap::{Parser, Subcommand};
pub(super) use thiserror::Error;

pub(super) use crate::{
    domain::{ProviderSessionId, Revision, RuntimeId, WorkstreamId},
    navigator::{materialize_initial_provisional_shell, run_navigator},
    presentation::{AttachmentPhase, Presentation},
    provider::codex::app_server::EphemeralAppServer,
    provider::codex::profile::{OBSERVER_PROFILE_SCHEMA_VERSION, ObserverProfile},
    provider::lifecycle::LifecycleEvent,
    runtime::{
        LinuxProcessProbe, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
        await_launch_release, is_direct_provider_hook,
    },
    state::{HostRegistry, StateError, StateRoot},
};

use model::AppError;
/// Runs one direct local CLI command.
#[must_use]
pub fn run() -> ExitCode {
    let cli = cli::Cli::parse();
    let observer_command = cli::is_observer_command(cli.command.as_ref());
    let provider_pane_command = cli::is_provider_pane_command(cli.command.as_ref());
    let presentation_mouse_command = cli::is_presentation_mouse_command(cli.command.as_ref());
    let shell_gate_command = cli::is_shell_gate_command(cli.command.as_ref());
    let shell_launch_helper_command = cli::is_shell_launch_helper_command(cli.command.as_ref());
    let observer_setup_command = cli::is_observer_setup_command(cli.command.as_ref());
    match dispatch::execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        // The account-shell wrapper needs this one non-error exit code to
        // delegate explicitly unmanaged provider commands back to the native
        // executable. All other gate failures stay silent so the wrapper can
        // present one fixed diagnostic without leaking state detail.
        Err(AppError::ShellGateUnmanaged) if shell_gate_command => ExitCode::from(10),
        Err(AppError::ObserverReadinessRequired) if shell_gate_command => ExitCode::from(11),
        Err(_) if shell_gate_command => ExitCode::FAILURE,
        // The helper replaces the provisional shell, so it has no wrapper to
        // translate its failure. Keep the provider pane free of state detail.
        Err(_) if shell_launch_helper_command => {
            eprintln!("WSNav onboarding command is unavailable");
            ExitCode::FAILURE
        }
        // The root mouse predicate is a synchronous tmux gate. Its failure
        // remains non-zero and silent so tmux does not select or forward the
        // original press, while diagnostics never reach a pane.
        Err(_) if presentation_mouse_command => ExitCode::FAILURE,
        // The interactive observer helper runs beside native Codex. Keep its
        // bounded failure silent so only the account-shell wrapper can render
        // the fixed setup-unavailable diagnostic and preserve its status.
        Err(_) if observer_setup_command => ExitCode::FAILURE,
        // Observer helpers are disconnected from the provider pane. Keep
        // their bounded errors silent, but let the owning action observe a
        // non-success exit status. Other provider-pane helpers deliberately
        // remain success-suppressed; normal navigator polling owns their
        // bounded state presentation after an attachment ends.
        Err(_) if observer_command => ExitCode::FAILURE,
        Err(_) if provider_pane_command => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
