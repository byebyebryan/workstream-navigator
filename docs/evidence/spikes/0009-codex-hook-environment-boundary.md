# Spike 0009: Codex hook-environment boundary

## Hypothesis

A managed Codex TUI preserves the launch authority environment through to the
passive commands configured by the selected `wsnav-observer` profile. The hook
can therefore use the runtime ID, generation, state root, and authority value
to bind one lifecycle event to one private Runtime.

## Procedure and privacy

This was a live operator recovery check against Codex CLI 0.146.0 after native
hook trust was re-established for the current installed WSNav executable. It
used one already-managed, previously parked Workstream and one private tmux
server; no ordinary tmux server, unmanaged Codex session, repository, prompt,
or transcript was inspected or changed.

The study recorded only boolean relationships:

- the private tmux environment contained each launch authority value;
- the resumed Codex process retained only its sanitized environment; and
- the expected `SessionStart` transition did not reach durable state.

It retained no provider or Workstream IDs, paths, PIDs, prompts, responses,
terminal captures, raw hook payloads, or credentials.

## Observed contract

The hypothesis is **falsified** for the observed Codex CLI build.

- The exact observer profile was trusted and invoked the installed WSNav
  executable, rather than an obsolete build-tree executable.
- WSNav supplied the four per-Runtime authority values to its private tmux
  server.
- The live Codex process retained `HOME`, `PATH`, and `SHELL`, but none of the
  WSNav authority values.
- The resumed Runtime remained `starting`, despite the current state machine
  requiring its accepted `SessionStart` event to transition it to `idle`.
- The current Codex hook documentation guarantees the JSON input object and
  documents a plugin-specific environment extension, but it does not guarantee
  inheritance of arbitrary launch environment values by ordinary command hooks.

## Decision and impact

The current production observer implementation is not viable: it has no
authority input after Codex sanitizes the provider environment, so every hook
correctly fails closed. This invalidates the live-status, activity-age, result
attention, cutover, and fork preconditions that rely on passive lifecycle
observation.

Do not weaken the binding check or resume observer-dependent implementation.
A redesign must first prove a replacement authority transport with the same
properties: it must bind the hook to one exact private Runtime, reject an agent
shell invocation, survive repeated native session changes, and retain the
provider's native UI and result tip. Candidate mechanisms require a dedicated
study before product work resumes.

The stale build-tree hook path discovered during recovery was repaired to the
stable installed executable and native trust was renewed. That deployment fix
does not alter this conclusion.

[sanitized fixture]: ../../../spikes/fixtures/codex-hook-environment-boundary.json
