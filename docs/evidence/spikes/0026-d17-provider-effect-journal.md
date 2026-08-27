# Spike 0026: D17 provider-effect journal

## Question

Can D17 retain its action fence and exact recovery boundary through provider
preparation: Codex's conclusive no-effect path and OpenCode's non-idempotent
blank-session `POST /session` with both known-binding and ambiguous outcomes?

## Procedure and isolation

The deterministic harness is
[`spikes/d17-provider-effect-journal.py`](../../../spikes/d17-provider-effect-journal.py),
with its sanitized [fixture][fixture]. It creates a mode-`0700` temporary root
and private JSON journals. It launches no coding provider, tmux server, shell,
or real OpenCode endpoint.

For OpenCode it starts an in-process loopback fake endpoint. The known case
returns one synthetic blank-session binding; the ambiguous case consumes the
request then closes before returning an HTTP response. The journal persists
`opencode_post_attempted` with an unknown outcome before either request, then
records the known binding only after the exact response. Codex uses a no-effect
readiness model. The harness exercises promotion, pre-effect helper crash,
exec-start fencing, exact exec proof, exact `execve`-error classification, and
passive recovery.

All endpoint state, journal values, request/runtime identifiers, and paths are
removed with the temporary root. The fixture preserves only the contract label
and boolean outcomes.

## Result

The fixture passed.

- Durable `runtime_owned_launching` and `provider_exec_started` stay action
  fenced; only exact exec proof releases normal action authority.
- Codex's known-absent final exec error rolls back only the no-effect attempt;
  a pre-effect helper crash can be passively reconciled to that same outcome.
- OpenCode records an attempted POST before sending it. A known response keeps
  the binding and ends a later known-absent exec error in stopped/recovery
  state, with no second POST.
- A response-ambiguous POST remains recovery-required. Passive recovery and a
  new preparation request are both unable to issue another POST.
- Duplicate/stale phase transitions refuse.

## Consequence

The production onboarding journal must split final provider exec absence from
prior provider effects. A known OpenCode binding is binding-preserving recovery,
and any possible POST is durable uncertainty—not permission to retry as a clean
session creation.

## Limits

- This is a synthetic journal and fake HTTP endpoint, not the Rust state,
  helper, live OpenCode server, or native Codex observer.
- It does not prove marker/process/revision/capability revalidation, actual
  exec, signal delivery, provider terminal behavior, or restart against the
  real database and private tmux server.

## Status

**Provider-effect ordering model validated; D17 production integration and
operator-gated live promotion remain required.**

[fixture]: ../../../spikes/fixtures/d17-provider-effect-journal.json
