mod attention;
mod compound;
pub(crate) mod current;
mod host;
mod lifecycle;
mod models;
mod runtime;
mod schema;
mod utils;
mod workstream;

#[cfg(test)]
mod current_state_tests;

pub use current::{
    BOOTSTRAP_LOCK_FILE, BootstrapPhase, CurrentState, FreshRootClassification, FreshRootRejection,
    ObserverDatabaseDeadline, ObserverDatabaseError, ObserverDegradedReason, PROVISIONAL_LOCK_FILE,
    ProjectLocationProjection, ProjectProjection, ProjectRecord, ProvisionalLease, StateMode,
    StateRecoveryReason, clear_observer_degraded_marker, create_current,
    observer_degraded_marker_path, open_current, read_observer_degraded_marker,
    run_observer_write_with_degraded_marker, write_observer_degraded_marker,
};
pub use models::{
    CodexIntegration, CreatedWorkstream, EXTERNAL_EFFECT_UNKNOWN_CODE, ForkPlan, ForkPreparation,
    HostIdentity, HostRegistry, IntegrationLifecycle, OpenCodeLifecycleObservation,
    OpenCodeObserverStatus, OpenCodeRuntimeHandle, OpenCodeSessionCreationOperation,
    OperationOverview, OperationOverviewPage, ProviderBinding, RuntimeRecord, StateError,
    StateRoot, WorkstreamOverview, WorkstreamOverviewPage,
};
pub use schema::{HOST_APPLICATION_ID, HOST_SCHEMA_VERSION};
