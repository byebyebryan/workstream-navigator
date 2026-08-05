# Spike 0016: opencode native runtime and observer contract

## Question

Can Workstream Navigator treat an opencode native TUI as a provider-owned
Runtime without taking over the provider pane, while observing only bounded
lifecycle metadata and preserving the exact running-source Fork boundary?

## Procedure and isolation

The disposable, operator-gated probe is
[`spikes/opencode-runtime-contract.py`](../../../spikes/opencode-runtime-contract.py).
It was run against opencode `1.18.11` with the installed OpenCode Go model.
The harness creates a temporary project, isolated XDG config/data/cache/state
roots, and copies the existing auth file into the temporary data root with
mode `0600`; the copy and all provider state are removed during cleanup. The
fixture contains no credentials, prompts, responses, terminal captures, or raw
event payloads.

The probe:

1. creates two disposable sessions with bounded marker prompts;
2. launches each session in its own private tmux server using the native TUI
   command and a distinct loopback port;
3. verifies `/global/health`, the pinned version, pane-to-provider process
   correlation, and the exact resumed session endpoint;
4. reads each Runtime's `/global/event` stream through a session-bound helper
   that retains only event type, session/message identifiers, role, finish,
   completion, and status metadata; content and other payload fields are
   discarded, and events for other (including child) sessions are ignored;
5. submits an event marker through the first Runtime's attached server and
   checks lifecycle events do not cross into the second Runtime; and
6. starts a deliberately slow source turn, forks at the last settled assistant
   message with `POST /session/:id/fork`, and checks the destination prefix,
   in-flight omission, distinct ID, and absent structural fork lineage.

## Result

The [sanitized fixture][fixture] is a pass:

- each native TUI starts its own embedded loopback server and private tmux
  Runtime;
- the observer can resume and identify the exact provider session without
  ingesting transcript content;
- lifecycle metadata remains scoped to its Runtime, and child-session events
  cannot rebind the observed Workstream; and
- HTTP Fork preserves the settled prefix and omits the source's in-flight turn,
  but the destination remains structurally unlinked from its source.

The last point agrees with Spike 0015: OpenCode's Fork response is sufficient
for the happy path, but its source children API and persisted session row do
not provide recovery lineage.

## Design consequence and limits

OpenCode's native TUI backend is an implementation detail of each Runtime, not
a shared app server or a WSNav-owned provider pane. A future adapter must own
one endpoint and one observer helper per Runtime, use a strict metadata
allowlist, and discard event content before it reaches WSNav state.

If a Fork response is lost, WSNav cannot safely identify or adopt the provider
destination. The operation therefore becomes terminal `Failed` with
`external_effect_unknown`; the source Workstream returns to its pre-Fork
visible state, no destination Workstream is created, and the user receives an
error explaining that an unmanaged provider session may need inspection or
cleanup in OpenCode. The same request key cannot replay the provider Fork; a
new explicit Fork is a new request.

This is behavioral evidence for opencode `1.18.11`, not a production adapter,
pixel-fidelity acceptance, or evidence that WSNav should persist provider
transcripts. Re-run the probe after an opencode upgrade before opening a
delivery checkpoint.

[fixture]: ../../../spikes/fixtures/opencode-runtime-contract.json
