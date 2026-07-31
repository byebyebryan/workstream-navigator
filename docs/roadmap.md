# Workstream Navigator V1 Roadmap

Date: 2026-07-29

Status: D0 through D5.1 complete

This roadmap turns the reconciled [V1 design](design.md) into reviewable
delivery checkpoints. The design remains the product and architecture contract.
This document owns sequencing, exit gates, and progress.

## Delivery rules

- Each checkpoint ends in a working, reviewable repository state.
- Commit by coherent capability; do not hold unrelated layers for one large
  checkpoint commit.
- Automated tests use disposable state, repositories, Codex homes, and private
  tmux sockets. They never install hooks or mutate ordinary Codex or tmux state.
- Live acceptance is bounded, records sanitized evidence, and proves cleanup
  plus non-interference with unrelated Codex and tmux processes.
- A failed core invariant stops the checkpoint. It does not authorize widening
  ownership, storing transcripts, replacing native UI, or weakening recovery
  ambiguity rules.
- Product polish follows functional acceptance. It does not mask incomplete
  lifecycle, isolation, or recovery behavior.

## Checkpoint overview

| Checkpoint | Outcome | Status |
| --- | --- | --- |
| D0 | Contract kernel and durable state | Complete |
| D1 | Local Codex CLI vertical slice | Complete (reconciled by D1.5) |
| D1.5 | Reconcile native trust and same-workstream tip transitions | Complete |
| D2 | Minimal directly interactive navigator | Complete |
| D3 | Local and SSH hosts through one protocol | Complete |
| D4 | Independent and conversation-forked Workstreams | Complete |
| D5 | Recovery, combined acceptance, and V1 closure | Complete |
| D5.1 | Operational closure for recovery, release diagnostics, and bounded I/O | Complete |

## D0 - Contract kernel

Build the provider-independent core that every later surface uses.

Scope:

- crate/module structure from the V1 design;
- opaque typed IDs, entities, lifecycle values, revisions, invariants, and
  stable error classes;
- private state-root discovery and permission policy;
- fresh SQLite host schema and client-catalog schema;
- transactional compare-and-update primitives;
- Start/Fork `CompoundOperation` phases, request deduplication, and ambiguous
  effect outcomes;
- versioned host request/response and capability types;
- deterministic clocks, ID sources, process probes, and failure injection at
  test boundaries; and
- The first commit that introduces production dependencies also adds their
  license/advisory policy and CI gate.

Exit gate:

- schema creation and development migrations are deterministic;
- invalid ownership, lifecycle, and revision transitions fail closed;
- concurrent attention updates cannot clear a newer event;
- Start/Fork phase tests cover pre-effect, confirmed effect, lost response,
  ambiguous effect, retry, and recovery-required outcomes;
- protocol frames reject unknown incompatible versions and bounded-field
  violations;
- introduced production dependencies are covered by the license/advisory gate;
  and
- the full format, lint, test, package-content, and `git diff --check` gates
  pass.

## D1 - Local Codex runtime

Deliver the first end-to-end product slice through direct CLI commands. D1
intentionally has no navigator TUI.

Scope:

- local ProjectLocation registration and one external initial Checkout;
- exactly one private tmux server, session, window, and pane per live Runtime;
- native Codex start, direct attach, park, and exact resume;
- explicit host-only `wsnav-observer` profile setup, native trust review with
  automatic post-review verification, ownership, doctor, update, and exact
  removal;
- passive lifecycle-hook ingestion with full stdin draining and authority
  validation;
- atomic ProviderBinding, status, settled-turn, and sticky-attention updates;
- per-operation App Server stdio reads and canonical thread rename;
- contextual tip-name fallbacks without a shadow Workstream label;
- direct CLI equivalents for D1 actions; bounded local snapshot projection is
  owned by D2's navigator surface; and
- cold Runtime reconciliation sufficient to resume one known native session.

Exit gate:

- a user can register an existing local checkout, start native Codex, attach
  interactively, complete a turn, observe attention, rename, park, and resume
  the exact thread;
- the provider pane remains byte-for-byte untouched after a completed result
  until the user acts;
- missing, stale, forged, racing, malformed, and oversized hooks cannot mutate
  an unrelated binding or break Codex;
- lost local action responses reconcile one Runtime generation without
  duplicate launch;
- ordinary Codex launches, the user's default tmux server, unrelated
  configuration, and existing history remain unchanged;
- setup and removal preserve foreign or modified files and report exact manual
  recovery; and
- automated and bounded live acceptance both pass with sanitized evidence and
  complete cleanup.

### First goal loop

The first implementation loop ends at D1, not D0. D0 alone is infrastructure;
D0 plus D1 proves the architecture against one real native Codex workflow.

Expected commit-sized slices:

1. establish the Rust module boundaries and domain contract;
2. add SQLite state, revisions, operations, and protocol types;
3. add private tmux Runtime ownership and probes;
4. add scoped Codex profile, hook, and App Server adapters;
5. add local CLI orchestration for register/start/attach/status/rename/park/resume;
6. add failure reconciliation and local acceptance evidence; and
7. reconcile documentation with the accepted behavior.

## D1.5 - Local Codex reconciliation

The first D1 live acceptance proved the direct native happy path, but review
identified implementation gaps against the design contract. Resolve them
before building a presentation surface. This is a correction to D1, not a
second provider or a navigator feature.

Scope:

- prove the installed Codex lifecycle contract for native same-TUI `/clear`
  (and any emitted source value) in a disposable, profile-selected run before
  permitting a changed binding;
- make observer setup open an explicit, private native trust-review session in
  an empty disposable directory, then verify the approved profile without
  writing Codex trust state;
- corroborate initial and permitted changed bindings through a bounded,
  read-only `thread/read` App Server request before durable state changes;
- accept only the live-proven, same-runtime native session transition and
  preserve predecessor metadata, settled attention, and fail-closed behavior
  for all other changed SessionStart events;
- add the exact observer profile update contract or move it out of D1 with a
  documented reason; and
- correct the App Server lifecycle documentation and D1 local acceptance
  procedure to describe the implementation actually required for native
  trust, binding, cutover, and cleanup.

Exit gate:

- a disposable live run proves the event order and source needed for the
  accepted native transition; unproven native actions remain rejected;
- setup uses only an exact owned profile, a private tmux server, and a
  disposable review cwd; native `/hooks` approval remains an explicit operator
  action;
- forged, stale, replayed, concurrent, unsupported-source, or
  provider-nonexistent session claims cannot replace the current binding;
- an accepted `/clear` changes only the current ConversationTip in the same
  Runtime and Workstream, without restarting the TUI or clearing prior result
  attention; and
- automated checks plus a sanitized, bounded native reacceptance pass with no
  WSNav-owned runtime/profile/review artifacts left behind.

Recorded evidence: [D1 local native-Codex acceptance](acceptance-d1-local-codex.md)
and its [D1.5 reconciliation fixture](../spikes/fixtures/d1.5-local-codex-reconciliation.json).

## D2 - Minimal navigator

Deliver the first normal user-facing terminal workflow.

Implementation status: complete. The bounded local snapshot, private
presentation tmux owner, direct attachment helper, Ratatui navigator,
disposable isolation acceptance, and operator-trusted native Codex terminal
acceptance passed. See the [D2 local navigator acceptance](acceptance-d2-local-navigator.md).

Scope:

- a disposable, private local presentation tmux session;
- a small Ratatui navigator pane beside the directly attached native Codex
  pane;
- keyboard and mouse selection, focus, switching, reconnect, and attention
  acknowledgement;
- immediate action-result updates plus bounded local snapshots; and
- direct `wsnav attach` mode using the same Runtime contract.

Exit gate:

- keyboard, mouse, color, resize, image attachment, detach/reconnect, and
  completed-result preservation pass product-level terminal regression tests;
- switching changes only the provider attachment helper and never restarts an
  inactive Runtime; and
- exiting the presentation leaves every host Runtime alive and recoverable.

## D3 - SSH hosts

Extend the accepted local semantics across pre-registered hosts.

Implementation status: complete. The bounded one-shot `_remote` service,
strict shell-free SSH adapter, fixed client registration fingerprint, local
subprocess parity tests, revision-guarded remote actions, interactive
`ssh -tt` attachment, and cached/backing-off navigator view passed automated
coverage and bounded operator-run native-Codex acceptance. The implementation
never copies or installs a remote binary; the remote executable remains an
explicit operator prerequisite. See the [D3 control-plane
acceptance](acceptance-d3-control-plane.md) and its [sanitized fixture](../spikes/fixtures/d3-ssh-control-plane.json).

Scope:

- host registration with fixed executable path;
- protocol/version/capability handshake and stable host identity;
- bounded JSON snapshot and apply commands over SSH;
- interactive `ssh -tt` attachment to an exact remote Runtime;
- adaptive polling, backoff, reconnect, and cached unreachable presentation;
  and
- remote start, attach, park, and cold resume.

Exit gate:

- local and SSH adapters return the same semantic outcomes;
- disconnect never implies stopped state or loses durable attention;
- host identity, registry generation, protocol, and capability disagreement
  disable mutation with actionable diagnostics; and
- simultaneous local and remote work preserves both native result tips and
  never creates sessions on either user's default tmux server.

## D4 - Workstreams and forks

Complete the explicit parallel-workstream actions.

Implementation status: complete. The disposable local harness and a bounded
native-Codex run both passed. See the [D4 Workstream and fork
acceptance](acceptance-d4-workstreams.md) and its [sanitized
fixture](../spikes/fixtures/d4-local-codex-workstream-fork.json).

Scope:

- collision-free managed branches and worktrees from one recorded
  `default_base_ref` commit;
- independent Workstream creation;
- exact settled-prefix App Server conversation fork from a running source;
- bounded provisional native fork naming;
- destination native resume in its independent checkout; and
- lost-response fork reconciliation without retrying an ambiguous
  non-idempotent provider operation.

Exit gate:

- independent and forked Workstreams have distinct IDs, Checkouts, Runtimes,
  and ConversationTips;
- a fork sees the last settled source turn and never the source's running turn;
- the source continues unchanged while the destination diverges;
- zero or multiple recovery candidates remain `recovery_required`; and
- dirty, external, shared, mismatched, or ambiguously owned worktrees are
  preserved.

The disposable local harness (`scripts/d4-local-workstream-acceptance.sh`) and
parser/state/transport tests drive a source with one completed turn followed by
an in-progress turn, assert that the provider request names only the completed
turn, prove the destination worktree contains the recorded base but not
source-only files, and compare the ordinary tmux fingerprint before and after
cleanup. The native run corroborated the same contract against the installed
Codex App Server and direct provider TUI.

## D5 - Recovery and V1 acceptance

Close the V1 contract after all behavior exists.

Scope:

- crash and partial-effect reconciliation across Start, Resume, and Fork;
- integration install, doctor, update, uninstall, and residue verification;
- combined local/remote/start/fork/switch/reconnect/cold-resume acceptance;
- bounded diagnostics and privacy audit;
- navigator UX and accessibility polish;
- operator and user documentation; and
- package-content and fresh-host installation verification.

The disposable native-recovery and fresh-install gates plus the bounded
combined real-Codex local/remote operator acceptance pass. See the
[D5 acceptance record](acceptance-d5-v1-closure.md).

Exit gate:

- every failure row in the V1 design has an automated or bounded-live
  acceptance case;
- the combined workflow preserves all provider result tips and unrelated
  processes;
- uninstall removes only exactly owned unchanged artifacts;
- no UUIDs, prompts, transcripts, paths, PIDs, credentials, or raw provider
  payloads appear in committed evidence; and
- all repository, package, cleanup, and documentation gates pass.

## D5.1 - Operational closure

Implementation status: complete. Durable operation recovery, release probing,
streaming local child-output bounds, full Runtime identities, explicit
first-run guidance, and declared/CI-enforced MSRV passed the disposable and
full local repository gates. See the [D5.1 operational closure acceptance](acceptance-d5.1-operational-closure.md).

Close the release-quality gaps found by the post-D5 broad review without
expanding the approved V1 product.

Scope:

- list and recover an exact unresolved Start or Fork CompoundOperation through
  local CLI, SSH protocol, and navigator visibility;
- preserve the exact-once Fork marker: recovery may reconcile a marked fork but
  never issue a second provider `thread/fork` call;
- stateless remote release probe, safe state-schema incompatibility diagnostic,
  and manual remote-upgrade documentation;
- streaming output caps for runtime tmux, presentation tmux, Git, and navigator
  child commands;
- full Runtime UUID private paths, an explicit empty navigator registration
  path, and declared plus CI-tested MSRV; and
- disposable recovery and compatibility acceptance with no normal Codex,
  provider, or default-tmux mutation.

Exit gate:

- a simulated client loss after each Start/Fork effect can be recovered from a
  visible opaque operation ID without its original request key;
- a stale or missing remote executable is diagnosed before stateful control,
  while matching hosts continue to work through local-subprocess and SSH
  protocol paths;
- every affected child process is output-bounded while it is read, not after it
  is buffered;
- first-run guidance requires an explicit checkout registration rather than a
  guessed current directory;
- the complete repository gate, package verification, disposable acceptance,
  and privacy audit pass.

## Deferred beyond V1

The roadmap does not include arbitrary existing-session adoption, worktree or
branch removal, checkout synchronization, task/context transfer, transcript or
memory features, automatic plan rollover, profile composition, Claude parity,
multiple-controller catalog synchronization, a public daemon, or a replacement
provider UI.
