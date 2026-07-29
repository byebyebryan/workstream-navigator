# Workstream Navigator V1 Roadmap

Date: 2026-07-29

Status: D0-D1 complete; the next implementation checkpoint is D2

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
| D1 | Complete local Codex CLI vertical slice | Complete |
| D2 | Minimal directly interactive navigator | Planned |
| D3 | Local and SSH hosts through one protocol | Planned |
| D4 | Independent and conversation-forked Workstreams | Planned |
| D5 | Recovery, combined acceptance, and V1 closure | Planned |

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
- explicit `wsnav-observer` profile setup, native trust review, ownership,
  doctor, update, and exact removal;
- passive lifecycle-hook ingestion with full stdin draining and authority
  validation;
- atomic ProviderBinding, status, settled-turn, and sticky-attention updates;
- per-operation App Server stdio reads and canonical thread rename;
- contextual tip-name fallbacks without a shadow Workstream label;
- local snapshots and CLI equivalents for all implemented actions; and
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

## D2 - Minimal navigator

Deliver the first normal user-facing terminal workflow.

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

Exit gate:

- every failure row in the V1 design has an automated or bounded-live
  acceptance case;
- the combined workflow preserves all provider result tips and unrelated
  processes;
- uninstall removes only exactly owned unchanged artifacts;
- no UUIDs, prompts, transcripts, paths, PIDs, credentials, or raw provider
  payloads appear in committed evidence; and
- all repository, package, cleanup, and documentation gates pass.

## Deferred beyond V1

The roadmap does not include arbitrary existing-session adoption, worktree or
branch removal, checkout synchronization, task/context transfer, transcript or
memory features, automatic plan rollover, profile composition, Claude parity,
multiple-controller catalog synchronization, a public daemon, or a replacement
provider UI.
