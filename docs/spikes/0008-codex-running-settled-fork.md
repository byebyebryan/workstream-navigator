# Spike 0008: running-source settled-prefix fork

## Hypothesis

After native source turn X settles and native turn Y starts, Workstream
Navigator can create a new Workstream whose conversation contains exactly X
and whose filesystem starts from the Project's recorded default-base commit.
The source TUI and Y must continue unchanged. If the non-idempotent fork
response is lost, one exact persisted effect must be recoverable without
retry.

## Procedure and isolation

The harness creates a temporary Codex home, a disposable Git repository with
one committed main branch, a private source runtime/presentation pair, and a
real native Codex TUI. The source completes one fixed harmless turn X, then
starts Y by running `sleep 120`.

While that descendant command is live, the harness:

1. records X's exact completed turn ID;
2. creates a clean destination worktree and branch from the recorded main
   commit;
3. sends exactly one `thread/fork` request through a short-lived stdio App
   Server with X as `lastTurnId`;
4. leaves the successful response unread and closes the helper;
5. performs bounded recovery with `thread/list` and exact `thread/read`,
   accepting only one candidate whose creation time, source lineage, and
   one-turn settled prefix agree;
6. starts a separate native `codex -C <destination> resume <id>` TUI; and
7. runs divergent turn Z containing a harmless `sleep 20`, observing the
   descendant command's actual cwd.

The harness compares source pane/process facts before and after divergence and
keeps Y running until cleanup. It never interrupts, steers, or waits for Y to
finish as part of the fork operation.

## Observed contract

The automated study passed locally with Codex CLI 0.145.0 and the recorded App
Server schema fingerprint; see the sanitized [fixture][fixture].

- the schema exposes `threadId`, `lastTurnId`, and `cwd` but no idempotency key;
- X was settled before Y began, and Y's `sleep` remained live through fork,
  destination resume, and Z;
- a separate App Server read represented Y's persisted partial turn as
  interrupted while its native command was still running, proving helper
  status is not live-TUI authority;
- the destination worktree HEAD exactly matched the recorded default-base
  commit;
- the valid fork request was submitted once and its response remained unread;
- recovery found exactly one destination from creation time, source lineage,
  and exact X boundary, without retry;
- the destination contained one completed turn X and no part of Y;
- native resume displayed X's history and accepted divergent turn Z;
- Z's descendant command actually ran from the destination worktree;
- the source pane, process birth, cwd, and running command remained stable; and
- both checkouts remained clean, all helpers/private tmux servers exited, and
  ordinary tmux plus pre-existing Codex processes were unchanged.

## Provider quirks and design impact

Two installed behaviors narrow the original recovery design without invalidating
the workflow.

First, the fork's pre-resume persisted `cwd` did not equal the requested
destination path. The destination launch must therefore make the checkout
authoritative with native `codex -C <destination> resume <id>`. The actual Z
command cwd proved that this works. Pre-resume recovery may record the requested
cwd but cannot require it to appear in fork metadata.

Second, the fork did not appear in a CLI-only source-kind query. Recovery for
an unresolved WSNav-owned fork must include every documented source kind, then
filter by the durable operation's exact lineage, settled boundary, and effect
time. This broader query is allowed only for that unresolved operation. Zero or
multiple matching candidates remain `recovery_required`.

App Server's view of a separately running native turn is likewise not activity
authority. Provider hooks plus native process/runtime evidence determine
working state; App Server remains a persisted-thread metadata adapter.

These are recovery-contract revisions, not reasons to add a shared App Server,
interrupt the source, copy its working tree, or retry a non-idempotent fork.

## Privacy and cleanup

The fixture contains only provider/schema fingerprints, boolean relationships,
timing, and cleanup status. No thread/turn UUID, prompt, transcript, response,
name, preview, path, PID, credential, environment, raw App Server frame, or
terminal capture was committed. All live provider state was disposable and
deleted.

[fixture]: ../../spikes/fixtures/codex-running-settled-fork.json
