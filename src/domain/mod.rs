use std::{
    fmt,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        #[allow(
            clippy::new_without_default,
            reason = "a randomly generated identity is never a meaningful default value"
        )]
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            #[must_use]
            pub fn short(self) -> String {
                self.0.simple().to_string()[..8].to_owned()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

opaque_id!(HostId);
opaque_id!(ProjectId);
opaque_id!(LocationId);
opaque_id!(WorkstreamId);
opaque_id!(CheckoutId);
opaque_id!(RuntimeId);
opaque_id!(BindingId);
opaque_id!(OperationId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Revision(i64);

impl Revision {
    pub const INITIAL: Self = Self(1);

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl TryFrom<i64> for Revision {
    type Error = DomainError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 1 {
            return Err(DomainError::InvalidRevision(value));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkstreamLifecycle {
    Open,
    Parked,
    RecoveryRequired,
}

/// The one explicit creation lineage of a Workstream.
///
/// This is durable operational metadata, not a task concept and not a
/// user-facing display name. A native `/clear` changes only the current
/// conversation tip; it never changes this filesystem/runtime lineage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkstreamOrigin {
    External,
    Independent,
    Fork,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Starting,
    Idle,
    Working,
    Attention,
    Stopped,
    Unknown,
    Unreachable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Start,
    Fork,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Prepared,
    ExternalEffectStarted,
    AwaitingReconciliation,
    Committed,
    RecoveryRequired,
    Failed,
}

impl OperationPhase {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Committed | Self::RecoveryRequired | Self::Failed
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompoundOperation {
    pub id: OperationId,
    pub request_key: String,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub expected_revisions_json: String,
    pub effect_watermark: Option<String>,
    pub outcome_json: Option<String>,
    pub revision: Revision,
}

impl CompoundOperation {
    /// Creates a prepared, durably deduplicated external operation.
    ///
    /// # Errors
    ///
    /// Returns an error when the request key is empty or too large, or when the
    /// expected-revision snapshot is not a JSON object.
    pub fn new(
        request_key: String,
        kind: OperationKind,
        expected_revisions_json: String,
    ) -> Result<Self, DomainError> {
        Self::with_id(
            OperationId::new(),
            request_key,
            kind,
            expected_revisions_json,
        )
    }

    /// Creates a prepared operation with an injected identity.
    ///
    /// This is the deterministic seam used by state and recovery tests. Normal
    /// production callers should use [`Self::new`].
    ///
    /// # Errors
    ///
    /// Returns an error when the request key is empty or too large, or when the
    /// expected-revision snapshot is not a JSON object.
    pub fn with_id(
        id: OperationId,
        request_key: String,
        kind: OperationKind,
        expected_revisions_json: String,
    ) -> Result<Self, DomainError> {
        if request_key.trim().is_empty() {
            return Err(DomainError::EmptyRequestKey);
        }
        if request_key.len() > 128 {
            return Err(DomainError::RequestKeyTooLong);
        }
        if !serde_json::from_str::<serde_json::Value>(&expected_revisions_json)
            .is_ok_and(|value| value.is_object())
        {
            return Err(DomainError::ExpectedRevisionsMustBeObject);
        }

        Ok(Self {
            id,
            request_key,
            kind,
            phase: OperationPhase::Prepared,
            expected_revisions_json,
            effect_watermark: None,
            outcome_json: None,
            revision: Revision::INITIAL,
        })
    }

    /// Advances this operation through one permitted recovery phase.
    ///
    /// # Errors
    ///
    /// Returns an error when the phase change is not permitted or the optional
    /// outcome is not valid JSON.
    pub fn transition(
        &mut self,
        next: OperationPhase,
        effect_watermark: Option<String>,
        outcome_json: Option<String>,
    ) -> Result<(), DomainError> {
        if !permits_transition(self.phase, next) {
            return Err(DomainError::InvalidOperationTransition {
                from: self.phase,
                to: next,
            });
        }
        if let Some(outcome) = &outcome_json {
            serde_json::from_str::<serde_json::Value>(outcome)
                .map_err(DomainError::InvalidOperationOutcome)?;
        }

        self.phase = next;
        self.effect_watermark = effect_watermark;
        self.outcome_json = outcome_json;
        self.revision = self.revision.next();
        Ok(())
    }
}

const fn permits_transition(from: OperationPhase, to: OperationPhase) -> bool {
    matches!(
        (from, to),
        (
            OperationPhase::Prepared,
            OperationPhase::ExternalEffectStarted | OperationPhase::Failed
        ) | (
            OperationPhase::ExternalEffectStarted,
            OperationPhase::Committed
                | OperationPhase::AwaitingReconciliation
                | OperationPhase::RecoveryRequired
        ) | (
            OperationPhase::AwaitingReconciliation,
            OperationPhase::Committed | OperationPhase::RecoveryRequired | OperationPhase::Failed
        )
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttentionState {
    pub workstream_id: WorkstreamId,
    pub result_unseen_since_revision: Option<Revision>,
    pub recovery_unseen_since_revision: Option<Revision>,
    pub latest_native_session_id: Option<String>,
    pub latest_turn_id: Option<String>,
    pub revision: Revision,
}

impl AttentionState {
    #[must_use]
    pub fn new(workstream_id: WorkstreamId) -> Self {
        Self {
            workstream_id,
            result_unseen_since_revision: None,
            recovery_unseen_since_revision: None,
            latest_native_session_id: None,
            latest_turn_id: None,
            revision: Revision::INITIAL,
        }
    }

    /// Records a settled native result without clearing an existing unseen one.
    ///
    /// # Errors
    ///
    /// Returns an error when either provider identifier is empty, oversized, or
    /// contains a line break.
    pub fn mark_result(&mut self, session_id: String, turn_id: String) -> Result<(), DomainError> {
        validate_provider_identifier(&session_id)?;
        validate_provider_identifier(&turn_id)?;
        let next = self.revision.next();
        if self.result_unseen_since_revision.is_none() {
            self.result_unseen_since_revision = Some(next);
        }
        self.latest_native_session_id = Some(session_id);
        self.latest_turn_id = Some(turn_id);
        self.revision = next;
        Ok(())
    }

    pub fn mark_recovery_required(&mut self) {
        let next = self.revision.next();
        if self.recovery_unseen_since_revision.is_none() {
            self.recovery_unseen_since_revision = Some(next);
        }
        self.revision = next;
    }

    /// Clears only the recovery-required signal while preserving any unseen
    /// native result. A verified native resume is the sole caller: ordinary
    /// navigator acknowledgement must not make an uncertain Runtime look
    /// healthy.
    pub fn clear_recovery_required(&mut self) {
        if self.recovery_unseen_since_revision.is_some() {
            self.recovery_unseen_since_revision = None;
            self.revision = self.revision.next();
        }
    }

    /// Clears result attention only when the caller saw the current revision.
    ///
    /// # Errors
    ///
    /// Returns a revision conflict when a newer observation has arrived.
    pub fn acknowledge_result(&mut self, expected: Revision) -> Result<(), DomainError> {
        if self.revision != expected {
            return Err(DomainError::RevisionConflict {
                expected,
                current: self.revision,
            });
        }
        self.result_unseen_since_revision = None;
        self.revision = self.revision.next();
        Ok(())
    }
}

fn validate_provider_identifier(value: &str) -> Result<(), DomainError> {
    if value.is_empty() || value.len() > 256 || value.contains('\n') || value.contains('\r') {
        return Err(DomainError::InvalidProviderIdentifier);
    }
    Ok(())
}

pub trait Clock: Send + Sync {
    /// Returns a wall-clock time in milliseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if the system clock predates the epoch or does not fit
    /// in the storage representation.
    fn now_millis(&self) -> Result<i64, DomainError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> Result<i64, DomainError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| DomainError::ClockBeforeUnixEpoch)?;
        i64::try_from(elapsed.as_millis()).map_err(|_| DomainError::ClockOverflow)
    }
}

pub trait IdGenerator: Send + Sync {
    fn uuid(&self) -> Uuid;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RandomIdGenerator;

impl IdGenerator for RandomIdGenerator {
    fn uuid(&self) -> Uuid {
        Uuid::new_v4()
    }
}

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("clock overflow")]
    ClockOverflow,
    #[error("system clock is before the Unix epoch")]
    ClockBeforeUnixEpoch,
    #[error("request key cannot be empty")]
    EmptyRequestKey,
    #[error("expected revisions must be a JSON object")]
    ExpectedRevisionsMustBeObject,
    #[error("invalid operation outcome JSON: {0}")]
    InvalidOperationOutcome(serde_json::Error),
    #[error("invalid operation phase transition from {from:?} to {to:?}")]
    InvalidOperationTransition {
        from: OperationPhase,
        to: OperationPhase,
    },
    #[error("invalid provider identifier")]
    InvalidProviderIdentifier,
    #[error("invalid revision {0}")]
    InvalidRevision(i64),
    #[error("revision conflict: expected {expected:?}, current {current:?}")]
    RevisionConflict {
        expected: Revision,
        current: Revision,
    },
    #[error("request key exceeds 128 bytes")]
    RequestKeyTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_operation_cannot_be_retried() {
        let mut operation =
            CompoundOperation::new("request-1".to_owned(), OperationKind::Fork, "{}".to_owned())
                .unwrap();
        operation
            .transition(OperationPhase::ExternalEffectStarted, None, None)
            .unwrap();
        operation
            .transition(OperationPhase::AwaitingReconciliation, None, None)
            .unwrap();
        operation
            .transition(OperationPhase::RecoveryRequired, None, None)
            .unwrap();

        assert!(matches!(
            operation.transition(OperationPhase::ExternalEffectStarted, None, None),
            Err(DomainError::InvalidOperationTransition { .. })
        ));
    }

    #[test]
    fn result_attention_is_sticky_and_revision_guarded() {
        let mut attention = AttentionState::new(WorkstreamId::new());
        attention
            .mark_result("session-a".to_owned(), "turn-a".to_owned())
            .unwrap();
        let first_unseen = attention.result_unseen_since_revision;
        attention
            .mark_result("session-a".to_owned(), "turn-b".to_owned())
            .unwrap();

        assert_eq!(attention.result_unseen_since_revision, first_unseen);
        assert!(matches!(
            attention.acknowledge_result(Revision::INITIAL),
            Err(DomainError::RevisionConflict { .. })
        ));
    }

    #[test]
    fn operation_phases_model_confirmed_and_ambiguous_external_effects() {
        let mut confirmed = CompoundOperation::new(
            "start-confirmed".to_owned(),
            OperationKind::Start,
            "{}".to_owned(),
        )
        .unwrap();
        confirmed
            .transition(
                OperationPhase::ExternalEffectStarted,
                Some("launch-recorded".to_owned()),
                None,
            )
            .unwrap();
        confirmed
            .transition(
                OperationPhase::Committed,
                Some("launch-recorded".to_owned()),
                Some("{\"runtime\":\"confirmed\"}".to_owned()),
            )
            .unwrap();
        assert!(confirmed.phase.is_terminal());

        let mut ambiguous = CompoundOperation::new(
            "fork-lost-response".to_owned(),
            OperationKind::Fork,
            "{}".to_owned(),
        )
        .unwrap();
        ambiguous
            .transition(
                OperationPhase::ExternalEffectStarted,
                Some("provider-fork-issued".to_owned()),
                None,
            )
            .unwrap();
        ambiguous
            .transition(
                OperationPhase::AwaitingReconciliation,
                Some("provider-fork-issued".to_owned()),
                None,
            )
            .unwrap();
        ambiguous
            .transition(
                OperationPhase::RecoveryRequired,
                Some("provider-fork-issued".to_owned()),
                Some("{\"reason\":\"no_unique_candidate\"}".to_owned()),
            )
            .unwrap();
        assert!(ambiguous.phase.is_terminal());
    }

    #[test]
    fn injected_operation_identity_makes_recovery_fixtures_deterministic() {
        let id = OperationId::from(Uuid::from_u128(42));
        let operation = CompoundOperation::with_id(
            id,
            "fixture-operation".to_owned(),
            OperationKind::Start,
            "{}".to_owned(),
        )
        .unwrap();

        assert_eq!(operation.id, id);
    }

    #[test]
    fn malformed_operation_snapshot_fails_before_an_external_effect() {
        assert!(matches!(
            CompoundOperation::new(
                "start-invalid".to_owned(),
                OperationKind::Start,
                "[]".to_owned()
            ),
            Err(DomainError::ExpectedRevisionsMustBeObject)
        ));
    }
}
