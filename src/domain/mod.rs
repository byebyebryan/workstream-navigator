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
opaque_id!(RuntimeId);
opaque_id!(BindingId);
opaque_id!(OperationId);

/// The fixed provider identity of a Workstream lane.
///
/// Provider identity is persisted and carried through every authoritative
/// state operation. Unknown values are rejected rather than treated as a
/// fallback provider.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Codex,
    OpenCode,
}

impl ProviderKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderKind {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            _ => Err(DomainError::UnknownProviderKind(value.to_owned())),
        }
    }
}

/// A bounded native provider-session identity namespaced by provider kind.
///
/// Native identifiers are opaque to Workstream Navigator. The provider kind
/// is part of the identity so equal native strings from different providers
/// can never be confused by state or recovery logic.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct ProviderSessionId {
    provider: ProviderKind,
    native_session_id: String,
}

impl<'de> Deserialize<'de> for ProviderSessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: ProviderKind,
            native_session_id: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.provider, wire.native_session_id).map_err(serde::de::Error::custom)
    }
}

impl ProviderSessionId {
    /// Constructs a validated namespaced native session identity.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or line-delimited native
    /// identifier.
    pub fn new(
        provider: ProviderKind,
        native_session_id: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let native_session_id = native_session_id.into();
        validate_provider_identifier(&native_session_id)?;
        Ok(Self {
            provider,
            native_session_id,
        })
    }

    /// Constructs a Codex-native session identity at the provider boundary.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid native identifier.
    pub fn codex(native_session_id: impl Into<String>) -> Result<Self, DomainError> {
        Self::new(ProviderKind::Codex, native_session_id)
    }

    #[must_use]
    pub const fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub fn native_id(&self) -> &str {
        &self.native_session_id
    }
}

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
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Onboard,
    Start,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Prepared,
    CapabilityIssued,
    RuntimeOwnedLaunching,
    ProviderPreparation,
    ExternalEffectStarted,
    ProviderExecStarted,
    ProviderExecProven,
    ExecFailedKnownAbsent,
    RolledBack,
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
            Self::Committed | Self::Failed | Self::ProviderExecProven | Self::RolledBack
        )
    }
}

/// The provider-exec lifecycle for one shell-promotion attempt.
///
/// This is deliberately separate from the existing `Start`
/// operation phases: promotion has a durable ownership boundary before a
/// provider effect, and no ordinary action authority exists until exact exec
/// proof or recovery resolves it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingPhase {
    Prepared,
    CapabilityIssued,
    RuntimeOwnedLaunching,
    ProviderPreparation,
    ProviderExternalEffectStarted,
    ProviderExecStarted,
    ProviderExecProven,
    KnownAbsentExec,
    RecoveryRequired,
    RolledBack,
}

impl OnboardingPhase {
    /// Maps the durable compound-operation phase used by the onboarding journal to
    /// the onboarding-only transition contract.
    #[must_use]
    pub const fn from_operation_phase(phase: OperationPhase) -> Option<Self> {
        match phase {
            OperationPhase::Prepared => Some(Self::Prepared),
            OperationPhase::CapabilityIssued => Some(Self::CapabilityIssued),
            OperationPhase::RuntimeOwnedLaunching => Some(Self::RuntimeOwnedLaunching),
            OperationPhase::ProviderPreparation => Some(Self::ProviderPreparation),
            OperationPhase::ExternalEffectStarted => Some(Self::ProviderExternalEffectStarted),
            OperationPhase::ProviderExecStarted => Some(Self::ProviderExecStarted),
            OperationPhase::ProviderExecProven => Some(Self::ProviderExecProven),
            OperationPhase::ExecFailedKnownAbsent => Some(Self::KnownAbsentExec),
            OperationPhase::RecoveryRequired => Some(Self::RecoveryRequired),
            OperationPhase::RolledBack => Some(Self::RolledBack),
            OperationPhase::AwaitingReconciliation
            | OperationPhase::Committed
            | OperationPhase::Failed => None,
        }
    }

    /// Returns the exact durable compound-operation phase for this
    /// onboarding state.
    #[must_use]
    pub const fn operation_phase(self) -> OperationPhase {
        match self {
            Self::Prepared => OperationPhase::Prepared,
            Self::CapabilityIssued => OperationPhase::CapabilityIssued,
            Self::RuntimeOwnedLaunching => OperationPhase::RuntimeOwnedLaunching,
            Self::ProviderPreparation => OperationPhase::ProviderPreparation,
            Self::ProviderExternalEffectStarted => OperationPhase::ExternalEffectStarted,
            Self::ProviderExecStarted => OperationPhase::ProviderExecStarted,
            Self::ProviderExecProven => OperationPhase::ProviderExecProven,
            Self::KnownAbsentExec => OperationPhase::ExecFailedKnownAbsent,
            Self::RecoveryRequired => OperationPhase::RecoveryRequired,
            Self::RolledBack => OperationPhase::RolledBack,
        }
    }

    /// Returns whether a Runtime in this phase must refuse ordinary
    /// attach/action authority.
    #[must_use]
    pub const fn action_fenced(self) -> bool {
        !matches!(
            self,
            Self::Prepared | Self::CapabilityIssued | Self::ProviderExecProven | Self::RolledBack
        )
    }

    /// Returns whether this phase cannot make further progress without exact
    /// recovery evidence.
    #[must_use]
    pub const fn requires_reconciliation(self) -> bool {
        matches!(self, Self::KnownAbsentExec | Self::RecoveryRequired)
    }

    /// Validates one monotonic onboarding transition.
    ///
    /// # Errors
    ///
    /// Returns an error when a caller tries to bypass the one-shot capability,
    /// Runtime-ownership, or provider-exec evidence boundaries.
    pub fn transition(self, next: Self) -> Result<Self, DomainError> {
        if permits_onboarding_transition(self, next) {
            Ok(next)
        } else {
            Err(DomainError::InvalidOnboardingTransition {
                from: self,
                to: next,
            })
        }
    }
}

const fn permits_onboarding_transition(from: OnboardingPhase, to: OnboardingPhase) -> bool {
    matches!(
        (from, to),
        (
            OnboardingPhase::Prepared,
            OnboardingPhase::CapabilityIssued | OnboardingPhase::RolledBack
        ) | (
            OnboardingPhase::CapabilityIssued,
            OnboardingPhase::RuntimeOwnedLaunching | OnboardingPhase::RolledBack
        ) | (
            OnboardingPhase::RuntimeOwnedLaunching,
            OnboardingPhase::ProviderPreparation | OnboardingPhase::RecoveryRequired
        ) | (
            OnboardingPhase::ProviderPreparation,
            OnboardingPhase::ProviderExternalEffectStarted
                | OnboardingPhase::ProviderExecStarted
                | OnboardingPhase::RecoveryRequired
        ) | (
            OnboardingPhase::ProviderExternalEffectStarted,
            OnboardingPhase::ProviderExecStarted | OnboardingPhase::RecoveryRequired
        ) | (
            OnboardingPhase::ProviderExecStarted,
            OnboardingPhase::ProviderExecProven
                | OnboardingPhase::KnownAbsentExec
                | OnboardingPhase::RecoveryRequired
        ) | (
            OnboardingPhase::KnownAbsentExec,
            OnboardingPhase::RolledBack | OnboardingPhase::RecoveryRequired
        ) | (
            OnboardingPhase::RecoveryRequired,
            OnboardingPhase::ProviderExecProven | OnboardingPhase::RolledBack
        )
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompoundOperation {
    pub id: OperationId,
    pub request_key: String,
    pub kind: OperationKind,
    pub phase: OperationPhase,
    pub expected_revisions_json: String,
    /// -only one-shot capability metadata. current operations retain `None`
    /// for every field and never query schema-15 columns.
    pub launch_token_id: Option<String>,
    pub launch_token_verifier: Option<String>,
    pub launch_token_expiry_monotonic: Option<i64>,
    pub launch_claims_digest: Option<String>,
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
            launch_token_id: None,
            launch_token_verifier: None,
            launch_token_expiry_monotonic: None,
            launch_claims_digest: None,
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
        if self.kind == OperationKind::Onboard {
            return Err(DomainError::OnboardingOperationRequired);
        }
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

    /// Returns the onboarding-specific phase only for an onboarding operation.
    ///
    /// # Errors
    ///
    /// Returns an error when a Start operation is treated as onboarding
    /// state or the persisted phase is outside the onboarding lifecycle.
    pub fn onboarding_phase(&self) -> Result<OnboardingPhase, DomainError> {
        if self.kind != OperationKind::Onboard {
            return Err(DomainError::OnboardingOperationRequired);
        }
        OnboardingPhase::from_operation_phase(self.phase)
            .ok_or(DomainError::InvalidOnboardingOperationPhase(self.phase))
    }

    /// Advances one onboarding operation through the ownership and
    /// provider-exec state machine without permitting the generic Start
    /// transition graph to bypass it.
    ///
    /// # Errors
    ///
    /// Returns an error when the operation is not onboarding state, the phase
    /// change skips an authority boundary, or the optional outcome is invalid
    /// JSON.
    pub fn transition_onboarding(
        &mut self,
        next: OnboardingPhase,
        effect_watermark: Option<String>,
        outcome_json: Option<String>,
    ) -> Result<(), DomainError> {
        let current = self.onboarding_phase()?;
        current.transition(next)?;
        if let Some(outcome) = &outcome_json {
            serde_json::from_str::<serde_json::Value>(outcome)
                .map_err(DomainError::InvalidOperationOutcome)?;
        }
        self.phase = next.operation_phase();
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
                | OperationPhase::Failed
        ) | (
            OperationPhase::AwaitingReconciliation,
            OperationPhase::Committed | OperationPhase::RecoveryRequired | OperationPhase::Failed
        ) | (OperationPhase::RecoveryRequired, OperationPhase::Committed)
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttentionState {
    pub workstream_id: WorkstreamId,
    pub result_unseen_since_revision: Option<Revision>,
    pub recovery_unseen_since_revision: Option<Revision>,
    pub latest_native_session_id: Option<ProviderSessionId>,
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
    pub fn mark_result(
        &mut self,
        session_id: ProviderSessionId,
        turn_id: String,
    ) -> Result<(), DomainError> {
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
    #[error("invalid onboarding phase transition from {from:?} to {to:?}")]
    InvalidOnboardingTransition {
        from: OnboardingPhase,
        to: OnboardingPhase,
    },
    #[error("the operation is not an onboarding operation")]
    OnboardingOperationRequired,
    #[error("the compound operation phase is not valid for onboarding: {0:?}")]
    InvalidOnboardingOperationPhase(OperationPhase),
    #[error("invalid provider identifier")]
    InvalidProviderIdentifier,
    #[error("unknown provider kind: {0}")]
    UnknownProviderKind(String),
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
    fn provider_kind_parses_only_known_representations() {
        assert_eq!(
            "codex".parse::<ProviderKind>().unwrap(),
            ProviderKind::Codex
        );
        assert_eq!(
            "opencode".parse::<ProviderKind>().unwrap(),
            ProviderKind::OpenCode
        );
        assert!(matches!(
            "unknown".parse::<ProviderKind>(),
            Err(DomainError::UnknownProviderKind(value)) if value == "unknown"
        ));
        assert_eq!(
            serde_json::to_string(&ProviderKind::Codex).unwrap(),
            "\"codex\""
        );
    }

    #[test]
    fn provider_session_id_is_bounded_and_namespaced() {
        let id = ProviderSessionId::new(ProviderKind::OpenCode, "same-native-id").unwrap();
        assert_eq!(id.provider(), ProviderKind::OpenCode);
        assert_eq!(id.native_id(), "same-native-id");
        let codex = ProviderSessionId::codex("same-native-id").unwrap();
        assert_ne!(id, codex);
        assert!(serde_json::from_str::<ProviderSessionId>("\"same-native-id\"").is_err());
        assert!(
            serde_json::from_str::<ProviderSessionId>(
                "{\"provider\":\"codex\",\"native_session_id\":\"\"}"
            )
            .is_err()
        );
        assert!(matches!(
            ProviderSessionId::codex(""),
            Err(DomainError::InvalidProviderIdentifier)
        ));
        assert!(matches!(
            ProviderSessionId::codex("line\nbreak"),
            Err(DomainError::InvalidProviderIdentifier)
        ));
    }

    #[test]
    fn recovery_required_operation_can_only_finish() {
        let mut operation = CompoundOperation::new(
            "request-1".to_owned(),
            OperationKind::Start,
            "{}".to_owned(),
        )
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
        operation
            .transition(OperationPhase::Committed, None, None)
            .unwrap();
        assert!(operation.phase.is_terminal());
    }

    #[test]
    fn onboarding_phases_fence_actions_until_exact_exec_proof() {
        let mut phase = OnboardingPhase::Prepared;
        assert!(!phase.action_fenced());
        phase = phase.transition(OnboardingPhase::CapabilityIssued).unwrap();
        assert!(!phase.action_fenced());
        phase = phase
            .transition(OnboardingPhase::RuntimeOwnedLaunching)
            .unwrap();
        assert!(phase.action_fenced());
        phase = phase
            .transition(OnboardingPhase::ProviderPreparation)
            .unwrap();
        phase = phase
            .transition(OnboardingPhase::ProviderExecStarted)
            .unwrap();
        phase = phase
            .transition(OnboardingPhase::ProviderExecProven)
            .unwrap();
        assert!(!phase.action_fenced());
        assert!(!phase.requires_reconciliation());
    }

    #[test]
    fn onboarding_refuses_to_skip_capability_or_exec_proof() {
        assert!(matches!(
            OnboardingPhase::Prepared.transition(OnboardingPhase::RuntimeOwnedLaunching),
            Err(DomainError::InvalidOnboardingTransition { .. })
        ));
        assert!(matches!(
            OnboardingPhase::ProviderExecStarted.transition(OnboardingPhase::RolledBack),
            Err(DomainError::InvalidOnboardingTransition { .. })
        ));
    }

    #[test]
    fn onboarding_known_absence_and_ambiguity_remain_fenced_for_reconciliation() {
        let known_absent = OnboardingPhase::ProviderExecStarted
            .transition(OnboardingPhase::KnownAbsentExec)
            .unwrap();
        assert!(known_absent.action_fenced());
        assert!(known_absent.requires_reconciliation());
        assert_eq!(
            known_absent
                .transition(OnboardingPhase::RolledBack)
                .unwrap(),
            OnboardingPhase::RolledBack
        );

        let ambiguous = OnboardingPhase::ProviderPreparation
            .transition(OnboardingPhase::RecoveryRequired)
            .unwrap();
        assert!(ambiguous.action_fenced());
        assert!(ambiguous.requires_reconciliation());
        assert_eq!(
            ambiguous
                .transition(OnboardingPhase::ProviderExecProven)
                .unwrap(),
            OnboardingPhase::ProviderExecProven
        );
    }

    #[test]
    fn onboarding_compound_operation_uses_only_the_onboarding_transition_graph() {
        let mut operation = CompoundOperation::with_id(
            OperationId::from(Uuid::from_u128(84)),
            "onboard-84".to_owned(),
            OperationKind::Onboard,
            "{}".to_owned(),
        )
        .unwrap();
        assert_eq!(
            operation.onboarding_phase().unwrap(),
            OnboardingPhase::Prepared
        );
        assert!(matches!(
            operation.transition(OperationPhase::ExternalEffectStarted, None, None),
            Err(DomainError::OnboardingOperationRequired)
        ));
        operation
            .transition_onboarding(OnboardingPhase::CapabilityIssued, None, None)
            .unwrap();
        assert_eq!(operation.phase, OperationPhase::CapabilityIssued);
        operation
            .transition_onboarding(OnboardingPhase::RuntimeOwnedLaunching, None, None)
            .unwrap();
        assert_eq!(
            operation.onboarding_phase().unwrap(),
            OnboardingPhase::RuntimeOwnedLaunching
        );
        assert!(matches!(
            CompoundOperation::new("start-84".to_owned(), OperationKind::Start, "{}".to_owned())
                .unwrap()
                .onboarding_phase(),
            Err(DomainError::OnboardingOperationRequired)
        ));
    }

    #[test]
    fn result_attention_is_sticky_and_revision_guarded() {
        let mut attention = AttentionState::new(WorkstreamId::new());
        attention
            .mark_result(
                ProviderSessionId::codex("session-a").unwrap(),
                "turn-a".to_owned(),
            )
            .unwrap();
        let first_unseen = attention.result_unseen_since_revision;
        attention
            .mark_result(
                ProviderSessionId::codex("session-a").unwrap(),
                "turn-b".to_owned(),
            )
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
            "start-lost-response".to_owned(),
            OperationKind::Start,
            "{}".to_owned(),
        )
        .unwrap();
        ambiguous
            .transition(
                OperationPhase::ExternalEffectStarted,
                Some("provider-effect-issued".to_owned()),
                None,
            )
            .unwrap();
        ambiguous
            .transition(
                OperationPhase::AwaitingReconciliation,
                Some("provider-effect-issued".to_owned()),
                None,
            )
            .unwrap();
        ambiguous
            .transition(
                OperationPhase::RecoveryRequired,
                Some("provider-effect-issued".to_owned()),
                Some("{\"reason\":\"no_unique_candidate\"}".to_owned()),
            )
            .unwrap();
        assert!(!ambiguous.phase.is_terminal());
        ambiguous
            .transition(OperationPhase::Committed, None, None)
            .unwrap();
        assert!(ambiguous.phase.is_terminal());
    }

    #[test]
    fn external_effect_can_terminally_record_an_unknown_provider_result() {
        let mut operation = CompoundOperation::new(
            "start-unknown".to_owned(),
            OperationKind::Start,
            "{}".to_owned(),
        )
        .unwrap();
        operation
            .transition(OperationPhase::ExternalEffectStarted, None, None)
            .unwrap();
        operation
            .transition(
                OperationPhase::Failed,
                Some("provider-effect-issued".to_owned()),
                Some("{\"code\":\"external_effect_unknown\"}".to_owned()),
            )
            .unwrap();
        assert!(operation.phase.is_terminal());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(operation.outcome_json.as_deref().unwrap())
                .unwrap()["code"],
            "external_effect_unknown"
        );
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
