//! Thin provider-aware CLI orchestration for local Workstreams.

mod cli;
mod dispatch;
mod launch;
mod local;
mod model;
mod observer;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

pub(super) use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};

pub(super) use clap::{Parser, Subcommand};
pub(super) use thiserror::Error;

pub(super) use crate::{
    domain::{OperationId, ProviderSessionId, Revision, RuntimeId, WorkstreamId},
    navigator::run_local_navigator,
    presentation::{AttachmentPhase, Presentation},
    provider::codex::app_server::EphemeralAppServer,
    provider::codex::profile::{OBSERVER_PROFILE_SCHEMA_VERSION, ObserverProfile},
    provider::lifecycle::LifecycleEvent,
    runtime::{
        LinuxProcessProbe, PrivateRuntime, RuntimePaths, RuntimeProbe, SystemTmux,
        await_launch_release, is_direct_provider_hook,
    },
    state::{HostRegistry, IntegrationLifecycle, StateError, StateRoot},
};

#[allow(unused_imports)]
pub(crate) use model::AppError;
#[allow(unused_imports)]
pub(crate) use observer::{ObserverActivation, prepare_observer_activation, remove_observer_exact};

/// Runs one direct local CLI command.
#[must_use]
pub fn run() -> ExitCode {
    let cli = cli::Cli::parse();
    let provider_surface = cli::is_provider_surface_command(cli.command.as_ref());
    match dispatch::execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        // These helpers execute inside a provider pane. They deliberately do
        // not expose CLI diagnostics there; normal navigator polling owns the
        // bounded state presentation after an attachment ends.
        Err(_) if provider_surface => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
