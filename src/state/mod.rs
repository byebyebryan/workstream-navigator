mod attention;
mod compound;
pub mod d16;
mod host;
mod lifecycle;
mod models;
mod runtime;
mod schema;
mod utils;
mod workstream;

#[cfg(test)]
mod tests;

pub use d16::{
    CurrentObserverHandleProof, D16_HOST_SCHEMA_VERSION, D16_SCHEMA_12_VERSION, D16OpenMode,
    D16State, FreshRootClassification, FreshRootRejection, HandoverPhase, HandoverRestartAction,
    LEGACY_CLIENT_DATABASE_FILE, LEGACY_CLIENT_DATABASE_SHM_FILE, LEGACY_CLIENT_DATABASE_WAL_FILE,
    OBSERVER_HANDOVER_ACTIVATION_ACK_FILE, OBSERVER_HANDOVER_ACTIVATION_ACK_TEMP_FILE,
    OBSERVER_HANDOVER_JOURNAL_FILE, OBSERVER_HANDOVER_JOURNAL_TEMP_FILE, ObserverDatabaseDeadline,
    ObserverDatabaseError, ObserverDegradedReason, ObserverHandoverActivationAck,
    ObserverHandoverJournal, ObserverProcessIdentity, OpenCodeObserverProjection,
    ProjectBrowserRootRevision, ProjectLocationProjection, ProjectLocationRegistration,
    ProjectLocationWorkstreamRegistration, ProjectProjection, ProjectRecord, ProjectRefreshInput,
    ProjectRefreshMember, ProjectRefreshObservation, ProjectRefreshOutcome, StateRecoveryReason,
    TRANSITION_LOCK_FILE, TransitionLease, acquire_transition_lease, classify_fresh_root,
    clear_observer_degraded_marker, exact_schema_12_fixture_sql, fresh_create,
    observer_degraded_marker_path, observer_handover_activation_ack_path,
    observer_handover_activation_ack_temp_path, observer_handover_journal_path,
    observer_handover_journal_temp_path, open_confirmed_cutover, open_current_only,
    open_cutover_transition, open_observer_transition, read_observer_degraded_marker,
    read_observer_handover_activation_ack, read_observer_handover_journal,
    recover_observer_handover_journal, run_observer_write_with_degraded_marker,
    write_observer_degraded_marker, write_observer_handover_activation_ack,
    write_observer_handover_journal,
};
pub use models::MAX_PROJECT_BROWSER_ENTRIES;
pub use models::{
    CodexIntegration, CreatedWorkstream, EXTERNAL_EFFECT_UNKNOWN_CODE, ExternalWorkstream,
    ForkPlan, ForkPreparation, HostIdentity, HostRegistry, IntegrationLifecycle,
    OpenCodeLifecycleObservation, OpenCodeObserverStatus, OpenCodeRuntimeHandle,
    OpenCodeSessionCreationOperation, OperationOverview, ProjectDirectoriesResponse,
    ProjectDirectoryEntry, ProviderBinding, RuntimeRecord, StateError, StateRoot,
    WorkstreamOverview,
};
pub use schema::HOST_SCHEMA_VERSION;
