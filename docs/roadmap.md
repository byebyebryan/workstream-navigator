# Workstream Navigator V1 Roadmap

Date: 2026-08-29

Status: D0-D18 are complete. D18 uses a direct schema-15 epoch and an explicit
destructive reset with no migration or state rollback. Accepted checkpoint
`c961c7e` binds the reset, ordinary schema-15 bootstrap, exact installation,
explicit native observer trust, disposable Codex/OpenCode lifecycle
acceptance, complete cleanup, and installed artifact. Post-acceptance source
correction `ed0d883` is locally verified, but no remote-CI or
accepted-release/live-provider evidence is transferred to it; it does not
reopen the product contract or create a compatibility checkpoint.

`docs/design.md` is the product and architecture contract. This file owns
delivery order, implementation status, and exit gates. The complete prior
roadmap is preserved as
[dated evidence](roadmap-through-d18-design.md).

## Delivery rules

- Implement only the active checkpoint and commit coherent capabilities.
- Automated tests use disposable state roots, repositories, provider homes,
  and private tmux sockets. They never install hooks or touch ordinary state.
- Live provider or installed-artifact acceptance requires explicit operator
  intent, sanitized evidence, exact artifact identity, and complete cleanup.
- A failed product invariant stops the checkpoint. It does not authorize a
  compatibility route, broader ownership, transcript capture, or weaker
  ambiguity handling.
- D18 is a clean state break. Schemas 12 through 14 are refusal evidence, not
  migration or adoption inputs.

## Completed checkpoint: D18 current-only consolidation

D18 preserves the D17.1 shell-first product while reducing the implementation
to one current schema, startup path, presentation, and semantic module graph.
It adds no workflow, provider, UI page, worktree manager, transcript feature,
daemon, or compatibility with the frozen Python prototype.

Delivery order is fixed: establish direct current state; delete transition and
compatibility routes; delete retired product surfaces and decompose current
modules; then reconcile gates, authority documents, and release/reset proof.
No partial slice is an install candidate.

### D18.0 — Direct current-state epoch

Implementation status: complete.

Scope:

- accept only schema 15 with SQLite application ID `0x57534e56` (`WSNV`);
- directly create absent or exact private-empty roots from one schema
  definition;
- use the stable checksummed `bootstrap.lock` phases `root_reserved`,
  `database_create_reserved`, `database_owned`, `database_ready`,
  `provisional_pending`, and `ready`;
- resume only exact current-format interrupted bootstrap evidence; and
- refuse schemas 12-14, future/foreign/malformed databases, client catalog,
  transition artifacts, legacy-shaped top-level evidence, replacements, and
  mixed roots before mutation or SQLite recovery.

Exit evidence:

- every reserve/create/identity/transaction/checkpoint/sync/rename/
  provisional/reopen boundary has deterministic failure injection;
- effect-unknown gaps refuse without cleanup or adoption;
- direct schema contains `onboarding_exec_targets`, no browser-settings table,
  and the exact application-ID/schema pair; and
- current CLI and Navigator behavior pass against disposable fresh roots.

### D18.1 — Transition and compatibility deletion

Implementation status: complete.

Scope:

- delete schema-12/13/14 open, creation, fixtures, validation, and migration;
- delete D16 cutover, client cleanup, legacy presentation retirement, and
  OpenCode standby/handover machinery;
- delete short Runtime-path compatibility; and
- refuse rather than upgrade Codex observer profile schema 1.

Exit evidence:

- no production route can mutate or adopt an old root, presentation, Runtime
  path, or profile contract;
- old-schema samples remain inert raw-header refusal evidence only; and
- the retained lifecycle, ownership, recovery, and privacy matrix passes.

### D18.2 — Retired surfaces and semantic decomposition

Implementation status: complete.

Scope:

- delete the Projects browser/root/refresh/arbitrary-registration surface while
  retaining private promotion-time Git discovery and contextual `n` reuse;
- remove stale aliases, exports, errors, fixtures, and test-only operational
  seams without a current caller;
- replace delivery-checkpoint identifiers and hidden routes with semantic role
  names; and
- split current state, presentation, and provider responsibilities along the
  boundaries defined in `docs/design.md`.

Exit evidence:

- active source and generated help contain no D16/D17 operational name outside
  exact negative refusal assertions;
- retired APIs and files have no compiled route;
- state separates schema/bootstrap, registry, onboarding, observer, and
  projection; presentation separates ownership/topology, control, attachment,
  provisional shell, and cleanup; and
- formatting, strict Clippy, all retained tests, and diff checks pass.

### D18.3 — Reconciliation, release gates, and explicit reset

Implementation status: complete in accepted checkpoint `c961c7e`.

Completed repository work:

- historical D12/D17 current-gate wrappers are replaced by semantic source,
  CLI, presentation, and schema-15 acceptance scripts;
- current documentation distinguishes the installed D18 release from its
  historical D17.1 predecessor;
- the prior roadmap is preserved as dated evidence and current authority is
  reduced to active delivery plus a concise completed index; and
- the local `scripts/check` gate exits successfully, including formatting,
  strict Clippy, tests, packaging, dependency policy, semantic acceptance, and
  staged/unstaged diff checks;
- `docs/design.md` retains the current product and architecture contract while
  historical transition narratives link to preserved evidence; and
- accepted-checkpoint clean-host matrices pass 350 unit and 7 presentation
  tests on both Rust 1.88/Debian/tmux 3.3a and Ubuntu 24.04/Rust 1.88/tmux
  3.4. The Ubuntu
  preflight found and closed the detached 80-column tmux startup race by
  establishing the exact 129-by-24 initial two-pane geometry before the
  provider split; and
- the rejected coherent-backup/symmetric-rollback design is replaced by an
  explicit destructive reset: exact owned-process shutdown and observer
  removal, whole-root quarantine as discarded data, fresh schema-15 bootstrap,
  and no import or downgrade path.

Completed exit gate:

- accepted checkpoint `c961c7e` binds the exact accepted source
  and evidence to the installed SHA-256
  `f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`.

Completed reset/install evidence:

- both exact schema-14 Codex Runtimes were identity-matched and parked; their
  private tmux servers stopped while provider-native history remained outside
  the WSNav root;
- the installed D17.1 observer declaration was removed before the complete
  schema-14 root was atomically quarantined as discarded state;
- the ordinary root directly bootstrapped application ID `0x57534e56`, schema
  15, with zero Workstreams, Runtimes, and operations; and
- the installed D18 artifact is byte-identical to the release build. A real
  80-column launch exposed and then verified the bounded transient-topology
  retry; the corrected shell-first presentation is detached and healthy; and
- explicitly authorized disposable Codex `0.150.0` and OpenCode `1.18.23`
  lifecycle acceptance passed native onboarding, exact binding, settled-result
  attention, fresh-Shell continuity, Park, retained-session start, final Park,
  and complete cleanup. The native Codex observer declaration was explicitly
  trusted, and no provider content or credential was retained; and
- the exact discarded D17.1 quarantine was deleted after acceptance while
  unrelated historical backup/test roots remained untouched.

#### Clean-break reset decision

D18 does not preserve D17.1 as a coherent rollback epoch. The old WSNav catalog,
Runtime ownership, private tmux output, and operational state are explicitly
discarded after exact owned-process shutdown. Provider-native history is not
part of the WSNav root and remains provider-owned. The quarantine was never
read by D18 and was deleted after acceptance; downgrade likewise requires a
fresh destructive reset rather than state restoration.

[Spike 0027](evidence/spikes/0027-d18-root-move-falsification.md) remains the
historical reason not to claim a race-free arbitrary-holder proof for an online
backup/rollback design. That stronger proof is unnecessary for discarded state
and no longer blocks D18.3.

### Post-acceptance D18 correction

Implementation status: implemented in `ed0d883` and locally verified; no
remote-CI or accepted-release/live-provider claim is recorded for this source.
Per-host development installation is separate operational evidence.

Trigger and scope:

- traceability commit `08f9265` exposed an intermittent
  `InvalidTopology` refusal during the one-shot default-width step in
  presentation startup on both stable and MSRV CI jobs;
- startup and post-attach restoration now share one 100-millisecond bounded
  retry that retries only `InvalidTopology`, fails unrelated errors
  immediately, and keeps persistent ambiguity fail-closed;
- direct disposable tests now drive the current Codex/OpenCode Fork and
  managed-operation recovery action/state paths, proving the durable attempt
  marker precedes the provider effect, commit precedes Runtime start, request
  replay is idempotent, Codex lost results reconcile without retry, and
  OpenCode unknown effects become terminal; and
- the obsolete test-only `prepare_fork` state seam is removed.

Local exit evidence:

- `scripts/check` passes 357 library tests, 7 presentation integration tests,
  formatting, strict Clippy, packaging, dependency policy, semantic acceptance,
  and documentation checks; and
- a fresh Rust 1.88.0/Debian/tmux 3.3a container passes the full locked
  all-targets/all-features suite five consecutive times using only
  container-local build and state paths.

The accepted installed artifact and its authorized live-provider evidence stay
historical facts about `c961c7e`; they are not silently transferred to the
newer correction.

## Completed checkpoint index

Detailed scope, procedures, test counts, and version-specific observations are
historical evidence rather than current delivery authority.

| Checkpoint | Outcome | Evidence |
| --- | --- | --- |
| D0-D4 | Contract kernel, local Runtime/Navigator, historical SSH, and Workstreams | [Archived roadmap](roadmap-through-d18-design.md) |
| D5-D7 | Recovery, operator-beta closure, Project identity, and Navigator lifecycle | [Acceptance records](evidence/README.md#acceptance-records) |
| D8 | Concrete Codex/OpenCode provider contract | [D8.1](evidence/acceptance/d8.1-multi-provider.md), [D8.2](evidence/acceptance/d8.2-opencode-fork-recovery.md) |
| D9-D15 | Reliability, interaction, browser, presentation, terminal, and switching refinements | [Archived roadmap](roadmap-through-d18-design.md) |
| D16 | Host-local clean break | [D16 acceptance](evidence/acceptance/d16-host-local.md) |
| D17 | Shell-first managed-session onboarding | [D17 acceptance](evidence/acceptance/d17-shell-first.md) |
| D17.1 | Correctness and release closure | [D17.1 acceptance](evidence/acceptance/d17.1-correctness-closure.md) |
| D18 | Current-only consolidation and release acceptance complete; source correction locally verified | [Acceptance evidence](evidence/acceptance/d18-current-source-candidate.md) |

## Deferred product decisions

- Worktree detection, never worktree management.
- Any future provider, release channel, packaging system, daemon, or richer
  metadata workflow.
