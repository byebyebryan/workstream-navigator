//! Provider-neutral bounded lifecycle evidence.

/// The lifecycle events understood by the shared state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleEvent {
    SessionStart,
    UserPromptSubmit,
    Stop,
    SessionEnd,
}

/// Minimal lifecycle evidence retained after a concrete provider payload is
/// parsed and bounded. Provider adapters own payload parsing; shared state
/// consumes only this neutral DTO.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleObservation {
    pub event: LifecycleEvent,
    pub cwd: String,
    pub native_session_id: String,
    pub turn_id: Option<String>,
    pub source: Option<String>,
}
