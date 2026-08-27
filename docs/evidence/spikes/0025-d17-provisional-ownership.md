# Spike 0025: D17 provisional ownership lifecycle

## Question

Can the selected D17 ownership model keep a provisional account shell outside
the registry until an exact lease-held promotion wins, while deterministic
cleanup-versus-helper interleavings leave either no residue or one durable,
action-fenced Runtime plus one fresh provisional candidate?

## Procedure and isolation

The deterministic harness is
[`spikes/d17-provisional-ownership.py`](../../../spikes/d17-provisional-ownership.py),
with its sanitized [fixture][fixture]. It uses a mode-`0700` temporary root,
private fake runtime artifacts, marker files, a small journal/registry model,
and one mode-`0600` nonblocking host-private flock. It never starts tmux or a
provider, and deletes its Git repositories, worktree, state, markers, journal,
and process-free fake artifacts before producing the fixture.

The model materializes only final full-UUID `RuntimePaths` equivalents
(directory, socket, config, session) and records them in a presentation-private
marker with a fresh slot generation. The bounded classifier recognizes an exact
marker-backed candidate, another presentation's busy candidate, registered
owned runtime directories, and otherwise refuses unknown or markerless
`runtime-*` artifacts.

The harness runs both serialized winner orders under the same flock: cleanup
after prepare but before helper consume, and helper consume/persistent ownership
before later cleanup. It also checks duplicate helper execution, the
`runtime_owned_launching` action fence, later exec proof, full-ID collision,
linked-worktree root detection, non-Git refusal, and materialization of the
next provisional slot after promotion.

## Result

The fixture passed.

- A linked worktree child resolves to that linked worktree root; a non-Git seed
  refuses without fallback.
- The first presentation materializes one marker-only full-UUID candidate; the
  registry is empty and a second presentation is busy without creating a second
  candidate.
- Cleanup winning after prepare cancels the handoff and leaves no registry row,
  candidate artifacts, or materialized marker. The old helper cannot promote.
- Helper promotion records durable ownership before removing presentation
  cleanup authority. Subsequent cleanup is a no-op that neither removes nor
  signals the promoted runtime; duplicate helper consumption refuses.
- The promoted runtime remains action-fenced until exact exec proof. Promotion
  then derives one new independent provisional candidate.
- Foreign collision and markerless `runtime-*` artifacts block materialization
  without adoption or deletion.

## Consequence

The lifecycle has a coherent serialized ownership rule for the two important
winner orders: pre-consume cleanup may roll back the provisional candidate,
whereas post-commit presentation cleanup has no process authority. This is the
model the schema-14 state, presentation marker, onboarding journal, and helper
must implement as one contract.

## Limits

- This is a synthetic sequential interleaving model, not concurrent Rust actors
  or the real SQLite schema/CompoundOperation graph. It does not prove process
  identity, revision/capability claims, restart replay, or every race window.
- Its artifacts are fake files; it does not prove private tmux lifecycle,
  Bash/Zsh account shells, broker I/O, Codex observer readiness, OpenCode
  `POST /session`, provider exec, or completed-output handling.

## Status

**Marker-backed ownership model validated; concurrent production integration,
provider effects, and recovery remain D17.0 gates.**

[fixture]: ../../../spikes/fixtures/d17-provisional-ownership.json
