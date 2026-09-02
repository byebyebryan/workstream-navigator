//! Host-local lifecycle actions shared by the direct CLI and Navigator.
//!
//! These actions own native process effects. The CLI and Navigator only parse
//! intent and render outcomes; neither gets to reimplement launch or
//! private-tmux authority.

mod attachment;
mod cleanup;
mod creation;
mod lifecycle;
mod model;
mod providers;
mod start;

#[cfg(test)]
mod tests;

pub use attachment::preflight_attachment;
pub(crate) use attachment::preflight_attachment_read_only;
pub use creation::{fork_workstream, recover_managed_operation, start_independent_workstream};
pub use lifecycle::{archive, await_deliberate_park, park, restore};
pub use model::{ActionError, StartOutcome, reconcile_observer_trust};
pub use providers::{codex_launch_program, codex_recovery_program};
pub(crate) use start::spawn_runtime_opencode_observer;
pub use start::{reconcile_lost_runtimes, recover, start};

pub(super) use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub(super) use thiserror::Error;

pub(super) use crate::{
    domain::{
        OperationId, OperationKind, OperationPhase, ProviderKind, ProviderSessionId, Revision,
        RuntimeId, SystemClock, WorkstreamId, WorkstreamLifecycle,
    },
    provider::codex::app_server::{AppServerError, EphemeralAppServer, ForkReconciliation},
    provider::codex::profile::{ObserverProfile, ProfileError},
    provider::opencode::{
        self, OpenCodeClient, OpenCodeEndpoint, OpenCodeError, endpoint_owned_by_process,
    },
    runtime::{
        LinuxProcessProbe, NativeLaunch, PrivateRuntime, ProcessProbe, RuntimePaths, RuntimeProbe,
        SystemTmux, prove_owned_process_group, terminate_owned_observer_process,
        terminate_owned_provider_process,
    },
    state::{HostRegistry, IntegrationLifecycle, ProviderBinding, StateError},
};
