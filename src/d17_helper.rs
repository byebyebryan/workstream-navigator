//! Dormant D17 launch-helper state boundary.
//!
//! This is deliberately a no-provider-effect seam.  It models the handoff a
//! hidden helper will use after replacing the controlled provisional shell,
//! and leaves actual provider preparation, `execve`, and post-exec proof to
//! later provider-specific and reconciler slices.

#![allow(
    dead_code,
    reason = "the D17 launch helper remains unreachable until the atomic Navigator cutover"
)]

use thiserror::Error;

use crate::{
    d17_broker::{BrokerError, PrepareContext, consume, request_from_context},
    domain::{OperationId, ProviderKind, RuntimeId},
    state::d16::OnboardingOwnership,
    state::{D16State, ProvisionalLease, StateError},
};

/// The exact provider-specific preparation ownership transferred from a
/// successfully consumed shell handoff. It contains no command, token,
/// terminal content, environment, or provider payload.
pub(crate) struct ProviderPreparation {
    ownership: Box<OnboardingOwnership>,
    provider: ProviderKind,
}

impl std::fmt::Debug for ProviderPreparation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderPreparation")
            .field("ownership", &"<opaque>")
            .field("provider", &self.provider)
            .finish()
    }
}

impl ProviderPreparation {
    #[must_use]
    pub(crate) const fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub(crate) const fn operation_id(&self) -> OperationId {
        self.ownership.operation_id
    }

    #[must_use]
    pub(crate) const fn runtime_id(&self) -> RuntimeId {
        self.ownership.runtime_id
    }
}

/// The only D17 result that is eligible to cross the helper's final native
/// exec fence. It does not prove that a provider was ever executed.
pub(crate) struct ProviderExecFence {
    ownership: OnboardingOwnership,
    provider: ProviderKind,
}

/// Type-level proof that `OpenCode`'s non-idempotent preparation boundary was
/// durably recorded. Only this value can reach the `OpenCode` native-exec fence.
pub(crate) struct OpenCodeExternalEffectFence {
    ownership: Box<OnboardingOwnership>,
}

impl std::fmt::Debug for OpenCodeExternalEffectFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenCodeExternalEffectFence")
            .field("ownership", &"<opaque>")
            .finish()
    }
}

/// Type-level proof that the session returned after `OpenCode`'s potential
/// external effect is durably bound to the exact Runtime. Only this proof may
/// cross the final native-exec fence.
pub(crate) struct OpenCodeSessionFence {
    ownership: Box<OnboardingOwnership>,
    session: crate::domain::ProviderSessionId,
}

impl std::fmt::Debug for OpenCodeSessionFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenCodeSessionFence")
            .field("ownership", &"<opaque>")
            .field("session", &"<opaque>")
            .finish()
    }
}

impl OpenCodeSessionFence {
    /// Returns the opaque native session only to the final direct provider
    /// command builder. It is never exposed through snapshots or diagnostics.
    #[must_use]
    pub(crate) fn session(&self) -> &crate::domain::ProviderSessionId {
        &self.session
    }
}

impl std::fmt::Debug for ProviderExecFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderExecFence")
            .field("ownership", &"<opaque>")
            .field("provider", &self.provider)
            .finish()
    }
}

impl ProviderExecFence {
    #[must_use]
    pub(crate) const fn operation_id(&self) -> OperationId {
        self.ownership.operation_id
    }

    #[must_use]
    pub(crate) const fn runtime_id(&self) -> RuntimeId {
        self.ownership.runtime_id
    }
}

/// Bounded launch-helper errors. They intentionally disclose no provider
/// command, token, process data, or private path.
#[derive(Debug, Error)]
pub(crate) enum HelperError {
    #[error("D17 shell handoff is unavailable")]
    Broker(#[from] BrokerError),
    #[error("D17 provider launch state is unavailable")]
    State(#[from] StateError),
    #[error("only OpenCode may enter the D17 external-effect phase")]
    ExternalEffectProviderMismatch,
    #[error("only Codex may move directly from preparation to native exec")]
    CodexPreparationProviderMismatch,
}

/// Consumes the exact one-shot shell capability, transfers cleanup authority,
/// and commits the durable provider-preparation fence. No provider is
/// inspected, started, attached, signalled, or executed here.
pub(crate) fn begin_provider_preparation(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    token: &str,
    now_monotonic_millis: i64,
) -> Result<ProviderPreparation, HelperError> {
    let ownership = consume(
        state,
        provisional_lease,
        context,
        token,
        now_monotonic_millis,
    )?;
    let (_, request) = request_from_context(state, provisional_lease, context)?;
    let ownership =
        state.record_d17_provider_preparation_current(provisional_lease, &request, ownership)?;
    Ok(ProviderPreparation {
        ownership: Box::new(ownership),
        provider: context.provider,
    })
}

/// Records `OpenCode`'s non-idempotent external-effect fence before a future
/// adapter could attempt its blank-session POST. This method has no HTTP or
/// provider side effect.
pub(crate) fn record_opencode_external_effect_started(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    preparation: ProviderPreparation,
) -> Result<OpenCodeExternalEffectFence, HelperError> {
    let ProviderPreparation {
        ownership,
        provider,
    } = preparation;
    if provider != ProviderKind::OpenCode {
        return Err(HelperError::ExternalEffectProviderMismatch);
    }
    let (_, request) = request_from_context(state, provisional_lease, context)?;
    let ownership = state.record_d17_provider_external_effect_started_current(
        provisional_lease,
        &request,
        *ownership,
    )?;
    Ok(OpenCodeExternalEffectFence {
        ownership: Box::new(ownership),
    })
}

/// Binds the exact blank `OpenCode` session that the already-recorded POST
/// returned. An error after the POST must be reconciled as a possible external
/// effect; this method never retries or creates another session.
pub(crate) fn record_opencode_created_session(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    effect_fence: OpenCodeExternalEffectFence,
    session: crate::domain::ProviderSessionId,
) -> Result<OpenCodeSessionFence, HelperError> {
    let OpenCodeExternalEffectFence { ownership } = effect_fence;
    let (_, request) = request_from_context(state, provisional_lease, context)?;
    let ownership = state.record_d17_opencode_created_session_current(
        provisional_lease,
        &request,
        *ownership,
        &session,
    )?;
    Ok(OpenCodeSessionFence {
        ownership: Box::new(ownership),
        session,
    })
}

/// Records Codex's final native-exec fence. Returning successfully still
/// proves no provider execution.
pub(crate) fn record_codex_provider_exec_started(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    preparation: ProviderPreparation,
) -> Result<ProviderExecFence, HelperError> {
    let ProviderPreparation {
        ownership,
        provider,
    } = preparation;
    if provider != ProviderKind::Codex {
        return Err(HelperError::CodexPreparationProviderMismatch);
    }
    let (_, request) = request_from_context(state, provisional_lease, context)?;
    let ownership =
        state.record_d17_provider_exec_started_current(provisional_lease, &request, *ownership)?;
    Ok(ProviderExecFence {
        ownership,
        provider,
    })
}

/// Records `OpenCode`'s final native-exec fence only after its typed
/// external-effect boundary. Returning successfully still proves no provider
/// execution.
pub(crate) fn record_opencode_provider_exec_started(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    session_fence: OpenCodeSessionFence,
) -> Result<ProviderExecFence, HelperError> {
    let OpenCodeSessionFence { ownership, .. } = session_fence;
    let (_, request) = request_from_context(state, provisional_lease, context)?;
    let ownership =
        state.record_d17_provider_exec_started_current(provisional_lease, &request, *ownership)?;
    Ok(ProviderExecFence {
        ownership,
        provider: ProviderKind::OpenCode,
    })
}

/// Advances a revalidated Codex handoff through the exact durable fences
/// immediately before its direct native exec. `OpenCode` cannot use this path:
/// it must bind the blank session returned after its separately fenced POST
/// before it may record the final native-exec boundary.
pub(crate) fn advance_codex_to_provider_exec_fence(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    token: &str,
    now_monotonic_millis: i64,
) -> Result<ProviderExecFence, HelperError> {
    if context.provider != ProviderKind::Codex {
        return Err(HelperError::CodexPreparationProviderMismatch);
    }
    let preparation = begin_provider_preparation(
        state,
        provisional_lease,
        context,
        token,
        now_monotonic_millis,
    )?;
    record_codex_provider_exec_started(state, provisional_lease, context, preparation)
}

/// Records a final direct-Codex-`execve` failure only after the caller has
/// received the operating-system error proving that no provider process was
/// created. The consumed fence is intentionally not reusable.
#[allow(
    clippy::needless_pass_by_value,
    reason = "moving the opaque fence prevents a caller from reusing the pre-exec authority"
)]
pub(crate) fn record_codex_exec_failed_known_absent(
    state: &mut D16State,
    provisional_lease: &ProvisionalLease,
    context: &PrepareContext<'_, '_>,
    exec_fence: ProviderExecFence,
) -> Result<(), HelperError> {
    let ProviderExecFence {
        ownership,
        provider,
    } = exec_fence;
    if provider != ProviderKind::Codex {
        return Err(HelperError::CodexPreparationProviderMismatch);
    }
    let (_, request) = request_from_context(state, provisional_lease, context)?;
    state.record_d17_codex_exec_failed_known_absent_current(
        provisional_lease,
        &request,
        ownership,
    )?;
    Ok(())
}
