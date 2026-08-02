# Spike 0007: ephemeral Codex metadata and naming

## Hypothesis

Workstream Navigator can read and rename the current native Codex thread
through bounded stdio App Server helpers without making the TUI a client of a
shared runtime, disturbing its visible result, or introducing a second naming
authority.

## Procedure and isolation

The spike creates a disposable Git repository, temporary CODEX_HOME, private
runtime tmux server, separate private presentation server, and one real native
Codex TUI. Its synthetic base config disables hooks and pre-trusts only the
disposable workspace.

After one harmless native turn settles, the harness identifies the only exact
CLI thread in the temporary store. Every read or mutation uses a new
`codex app-server --listen stdio://` process with a private JSONL connection,
the initialize handshake, bounded output, and bounded shutdown.

The live sequence is:

1. read the exact thread summary with `includeTurns: false`;
2. rename through native `/rename` and observe the new name through a later
   short-lived reader;
3. set a second fixed synthetic name through `thread/name/set`;
4. verify that name through another short-lived reader; and
5. evaluate every V1 missing/unavailable-name fallback without another provider
   mutation.

Screens and native runtime facts are compared around App Server operations.
Raw thread IDs, names, previews, prompts, responses, paths, PIDs, schemas, and
terminal captures remain private and are deleted.

## Observed contract

The live automated study passed locally with Codex CLI 0.145.0 and the recorded
schema fingerprint; see the sanitized [fixture][fixture].

- an exact `thread/read` did not change the native TUI screen, pane, process
  birth, or cwd;
- native `/rename` persisted to the Codex-owned `thread.name` field and was
  visible to a later helper;
- `thread/name/set` persisted to that same field without disturbing the TUI;
- every App Server helper exited after its operation;
- filtered metadata exposed only `name_state` and `name`, never `preview`;
- the fallback matrix distinguished `known_empty` from `unavailable`, used
  stable Workstream short IDs, retained cached names as stale, and covered
  new, cutover, and fork contexts;
- evaluating a cutover/fork fallback performed no provider write;
- installed `ThreadSetNameParams` contained only `threadId` and `name`, with no
  compare-and-set field;
- persistent Unix/WebSocket App Servers and managed `codex --remote` runtimes
  were rejected by the spike boundary; and
- the disposable repository, ordinary tmux server, and pre-existing Codex
  processes were unchanged after complete cleanup.

## Decision and limits

The short-lived stdio helper is viable for exact managed-thread metadata and
explicit rename. It does not replace the dedicated native TUI runtime.

The lack of compare-and-set is now live schema evidence. Workstream Navigator
must not copy the previous tip's name into a new cutover tip: a concurrent
native or skill rename could be overwritten. Cutover naming remains a visibly
provisional computed fallback. A navigator-controlled fork may still set a
provisional name before its destination TUI starts, as already designed.

This spike does not authorize broad thread discovery in the normal path.
`thread/list` was safe only because the temporary Codex home contained one
disposable CLI thread; production V1 uses it only for bounded recovery and
doctor operations.

The Python harness is disposable validation code. The product implementation
remains Rust and must reproduce the allowlist, filtering, timeout, and cleanup
properties.

[fixture]: ../../../spikes/fixtures/codex-app-server-naming.json
