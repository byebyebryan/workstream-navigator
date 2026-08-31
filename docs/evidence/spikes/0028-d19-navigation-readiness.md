# D19 navigation readiness and baseline falsification

Date: 2026-08-31

Status: partial pre-implementation evidence. The D19 product contract remains
viable, but three assumptions about reusing the D18 implementation unchanged
are falsified. No production source, ordinary tmux server, provider process,
live WSNav state, or installation was changed during this study.

Candidate: documentation checkpoint `a271ac9` on tmux `3.7c`.

## Question

Can D19 be implemented by only changing the outer presentation key bindings,
while reusing the current Runtime topology probe, attachment preflight, and
Navigator row ordering unchanged?

## Result

No. The product contract is implementable, but its implementation slice must
also close three baseline gaps:

1. The private Runtime configuration does not clear tmux's default prefix/root
   tables. Direct Runtime attachment therefore exposes split, new/select/next
   window, layout, and related management commands. The current Runtime probe
   proves a target session and pane `0.0`, but does not reject an additional
   pane or window; a disposable two-pane Runtime remained acceptable to the
   equivalent current probe.
2. `actions::preflight_attachment` is not a read-only predicate. Depending on
   evidence, it may backfill a provider PID, mark a Runtime
   `recovery_required`, or mark an OpenCode observer handle unknown. D19
   Up/Down switching promises no durable or lifecycle mutation, so it requires
   a strict read-only success path that refuses any evidence needing repair.
3. `load_project_projections` orders Projects by opaque `ProjectId`, and
   Navigator `rows_for` iterates that order. Workstream activity order exists in
   the registry query but its sequence is not retained in `WorkstreamSnapshot`.
   This contradicts the design requirement that Project groups sort by their
   newest included member and prevents a second control helper from sharing an
   exact semantic visual order.

These are implementation falsifications, not reasons to weaken D19. The
checkpoint must establish exact single-session/single-window/single-pane
Runtime proof, a non-mutating attachment validator, and one shared pure
Project/Workstream ordering authority before provider-pane switching can be
accepted.

## Disposable tmux evidence

Two temporary private tmux servers and isolated temporary directories were
used. The ordinary/default tmux server and ordinary user state were not
addressed.

- tmux's default prefix table exposed split, window, select/next, layout, and
  related general management commands;
- clearing the prefix and root tables left only explicitly installed bindings;
- in the fixed two-pane presentation, `Ctrl+b Left` and `Ctrl+b Right` changed
  the active pane while Up/Down client guidance did not;
- an SGR primary-button press changed the active pane immediately, and its
  release did not create a second focus transition; and
- the temporary servers and directories were stopped and removed after the
  study.

This partial study does not transfer D19 acceptance. Before production behavior
changes, the writer must still prove drag, wheel, copy mode, nested Runtime
prefix delivery, reattach, multiple clients, the unambiguous tmux-owned focus
cue, and the optional outer-tmux passthrough boundary on disposable state.

## Implementation consequence

D19 must land as one coherent interaction checkpoint. Shipping only the outer
focus bindings would leave direct Runtime management commands and hybrid
Navigator focus behavior. Shipping Up/Down before the read-only validator and
shared ordering authority would either mutate durable state or create a second
ordering implementation. Any disposable study result that requires provider
content, Navigator polling, unbounded IPC, or a weaker topology check returns
to design review instead of being worked around.

## Follow-up

Checkpoint `a0ec38b` subsequently corrected all three falsifications and passed
the complete local/disposable D19 gate. The exact candidate, installed artifact,
and remaining evidence limits are recorded in the
[D19 acceptance record](../acceptance/d19-tmux-navigation.md). This follow-up
does not rewrite the partial pre-implementation status of the study above.
