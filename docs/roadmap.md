# Workstream Navigator V1 Roadmap

Date: 2026-08-31

Status: D0-D18 are complete. D19 is the active design-first UI/UX checkpoint;
its interaction contract is specified below and production implementation has
not started. D18 remains the implemented baseline: it uses a direct schema-15
epoch and an explicit destructive reset with no migration or state rollback.
Accepted checkpoint `c961c7e` binds the reset, ordinary schema-15 bootstrap,
exact installation, explicit native observer trust, disposable Codex/OpenCode
lifecycle acceptance, complete cleanup, and installed artifact.
Post-acceptance source correction `ed0d883` does not transfer those
accepted-release/live-provider claims or reopen the state contract.

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

## Active checkpoint: D19 tmux-derived presentation navigation

Implementation status: design specified; no production implementation or
acceptance claim.

D19 tightens the existing two-pane shell-first presentation without adding a
page, pane, window, provider, state schema, provider effect, or persisted UI
preference. Its governing separation is:

- tmux alone owns which presentation pane receives keyboard input;
- the Navigator owns its process-local row selection;
- the presentation controller owns which exact shell, review, or managed
  Runtime surface appears in the right pane; and
- lifecycle actions retain their existing revision-fenced state authority.

Delivery order is fixed: prove the tmux mouse, copy-mode, nested-prefix, and
multi-client semantics in a disposable study; establish the single focus
authority and closed tables on both private tmux layers; add bounded
provider-pane Workstream switching through the existing presentation attachment
claim/status boundary; then complete the full gate and installation. No earlier
slice changes the installed product contract.

### D19.0 — Single focus authority

Scope:

- keep a fresh presentation focused on Navigator while showing its initial
  Shell surface, and preserve tmux's existing active pane on reattach;
- make `Ctrl+b Left` and `Ctrl+b Right`, plus the deliberate primary-button
  press that begins a click or drag in either pane, the only ordinary
  focus-changing inputs;
- keep `Enter`, card activation, Start, Fork, recovery, observer review,
  background reconciliation, resize, and right-surface replacement from
  changing pane focus; and
- keep focus ephemeral in tmux: never persist, infer, poll, or reconstruct it
  in WSNav state.

The existing exact client, source-pane, ownership, and topology checks remain
mandatory. Tmux-derived interaction does not authorize a raw default tmux key
table or a weaker `select-pane` boundary. The active pane is shared tmux
window/session state: a focus change by one client attached to the same
presentation is visible to its other attached clients. D19 adds no per-client
focus field or input lease.

### D19.1 — Fixed private-tmux control surfaces

Scope:

- discard the default prefix and root management tables on both the private
  presentation server and every private single-pane Runtime server, then rebuild
  each from its own closed allowlist;
- on the presentation, keep `Ctrl+b d` for detach, `Ctrl+b Ctrl+b` for the
  existing bounded literal-prefix path, `Ctrl+b ?` for presentation help, and
  Left/Right for exact two-pane focus;
- on a direct Runtime attachment, keep only detach, literal-prefix delivery,
  bounded help, and copy-mode entry for its exact sole provider pane;
- remove focus-next and vertical-focus interpretations from `Ctrl+b o`,
  `Ctrl+b Up`, and `Ctrl+b Down`; and
- explicitly omit every split, new/select/next/previous/rename/kill/link/move
  window, pane kill/swap/join/break/rotate/resize, layout mutation, menu, and
  arbitrary command-prompt route from keyboard and mouse tables.

The primary-button press may both focus the target pane and begin delivery of
that click or drag to its native surface; release only completes delivery and
is not an independent focus trigger. Hover and wheel input must not change
focus. Copy-mode and nested alternate-screen scrolling must remain usable
without selecting a previously inactive presentation pane. The Runtime root
table forwards native mouse input only to its exact sole pane; its bounded
copy/scroll bindings cannot create or select topology.

The allowlists remove interactive routes offered by WSNav-owned key and mouse
tables; they are not a security boundary against the same user explicitly
addressing a known private socket with the tmux CLI. Any externally changed or
ambiguous topology still fails the existing ownership checks closed. Reattach
must converge exact D18-owned presentation and Runtime servers to the D19
tables without restarting a provider, and must never touch an ordinary or
foreign tmux server.

### D19.2 — Provider-pane Workstream switching

Scope:

- when an exact managed provider Runtime owns the focused right pane,
  `Ctrl+b Up` and `Ctrl+b Down` attach the previous or next eligible managed
  Workstream strictly above or below the current row in the same bounded visual
  order as Workstreams; the source must still occur in that fresh active
  projection;
- eligible means active, non-archived, free of onboarding/recovery fences, and
  backed by an already-live Runtime that passes the ordinary attachment
  preflight;
- switching never materializes Shell, starts, resumes, recovers, forks, parks,
  or otherwise causes a provider or lifecycle effect;
- the first and last eligible Workstreams do not wrap, and an unavailable
  direction leaves the current attachment and focus unchanged with bounded
  tmux-client guidance outside provider content;
- the successful switch preserves focus in the right pane, returns Navigator to
  Workstreams if necessary, and aligns its process-local selection with the
  newly attached Workstream; and
- Shell, provider-wait, native observer review, onboarding, stopped,
  recovery-required, archived, and direct-attach surfaces do not participate.

The ordered projection and attachment preflight must remain shared semantic
authorities rather than being reimplemented in tmux shell fragments. No
Workstream ID, provider identifier, path, or raw state is rendered in tmux
guidance. The helper resolves from a fresh bounded snapshot and commits through
the existing presentation attachment claim after revalidating the current
attachment, source-pane role, pane Workstream marker, topology, and revisions.
Its existing mode-`0600` attachment status is the presentation-private
synchronization boundary by which Navigator observes the completed destination
and aligns its page/selection; D19 adds no listener, general event bus, tmux
`send-keys` injection, or durable UI state. Races fail closed and preserve the
current attachment.

### D19 exit gate

- a disposable tmux study first proves click, drag, wheel, copy-mode, nested
  Runtime prefix, reattach, and optional outer-tmux prefix-passthrough behavior;
- deterministic tests prove that only Left/Right and primary-button press change
  focus, while release, wheel, every Navigator action, and asynchronous
  completion preserve it;
- deterministic tests prove Up/Down changes only an exact eligible attachment,
  preserves right-pane focus, aligns Navigator selection, stops at boundaries,
  and never launches or mutates a Runtime/provider;
- presentation and Runtime key-table tests prove both complete allowlists, D18
  live-server convergence, and the absence of split, window, layout, menu,
  command-prompt, and unsafe mouse routes;
- a tmux-owned, unambiguous focus cue outside provider content makes the active
  pane visible without Navigator polling; and
- `scripts/check`, the declared MSRV job, nested terminal input/fidelity tests,
  and staged/unstaged diff checks pass before the locked release is installed
  for operator inspection.

No partial D19 slice is an install candidate. Live-provider acceptance, if the
final implementation needs it, remains separately authorized and
artifact-bound.

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
