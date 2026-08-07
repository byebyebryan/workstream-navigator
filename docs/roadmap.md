# Workstream Navigator V1 Roadmap

## 2026-08-05 minimal multi-provider creation contract and D8 approval

The [V1 design](design.md#multi-provider-and-multi-agent-design) now records a
minimal creation contract for generalizing the single-Codex V1 into a
multi-provider navigator. It is supported by [Spike
0015](evidence/spikes/0015-opencode-provider-feasibility.md), which validates
opencode's settled-prefix Fork boundary, absent Fork lineage, and probe-local
database concurrency. [Spike
0016](evidence/spikes/0016-opencode-runtime-contract.md) adds the native TUI
Runtime, exact session resume, probe-local per-Runtime observer wiring, and
two-runtime noninterference evidence. [Spike
0017](evidence/spikes/0017-opencode-fresh-session.md) proves blank New binding,
endpoint ownership, and a persistent host-owned observer sidecar on OpenCode
`1.18.11`.

`ProviderKind` is first-class, typed, persisted Workstream identity. Models,
effort, roles, agents, prompts, and presets remain entirely native-provider
choices. Ordinary `n` from an existing Workstream retains its exact host and
ProjectLocation. The host supplies providers eligible for fresh launch, exact
resume, and observation: one is selected without prompting, while multiple
providers open a provider-only chooser initially selecting the source
Workstream's provider when eligible. The empty navigator still performs host
and ProjectLocation registration first. Cross-provider work is an independent
New Workstream with an empty conversation, never Fork, migration, or automatic
context transfer.

D8.0 completed on 2026-08-05. It introduced the provider identity and dispatch
foundation, preserved Codex behavior, and made provider kind visible without
adding OpenCode production launch. D8.1 completed on 2026-08-06 with the
contract-validated OpenCode New/Resume vertical slice, acceptance-tested on
`1.18.11`, and provider-aware creation flow. D8.2's original functional
acceptance completed on 2026-08-06, but later process inspection reopened its
cleanup gate. The corrective implementation now covers exact-session
lost-Runtime recovery, same-provider Fork, conservative lost-response handling,
durable blank-session creation, and exact provider-process-group cleanup;
operator-gated production reacceptance remains pending. There is no generic
provider onboarding, provider view
or filter, model selector, role/preset system, or remembered per-Project
provider policy in D8. Availability is dynamic host-owned snapshot state
rather than immutable client-registration identity, and every creation action
revalidates it on the authoritative host.

## 2026-08-04 terminal-fidelity root cause is upstream tmux

[Spike 0014](evidence/spikes/0014-terminal-fidelity-a-b.md) built the
deterministic A/B instrument and proved that the nested presentation re-emits
~2.4-2.6x the cursor-motion sequences of a direct single-tmux baseline for
identical output. Follow-up probing identified the root cause: **upstream tmux
behavior, not a WSNav configuration fault**. On every full client redraw,
tmux emits `civis` (`CSI ?25 l`) before synchronized output and `cnorm`
(`CSI ?25 h`) after it, even when the pane cursor is visible. On terminals
where cursor-state updates restart blinking (Ghostty included), repeated
redraws during streaming visibly disrupt the blink phase. This is documented
as [tmux issue 5419](https://github.com/tmux/tmux/issues/5419).

The following WSNav-controllable candidates were each ruled out with the
instrument and left the `civis`/`cnorm` emission unchanged:

- `set -g cursor-style block` (steady, non-blinking) - only selects the cursor
  shape; the hide/show toggle during redraw is independent;
- `set -g extended-keys always` / `terminal-features` from commit `c0ce139`;
- `set -g update-scroll-region on`; and
- the `sync` (`CSI ?2026`) terminal feature, which is already active for
  Ghostty clients.

The fix is version-bound: tmux `3.7b` (current Arch `extra`) has the behavior,
the AUR `tmux-git` package is stale, and upstream master does not yet contain
the fix. WSNav therefore keeps its best-available private-server configuration
and defers a fix until an upstream tmux release includes it. Revisit this note
when tmux ships the `#5419` fix; the instrument's `nested_motion_not_amplified`
and `nested_bytes_not_amplified` assertions are the objective confirmation
gate.

## 2026-08-02 deferred terminal-fidelity studies

The retained presentation topology is Ghostty -> private presentation tmux ->
private Runtime tmux -> Codex. [Spike
0005](evidence/spikes/0005-codex-terminal-presentation.md) proves the required
native input, resize, reconnect, and result-tip behavior, but does not claim
pixel- or cursor-identical rendering. In current live use, minor cursor
artifacts remain during typing and agent streaming after removal of continuous
runtime and presentation-tmux control probes.

This is accepted, non-blocking V1 polish. It does not approve a delivery slice
or relax private-Runtime, native-UI, input, or result-tip invariants. Revisit
only if the artifact causes input loss, measurable latency, persistent tearing,
or otherwise crosses the operator-quality bar.

Before any presentation redesign, run these disposable, privacy-safe studies:

1. **Topology A/B.** Compare a direct single-tmux Codex pane with the retained
   presentation-plus-Runtime attachment under the same Ghostty dimensions,
   fixed harmless prompt, typing interval, and bounded streaming turn. Record
   only aggregate cursor-artifact, input-latency, redraw, and cleanup results.
2. **Terminal-contract matrix.** Vary only documented private tmux terminal
   settings (`xterm-ghostty`/`tmux-256color`, RGB, extended keys, mouse, focus,
   and cursor behavior) in disposable servers. Keep the actual interactive
   nested path; do not alter ordinary tmux or user terminal configuration.
3. **Control-plane audit.** Prove that steady-state attachment has no repeated
   tmux control clients, then separately measure the unavoidable hook/action
   probes so a visual result is not attributed to hidden management traffic.
4. **Topology alternatives, only if the first studies fail.** Evaluate whether
   an alternative can retain one private tmux server per Runtime, direct native
   interaction, exact recovery, and no provider-pane management traffic. A
   proposal that drops any of those invariants is a rejected workaround, not a
   polish fix.

An eventual candidate passes only when the nested case has no distracting
cursor artifacts across a bounded typing and streaming run, retains normal
input/resize/reconnect/result-tip behavior, leaves ordinary tmux unchanged,
and cleans up all disposable state.

**Resolution:** Study 1 (Topology A/B) was implemented as [Spike
0014](evidence/spikes/0014-terminal-fidelity-a-b.md), and the resulting
investigation is recorded in the 2026-08-04 note above: the artifact is
upstream [tmux issue 5419](https://github.com/tmux/tmux/issues/5419),
version-bound, and WSNav defers a fix until the upstream release. The
terminal-contract matrix and control-plane audit were subsumed by that probe
work (the matrix candidates and control-plane probes were each ruled out with
the instrument). Topology alternatives are not scheduled unless the upstream
fix fails to resolve the visible artifact.

## 2026-08-02 lifecycle evidence — native `/new` remains unsupported

[Spike 0011](evidence/spikes/0011-codex-native-new-rebinding.md) falsified a
`SessionStart(source=new)` changed-binding candidate. [Spike
0012](evidence/spikes/0012-codex-new-prompt-session-rotation.md) likewise
falsified a changed first-destination-prompt hook identity. [Spike
0013](evidence/spikes/0013-codex-new-thread-inventory.md) establishes the
remaining provider fact: native `/new` does create a distinct Codex thread.
That inventory evidence cannot identify which live TUI owns the thread when
more than one TUI shares a project root, so it is not authority to rebind a
Workstream.

Operator instruction: do not use native `/new` in a WSNav-managed Codex pane.
Use `/clear` for a fresh chat in the same Workstream, or WSNav Start/Fork for a
separate Workstream. This records a fail-closed product boundary; it neither
approves a delivery slice nor weakens exact Runtime identity requirements.

## 2026-08-01 design correction — project-root-only workstreams

The earlier D4/D6.1 worktree-management scope is retired. WSNav now registers a
canonical project root, launches every independent and forked Workstream at
that same root, and leaves all Git worktree/branch/file decisions to the user
or Codex inside the native session. The prior worktree evidence remains
historical only; it is not a current product commitment. Host schema 8 is a
clean breaking boundary and requires explicit reset/re-registration instead of
an automatic migration from the retired schema.

Date: 2026-08-06

Status: D0 through D8.1 are complete. D8.2 corrective implementation and
production reacceptance remain in progress. V1 remains a source-installed
operator beta.

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
| D5.2 | Correctness closure for release, identity, recovery, and presentation | Complete |
| D6 | Source-installed operator-beta closure | Complete |
| D6.1 | Repository identity and cross-host Project grouping polish | Complete |
| D6.2 | Navigator shortcut-reference polish | Complete |
| D6.3 | Cross-host activity ordering polish | Complete |
| D6.4 | Navigator grouping and visual-hierarchy polish | Complete |
| D6.5 | Project-marker collision correction | Complete |
| D6.6 | Project-label accent refinement | Complete |
| D6.7 | Compact context hierarchy refinement | Complete |
| D6.8 | Activity-age hierarchy refinement | Complete |
| D6.9 | Codex observer authority repair | Complete |
| D7 | Navigator workflow and lifecycle management | Complete through D7.6 |
| D7.6 | Host-private Project directory browser | Complete |
| D8.0 | Provider identity foundation and Codex parity | Complete (2026-08-05) |
| D8.1 | Provider-aware New and OpenCode New/Resume vertical slice | Complete (2026-08-06) |
| D8.2 | OpenCode Fork, recovery, and integrated acceptance | Corrective implementation; production reacceptance pending |

The completed checkpoints describe the source-installed operator-beta at the
time of their acceptance. [Spike 0009](evidence/spikes/0009-codex-hook-environment-boundary.md)
subsequently falsified its launch-environment observer authority. [Spike
0010](evidence/spikes/0010-codex-hook-ancestry-authority.md) validates a strict
PID-plus-birth-plus-cwd candidate on Codex 0.146.0, but no production rework is
included in those completed checkpoints. D6.9 implements that candidate and
requires a fresh native trust review before it admits any observer-derived
lifecycle state.

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

Recorded evidence: [D1 local native-Codex acceptance](evidence/acceptance/d1-local-codex.md)
and its [D1.5 reconciliation fixture](../spikes/fixtures/d1.5-local-codex-reconciliation.json).

## D2 - Minimal navigator

Deliver the first normal user-facing terminal workflow.

Implementation status: complete. The bounded local snapshot, private
presentation tmux owner, direct attachment helper, Ratatui navigator,
disposable isolation acceptance, and operator-trusted native Codex terminal
acceptance passed. See the [D2 local navigator acceptance](evidence/acceptance/d2-local-navigator.md).

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
acceptance](evidence/acceptance/d3-control-plane.md) and its [sanitized fixture](../spikes/fixtures/d3-ssh-control-plane.json).

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

Implementation status: superseded by the project-root-only correction above.
The retained provider-fork acceptance evidence remains useful for settled-turn
lineage and result preservation, but not filesystem behavior.

Scope:

- independent Workstream creation at the registered project root;
- exact settled-prefix App Server conversation fork from a running source;
- bounded provisional native fork naming;
- destination native resume at the same registered project root; and
- lost-response fork reconciliation without retrying an ambiguous
  non-idempotent provider operation.

Exit gate:

- independent and forked Workstreams have distinct IDs, Runtimes, and
  ConversationTips while retaining the ProjectLocation root;
- a fork sees the last settled source turn and never the source's running turn;
- the source continues unchanged while the destination diverges;
- zero or multiple recovery candidates remain `recovery_required`; and
- WSNav never creates, validates, or changes Git worktrees.

The disposable local harness (`scripts/d4-local-workstream-acceptance.sh`) and
parser/state/transport tests drive a source with one completed turn followed by
an in-progress turn, assert that the provider request names only the completed
turn and keeps the registered root as its cwd, and compare the ordinary tmux
fingerprint before and after cleanup.

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
[D5 acceptance record](evidence/acceptance/d5-v1-closure.md).

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
full local repository gates. See the [D5.1 operational closure acceptance](evidence/acceptance/d5.1-operational-closure.md).

Close the release-quality gaps found by the post-D5 broad review without
expanding the approved V1 product.

Scope:

- recover an independently started Workstream through its normal open path,
  and reconcile an exact unresolved Fork through local CLI, SSH protocol, and
  navigator visibility;
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

- a simulated client loss after independent Start resumes from its durable
  Workstream row, while an unresolved Fork reconciles from its exact opaque
  operation without its original request key;
- a stale or missing remote executable is diagnosed before stateful control,
  while matching hosts continue to work through local-subprocess and SSH
  protocol paths;
- every affected child process is output-bounded while it is read, not after it
  is buffered;
- first-run guidance requires an explicit checkout registration rather than a
  guessed current directory;
- the complete repository gate, package verification, disposable acceptance,
  and privacy audit pass.

## D5.2 - Correctness closure

Implementation status: complete. The declared Rust 1.88 toolchain now matches
the locked production graph and passes its pinned CI job. Stable project
labels, fail-closed tmux probes, authority-before-provider launch, observable
attachment outcomes, process-group deadlines, scrolled mouse targeting, and
cursor-paged snapshots passed the full local and disposable gates. See the
[D5.2 correctness closure acceptance](evidence/acceptance/d5.2-correctness-closure.md).

Reopen V1 closure for the concrete contradictions found by the post-D5.1
project-wide review. This is hardening of the approved product, not a new
workflow layer.

Scope:

- declare and test an MSRV compatible with every locked production dependency;
- derive remote project labels from the stable ProjectLocation repository
  rather than a generated managed-checkout basename;
- classify only a conclusively absent private tmux server as missing;
- make provider attachment completion or failure observable outside the
  provider pane, with an exact same-row retry path;
- enforce wall-clock deadlines as well as retained-output bounds on finite
  local control commands;
- retain the rendered list offset for exact mouse targeting after scrolling;
  and
- page bounded host snapshots so retained Workstreams do not make the
  navigator unusable at one fixed row count.

Exit gate:

- the pinned MSRV job is consistent with locked dependency metadata and passes;
- local and remote independent/forked Workstreams retain one stable project
  label;
- unavailable or malformed tmux evidence remains `unknown` and cannot authorize
  recovery cleanup;
- a failed local or SSH attachment is reported only in the navigator and the
  selected row can be retried without switching away;
- stalled finite tmux, Git, capability, and child-CLI commands time out without
  leaving their process group alive;
- mouse activation selects the visible row after vertical scrolling;
- multi-page snapshots preserve deterministic order and bounded frames; and
- `scripts/check`, package verification, disposable acceptance, privacy audit,
  and `git diff --check` pass.

## D6 - Source-installed operator-beta closure

Implementation status: complete. Present-tense documentation, the explicit
source-installed distribution posture, exact-candidate local/SSH release
parity, clean navigator shutdown, and bounded native operator smoke passed. See
the [D6 operator-beta acceptance](evidence/acceptance/d6-operator-beta.md).

Close the implemented V1 as an operator-ready beta without adding another
workflow or changing the approved ownership boundaries.

Scope:

- reconcile the README, design, roadmap, and acceptance index with the final
  D5.2 behavior and remove stale pre-implementation wording;
- run the complete repository gate from the exact candidate commit;
- run one bounded local-plus-SSH operator smoke against matching builds,
  exercising native attachment, switching, parking, reopening, status
  visibility, mouse focus, and result-tip preservation;
- corroborate attachment failure, same-row retry, scrolled mouse targeting,
  snapshot paging, and process cleanup through the disposable automated gates
  rather than injecting failures into an operator's active provider pane;
- record sanitized release, isolation, cleanup, and privacy evidence; and
- state the V1 distribution posture explicitly.

Distribution posture:

- D6 remains a source-installed operator beta at version `0.1.0`;
- local and remote executables are built from the same reviewed commit and
  installed by the operator;
- no tag, hosted binary, automatic remote deployment, update service, or Cargo
  publication is implied by V1 acceptance; and
- a public binary release is a separate post-D6 product decision.

Exit gate:

- top-level documentation describes D0 through D6 in present tense and links
  every final acceptance record;
- the current stable toolchain and declared Rust 1.88 gate pass against the
  locked dependency graph;
- the exact candidate build completes the bounded local-plus-SSH smoke without
  provider-pane management text, result-tip loss, or default-tmux mutation;
- disposable tests cover destructive failure injection and remove all owned
  temporary state;
- committed evidence contains no provider or Workstream identifiers, prompts,
  results, terminal captures, paths, process IDs, credentials, or raw payloads;
  and
- the repository is clean, synchronized, and remains explicitly
  source-installed rather than presenting an unshipped release channel.

## D6.1 - Repository identity and cross-host Project grouping polish

Implementation status: revised. Canonical remote fingerprinting, safe origin
labels, and client-side cross-host Project grouping remain current. Linked
worktree input is normalized to the primary project root rather than retained
as a separate workstream cwd; the development schema migration is superseded
by the explicit host-state reset boundary.

Refine the accepted operator beta without changing provider or Runtime
ownership. Make the existing client-side Project concept useful when
the same repository is registered at different paths or on different hosts.

Scope:

- normalize new registrations, including linked-worktree input, to the primary
  Git project root;
- derive one credential-free, transport-normalized fetch-remote fingerprint
  and safe `host/path` display label through bounded local Git inspection
  without network access;
- expose only that opaque fingerprint, safe origin label, and bounded
  repository name through a versioned host snapshot;
- reuse a client Project ID when exact fingerprints match, while keeping
  missing or ambiguous identities separate;
- fail closed on retired development schemas without importing the Python
  prototype or weakening matching-build remote checks; and
- retain per-host Location and Workstream authority beneath the presentation
  grouping.

Exit gate:

- linked-worktree registration launches at the primary project root;
- SSH and HTTPS spellings of the same fetch remote group across host locations;
- forks with different `origin` remotes, ambiguous remotes, local-path remotes,
  and repositories without remotes do not group automatically;
- no URL, credential, repository path, common-directory path, provider payload,
  or terminal content crosses the host protocol or enters committed evidence;
- development-schema migration, protocol validation, full tests, lint,
  packaging, and `git diff --check` pass; and
- matching local and remote builds remain required after the protocol/schema
  revision.

## D6.2 - Navigator shortcut-reference polish

Implementation status: complete. The navigator keeps a compact footer and
renders its complete shortcut reference only within its own Ratatui pane. The
full automated repository gate passed without changing any Runtime, provider,
or presentation-tmux ownership boundary. Later D7 presentation refinement
keeps that reference at the bottom of the pane, keyboard-only, and omits
self-close and mouse instructions; the original centered-overlay wording below
is retained as the historical D6.2 delivery shape.

Polish the existing navigator without adding a workflow, new durable state, or
another terminal surface. The full control reference must be easy to discover
without consuming the small navigator pane during ordinary work.

Scope:

- replace the long persistent shortcut sentence with a compact `? help` footer
  while retaining registration, recovery, and unavailable-host warnings;
- render a centered shortcut reference only inside the navigator's Ratatui
  pane, never as a tmux popup/window or inside the provider pane;
- make the overlay a local modal: `?`, `Esc`, and `q` dismiss it, while its
  remaining keyboard and mouse inputs cannot activate or mutate a Workstream;
  and
- cover footer priority, overlay rendering, and local modal state with
  deterministic terminal tests.

Exit gate:

- help is discoverable in normal, empty, unavailable, and recovery states;
- it never writes to, resizes, replaces, or overlays the native provider
  surface;
- action keys cannot perform a Workstream operation while the reference is
  visible; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.3 - Cross-host activity ordering polish

Implementation status: complete. The combined navigator projection now orders
known local and remote activity timestamps newest first, then uses stable
identity fallbacks without changing host or provider authority.

Make the navigator's global row order agree with its visible relative-activity
labels. This is a client-side presentation correction only; it must not turn a
remote wall clock into state or action authority.

Scope:

- after combining local and cached remote projections, sort known activity
  timestamps newest first across hosts;
- keep unknown activity after known rows; and
- use stable host, Project, and Workstream identity fallbacks for equal or
  unknown timestamps.

Exit gate:

- a deterministic test proves local and remote rows interleave by recency,
  with stable fallbacks;
- no host schema, remote protocol, Runtime, or provider contract changes; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.4 - Navigator grouping and visual-hierarchy polish

Implementation status: complete. The navigator now has local-only Recent,
host, and Project views with quiet dual-axis context cues, group-header-safe
selection, and deterministic terminal coverage. Later navigation refinement
cycles the current `Recent`, `By project`, `By host`, and `Archived` views with
`Left`/`Right`; the original `v` delivery wording below is historical. No
durable or provider-facing behavior changed.

Make multi-host and multi-Project navigation easier to scan without adding a
dashboard, a new durable preference, or a second control surface. The native
provider pane remains the primary visual focus.

Scope:

- add local-only `Recent`, `By host`, and `By project` navigator views, cycled
  with `v`;
- keep global recency as the default and use first visible activity to order
  groups and rows within a group;
- render non-actionable group headers while preserving keyboard selection,
  same-row activation, and correct mouse targeting for Workstream rows;
- use quiet, deterministic host-label accents and project-marker accents;
  preserve neutral row text and reserve green, yellow, and red for lifecycle
  state; and
- make the active view discoverable in the border title, footer, and help
  overlay.

Exit gate:

- deterministic tests cover view cycling, host and Project group construction,
  neutral header clicks, scrolling mouse targeting, and color-marker rendering;
- selection and provider attachment continue to refer only to exact host and
  Workstream identity, never a group header or display label;
- no host schema, remote protocol, Runtime, or provider contract changes; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.5 - Project-marker collision correction

Implementation status: complete. Visible Project markers now use a
collision-resolved muted 256-color palette while retaining neutral row text
and the existing host/status color boundaries.

Correct the D6.4 marker palette so concurrently visible Projects do not land on
the same accent merely because a small independent hash palette collided. Keep
the accent quiet, bounded, terminal-safe, and presentation-only.

Scope:

- replace the four-color Project hash with a curated muted 256-color marker
  palette;
- allocate distinct colors to the first twelve visible Project identities using
  deterministic collision probing; and
- preserve neutral Project/Workstream text, host-label accents, and reserved
  lifecycle-state colors.

Exit gate:

- deterministic rendering tests prove all twelve visible Project markers are
  distinct and stable for an unchanged projection;
- no color becomes durable identity, action authority, provider traffic, or a
  host/protocol contract; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.6 - Project-label accent refinement

Implementation status: complete. Compact Project labels and Project headers
now carry their same muted marker accent; host labels, Workstream titles, and
lifecycle-state colors remain separate.

Promote the existing quiet Project accent from marker-only decoration to the
compact Project text the user actually scans, without coloring Workstream
titles, host labels, or lifecycle state.

Scope:

- apply each visible Project's collision-resolved muted accent to its compact
  flat/host-view label and project-group header;
- retain the small marker as a visual anchor; and
- preserve the separate host-label axis, neutral Workstream title, selected-row
  background, and reserved lifecycle-state colors.

Exit gate:

- terminal rendering tests prove the compact Project label and Project header
  use the same allocated accent as their marker;
- no durable state, control path, Runtime, or provider behavior changes; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.7 - Compact context hierarchy refinement

Implementation status: complete. The flat recent view now uses one neutral
separator between colored host and Project labels; host accents are cool blue
and Project accents are muted violet.

Reduce visual noise in the compact context line without changing its identity
or control semantics. Preserve readable host/Project names while making their
separate identity axes immediately apparent.

Scope:

- remove the redundant Project marker from the flat `host · Project` context
  line, retaining one neutral separator;
- use a bounded cool-blue palette for local and remote host labels; and
- use a separate collision-resolved muted-violet palette for Project marker,
  compact label, and Project-group header.

Exit gate:

- deterministic tests prove the recent context has exactly one neutral
  separator and no redundant marker, and that host and Project palettes do not
  overlap;
- Workstream titles, selected-row treatment, lifecycle colors, navigation,
  Runtime state, and protocol contracts remain unchanged; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.8 - Activity-age hierarchy refinement

Implementation status: complete. The age beside each neutral Workstream title
now uses a quiet staleness scale: dim for fresh work, neutral gray through the
same day, light neutral through the week, and a muted warm accent thereafter.

Make the numerical activity age legible as a secondary time signal without
competing with the title, host/Project identity, or lifecycle indicator.

Scope:

- keep the age separator neutral and the Workstream title white;
- derive age styling from the bounded observed activity timestamp, treating a
  missing timestamp as dim rather than inventing recency; and
- reserve saturated green, yellow, and red exclusively for lifecycle state.

Exit gate:

- deterministic tests cover the fresh, same-day, same-week, stale, and
  missing-timestamp age-style boundaries;
- no age color becomes durable state, action authority, or protocol data; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D6.9 - Codex observer authority repair

Implementation status: complete. The observer no longer depends on launch
environment values that Codex strips before invoking a hook. Its exact owned
profile now passes the canonical private state root as a quoted command
argument, then accepts one hook only after it matches a single live WSNav
Runtime by direct parent PID, process birth, and cwd.

Scope:

- retain stdin draining before all state and authority checks, so unmanaged or
  rejected large payloads cannot cause a broken pipe;
- replace the removed environment authority with a static profile command and
  a private-registry candidate scan bounded to live, process-fingerprinted
  Runtimes;
- reject wrappers, stale or ambiguous candidates, wrong private tmux panes,
  process births, and cwd values before any state mutation;
- version profile ownership, require explicit update of the old declaration,
  and return the observer to native-trust pending; and
- retain exact legacy-profile removal without silently replacing an old
  declaration.

Exit gate:

- unit tests cover static command generation, legacy update/removal,
  process-fingerprinted candidate selection, and direct-parent-only ancestry;
- the full repository check and package-content gates pass; and
- a live host update parks all managed Runtimes, performs the explicit native
  `/hooks` review, and records fresh lifecycle activity without provider-pane
  output.

## D7 - Navigator workflow and lifecycle management

Implementation status: D7.0 through D7.5 passed the bounded native
local/SSH observer reviews and the integrated disposable/reversible navigator
acceptance. See the [D7 navigator workflow acceptance](evidence/acceptance/d7-navigator-workflow.md).
D7.1 supplies the Workstreams, Projects, and Hosts navigation foundation;
D7.2 now supplies revision-guarded archive/restore through local and SSH host
contracts, the Archived navigator view, bounded Workstream status,
canonical rename, and exact local/remote unresolved-operation reconciliation.
D7.3 now exposes bounded host-owned ProjectLocations with active/archived
counts and registers existing local or SSH checkouts through navigator-local
forms. New Workstreams remain a Workstreams-home action, keeping Projects
strictly for Project management. D7.4 now adds
an active-Project tree per host plus streamlined host onboarding: add verifies,
registers, prepares the observer, and opens native review; removal explicitly
chooses client-only disconnect or guarded observer offboarding.
D7 makes ordinary WSNav administration available through the navigator without
turning it into a task manager or replacing the provider surface.

Scope:

- make the two-pane TUI sufficient for every ordinary WSNav-owned operation
  after external installation prerequisites, with CLI commands retained only
  as optional scripting, diagnostics, direct attachment, and break-glass
  parity;
- retain Workstreams as the default page and add sibling Projects and Hosts
  pages inside the existing navigator pane, with mouse and keyboard switching,
  inline inventory detail, and page-specific help;
- retain page-local single-key actions as the canonical terminal control path,
  with a separate status line, a compact action-boundary-wrapped key strip, and
  a `?`-toggled single-column expanded reference at the bottom of the pane;
- add reversible Workstream archive/restore as a visibility concern separate
  from runtime lifecycle, preserving provider binding, attention, lineage,
  Project location, and native history;
- expose bounded Workstream status, canonical rename, attention acknowledgement,
  and exact unresolved-operation recovery through the Workstreams page;
- add Project inventory and local/remote ProjectLocation registration without
  cloning, syncing, deleting, or exposing remote repository paths, including
  the empty-navigator flow; Projects remain management-only surfaces;
- add Host registration that verifies, prepares, and opens native observer
  review as one flow; render active Projects per host; and offer explicit
  client-only disconnect or guarded exact-observer offboarding while protecting
  the local host and leaving retained remote state untouched;
- keep bare and explicit `wsnav navigator` as the host-local observer
  activation entry point, retaining setup/update commands only as hidden
  diagnostics; and
- run required local or remote native `/hooks` review in the right provider
  pane with a disposable cwd, never in a Workstream or existing agent session.

Delivery slices:

1. **D7.0 - Observer activation closure.** Complete bounded native acceptance
   of the implemented host-local activation. Creation or exact legacy migration
   remains restricted to hosts with no live managed Runtime; native review uses
   a disposable cwd in the right pane, verifies trust only after the TUI exits,
   and fails closed on foreign profiles, failed review, or ambiguous state.
2. **D7.1 - Management navigation foundation.** Add the Workstreams home and
   its Projects and Hosts child pages, inline inventory rows, mouse behavior,
   and direct page-local keys without changing provider state. Refine the
   narrow Workstreams pane with two-line
   Recent rows, explicit two-line tree children in grouped views, the `Recent`
   / `By project` / `By host` / `Archived` cycle, compact bottom key hints, and a
   single-column expanded reference while retaining the accepted Workstreams
   bindings. Each later stateful action owns its bounded text entry,
   confirmation, and non-blocking progress path; D7.1 deliberately does not
   ship an unused generic modal.
3. **D7.2 - Workstream lifecycle and recovery.** Add bounded status and
   canonical rename, preserve existing open/new/fork/park/acknowledge keys, add
   revision-guarded local/remote archive visibility and restore-without-start,
   and make exact unresolved Fork reconciliation available by repeating `f` on
   its source Workstream. That focused path holds the opaque operation handle
   only long enough to issue exact local or SSH reconciliation; multiple
   candidates use a bounded source-scoped chooser. Archive/restore, scope
   selection, bounded status, canonical rename, and recovery are complete.
4. **D7.3 - Project management.** List logical Projects and their host-owned
   locations, show active/archived counts, and register the first or an
   additional existing checkout on a selected local or SSH host. Projects stay
   management-only; `n` on the Workstreams home creates an independent
   Workstream from a selected active Workstream, while an empty navigator uses
   the registration flow. Location inventory, counts, and navigator-local
   local/SSH checkout registration are complete.
5. **D7.4 - Host management.** Add SSH hosts through a single
   verify/prepare/native-review flow, show their active Project trees, and
   choose either client-only disconnect or guarded observer offboarding through
   the navigator. Preserve client/host ownership boundaries and carry the
   native review boundary proven in D7.0 through the remote Hosts-page flow.
6. **D7.5 - Integrated acceptance.** Exercise fresh local and remote setup,
   Project registration, Workstream lifecycle/recovery, guarded observer
   offboarding, and host disconnect/re-register using only the two-pane TUI
   after installation, without provider-pane management traffic or remote
   Runtime interference.
7. **D7.6 - Host-private Project directory browser.** Replace the ordinary
   typed checkout-path form with a navigator-only host picker followed by a
   bounded directory browser. Each host defaults to `~/code` and exposes an
   explicit Hosts-page root setting. The protocol returns only a safe root
   label, relative cursor, and direct-child names; host-side registration
   reconstructs the chosen directory locally. The direct `register` and
   `host register-checkout` commands remain optional scripting and break-glass
   paths. This slice is complete.

Exit gate:

- deterministic tests cover page navigation, modal input isolation,
  confirmation, duplicate-action suppression, status/action-line separation,
  compact/expanded key-help state, narrow-width row truncation, two-line mouse
  targeting, explicit grouped-tree rendering, view-cycle order, archive/restore
  revisions, ProjectLocation ownership, remote path bounds, host forget
  semantics, and local-host protection;
- observer tests retain fresh activation, untrusted-ready reconciliation,
  live-runtime refusal, hidden diagnostic commands, and shell-free temporary
  pane launch;
- native acceptance proves local and remote observer review occurs in the right
  pane, leaves no Workstream/runtime behind, and marks only the exact profile
  ready after native approval;
- local-plus-SSH acceptance proves archive removes a Workstream from the active
  view without deleting Git or provider state, restore does not start Codex,
  and forgetting a host does not mutate that host;
- a greenfield post-install acceptance uses bare `wsnav` for local observer
  approval, first Project registration, SSH host and remote-location
  registration, Workstream start/fork/rename/park/recover/acknowledge/archive/
  restore, unresolved-operation recovery, guarded observer offboarding, and
  host disconnect/re-register without entering another `wsnav` shell command;
- the native provider result and input surface remain untouched until the user
  explicitly chooses a Workstream or observer-review action; and
- D7.6 adds protocol, state, local-subprocess, and terminal tests proving a
  browser response cannot carry an absolute project path, relative cursors
  cannot escape the selected root, and a selected Git directory registers only
  through host-side resolution; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## D8 - Multi-provider Workstreams

Implementation status: D8.0 completed on 2026-08-05 and D8.1 completed on
2026-08-06 after its fresh-session, observer, mixed-provider, and real-provider
acceptance passed on OpenCode `1.18.11`. D8.2's original functional acceptance
completed on 2026-08-06, but its cleanup gate was later falsified; corrective
implementation and production reacceptance remain in progress. The
[multi-provider design](design.md#multi-provider-and-multi-agent-design) is
authoritative for the shared provider boundary and privacy invariants.

The product goal is deliberately narrower than provider orchestration. A user
can start independent Workstreams at the same ProjectLocation, choose among
providers eligible on the authoritative host, and use each provider's
native TUI to select models, effort, agents, and workflow. Project files and
user-authored notes may carry context between Workstreams; WSNav does not copy
conversation state or invent a cross-provider handoff.

New retains the D7 location behavior:

- `n` on an existing Workstream fixes the target to its host and exact
  ProjectLocation. It does not ask for a Project or location again.
- `n` from an empty Workstreams home performs host and ProjectLocation
  registration before creating the initial Workstream.
- zero providers eligible for fresh launch plus exact resume and observation
  rejects creation; one is selected without a prompt; multiple open a
  provider-only chooser. The source provider is the initial selection when it
  remains eligible.
- availability is detected without installation, credential configuration,
  trust mutation, or generic onboarding. The host revalidates the selected
  provider at the action boundary and never silently falls back to another;
- the selected provider is fixed Workstream identity. Resume and same-provider
  Fork never reopen the chooser; a different-provider conversation is always
  another independent New Workstream.

Presentation adds only a quiet `Codex` or `OpenCode` context label on each
Workstream. Provider filters, provider grouping, roles, presets, model and
effort fields, remembered per-Project provider policy, and generalized
provider-management UI have no approved checkpoint.

### D8.0 - Provider identity foundation and Codex parity

Status: Complete on 2026-08-05.

Scope:

- introduce `ProviderKind` as a validated enum on Workstream, Runtime,
  ProviderBinding, host snapshots, creation actions, and provider session IDs;
- migrate host schema 9 to 10 transactionally by assigning `codex` to every
  existing Workstream and ProviderBinding, validating the existing Runtime
  provider, and rejecting any unknown or cross-record mismatch; migrate client
  schema 4 to 5 so the former `codex` executable bit no longer participates in
  fixed host registration while retaining aliases and Project associations;
- bump protocol 16 to 17 for the incompatible provider-bearing creation and
  snapshot wire contract while leaving the independently versioned control ABI
  unchanged;
- introduce provider-neutral session identity, lifecycle observation, name
  state, capability, and error DTOs wherever shared state/wire/navigator code
  currently imports a concrete Codex type. Keep the existing concrete Codex
  implementation and add only the typed Codex action-dispatch branch; do not
  implement or simulate an OpenCode surface in D8.0;
- carry bounded, sorted, duplicate-free `ProviderCapability` records on every
  snapshot page and exclude them from the persistent client host identity.
  New eligibility requires available fresh launch, exact resume, and observe;
  the authoritative host re-probes before creation. D8.0 reports OpenCode as
  `unavailable/adapter_unavailable` even if its binary is installed;
- make New and empty-state registration carry an explicit provider. The UI
  selects the sole eligible Codex record without a chooser; internal host wire
  always requires the kind; direct New defaults only to its eligible source
  provider; and direct registration requires `--provider` only when more than
  one eligible provider exists;
- make independent-creation deduplication compare provider kind, reject stale
  or changed-provider request replay, and retain a fixed-provider visible
  recovery state if process launch fails only after successful eligibility
  revalidation and durable creation;
- render the provider label in Workstream rows and details, and replace action
  feedback that incorrectly hardcodes Codex where the provider is known. The
  full provider label remains visible at the supported 32-cell width before
  variable Project/Host context is truncated; and
- preserve all current Codex CLI, local, SSH, observer, Runtime, recovery, and
  native-pane behavior. No OpenCode production process is launched in D8.0.

Exit gate:

- schema and protocol tests cover host 9-to-10 and client 4-to-5 Codex
  migration without lost associations, fresh-schema explicit provider writes,
  protocol-16 refusal, unknown provider rejection, namespaced native session
  identity, and
  Workstream/Runtime/Binding mismatch;
- capability tests cover bounds/order/duplicates, page inconsistency,
  unavailable/unknown reasons, exact New eligibility, provider installation or
  version drift without host re-registration, and action-boundary revalidation;
- creation tests cover provider-aware request deduplication, deterministic CLI
  defaults/errors, zero-provider refusal before creation, and post-creation
  launch failure without fallback;
- navigator tests prove `n` retains the selected Workstream's ProjectLocation,
  the sole Codex provider creates no chooser, and every Workstream visibly
  identifies its full provider at minimum width without adding a provider view
  or filter;
- existing local and SSH Codex tests remain behaviorally unchanged through the
  provider identity and dispatch seam, and bounded native acceptance confirms
  launch, observe, resume, Fork, attachment, and cleanup; and
- formatting, tests, lint, package checks, and `git diff --check` pass through
  `scripts/check`.

Completion evidence (2026-08-05): D8.0 now has typed provider identity across
the schema 10 and protocol 17 boundaries, dynamic capability and
action-boundary revalidation, deterministic provider selection, and
provider-visible presentation while preserving Codex's native launch,
observation, resume, and Fork behavior.
The final gates report 277 library tests plus 5 local-transport tests (282
all-target tests), with format, clippy, package/license/advisory, shell, fixture,
and diff checks green through `scripts/check`. Disposable local acceptance
exercised fake-Codex launch and lifecycle observation, settled-prefix Fork,
native attachment with the marker visible through the outer attach driver,
detachment while the private Runtime remained live, exact recovery/resume, and
cleanup with the default tmux server unchanged. No OpenCode production process
launch is claimed.

### Required evidence before D8.1 - OpenCode fresh binding and observer ownership

Status: passed on 2026-08-05 using OpenCode `1.18.11`. This remains an
operator-gated disposable spike, not production Rust; its [sanitized
fixture](../spikes/fixtures/opencode-fresh-session.json) and
[evidence record](evidence/spikes/0017-opencode-fresh-session.md) authorize the
selected D8.1 binding and observation contract. The observed release records
what was tested; it is not production compatibility authority.

The probe must:

- use the production OpenCode TUI command shape without `--pure`, `--model`,
  `--agent`, or `--prompt`, so WSNav does not suppress normal native
  configuration or plugin semantics;
- use the selected blank provider-session precreation through a short-lived
  server, then prove two blank same-project TUIs bind distinct exact root
  sessions without using transcript content for identity, title/recency
  inference, or event crossing; disposable postcondition checks are discarded;
- start the observer before either provider pane accepts native input, record
  and verify its exact PID/process birth/Runtime generation, exercise bounded
  reconnect plus helper crash, and prove detach/reopen retains host-side
  observation;
- correlate each loopback listening socket to the exact recorded provider pane
  process or proven descendant, and reject a healthy wrong-process endpoint,
  stale saved port, port collision, changed or malformed health metadata,
  child session, and unrelated root session;
- prove exact resume after park/restart. Native in-TUI session
  creation/switching remains unsupported because no exact active-TUI
  changed-binding claim was established, matching Codex native `/new`; and
- validate only metadata surfaces intended for D8.1. Navigator Rename remains
  unavailable unless canonical OpenCode rename is separately proven.

The fixture records only bounded assertions and identifier digests. Cleanup
removed all disposable provider roots, temporary auth copies, sidecars, ports,
processes, and private tmux servers. A failed identity, native-workflow,
privacy, persistence, or cleanup assertion in a future rerun keeps the adapter
inactive and narrows it rather than weakening a core invariant.

Production-status correction (2026-08-05): a disposable fresh OpenCode
`1.18.11` start falsified the earlier assumption that a known idle root always
appears in `/session/status`; the real endpoint returned an empty status map
while `GET /session/:id` returned exact root metadata (`id`, `directory`, and
no `parentID`; an explicit JSON `null` is also accepted). A child response in
the same probe carried a non-null `parentID` matching its root. The adapter now
corroborates that exact metadata before interpreting the status map, and treats
an absent root entry as `Idle` only after that proof. Wrong identity/directory,
child sessions, missing or malformed metadata, and unknown/malformed statuses
remain fail-closed. This correction is included in the completed D8.1
evidence; D8.2 owns the remaining recovery and Fork work.

### D8.1 - Provider-aware New and OpenCode New/Resume vertical slice

Status: Complete on 2026-08-06. The prerequisite evidence checkpoint passed
using OpenCode `1.18.11`, and the design records blank-session precreation as
the selected fresh-binding mechanism. Production eligibility assumes that
contract across releases and validates every consumed surface at its action or
Runtime boundary.

Scope:

- add non-mutating, bounded OpenCode executable/readiness detection to each
  host's dynamic provider availability without probing or storing credentials.
  A successful bounded version command proves installation but never selects
  compatible releases; the launched Runtime must satisfy the exact HTTP/SSE,
  identity, and process contract;
- add the provider-only chooser for a mixed-provider target, initially select
  the source Workstream's provider when eligible, and apply the same chooser
  after empty-state host and ProjectLocation selection but before its initial
  Workstream is recorded;
- make startup provider-scoped: an unready Codex observer cannot block an
  eligible OpenCode adapter, while the Codex-specific Hosts review action
  remains available and creates no setup Workstream;
- implement only the evidence-selected OpenCode blank fresh binding, exact
  resume, session-bound lifecycle observation, and local/SSH attachment. The
  production command supplies no `--pure`, model, agent, prompt, or WSNav-owned
  first input; OpenCode navigator Rename remains unavailable unless proven;
- complete the shared provider boundary so common action/app/state/navigator/
  remote code consumes provider-neutral types and dispatches once from the
  fixed Workstream provider;
- persist the bounded host-private OpenCode Runtime handle and supervise one
  exact stdio-disconnected `wsnav` observer sidecar per Runtime generation.
  Corroborate endpoint-to-process ownership before observation or metadata
  access; fail observation to unknown on helper/endpoint ambiguity; stop the
  exact helper during park; and replace endpoint plus helper on recovery; and
- leave OpenCode Fork unavailable until D8.2. A missing capability produces a
  bounded provider-aware refusal, never Codex fallback or inferred lineage.

Exit gate:

- deterministic state, protocol, navigator, local-subprocess, SSH-command, and
  private-tmux tests cover zero/one/multiple-provider creation, source-provider
  initial selection, action-boundary availability/version drift, exact blank
  binding and resume, provider-scoped Codex readiness, endpoint/process
  correlation, per-generation sidecar identity, reconnect/crash/unknown
  transitions, port collision, child/root-session filtering, native session
  switch refusal when unproven, and cleanup;
- sanitized operator-gated acceptance proves two same-project Codex/OpenCode
  Workstreams remain independent, switching preserves both native TUIs, no
  model, prompt, raw event, or transcript data enters WSNav state, detaching the
  client does not stop host-side observation, and all disposable endpoints,
  sidecars, provider processes, tmux servers, and provider roots are removed;
- `scripts/check` passes.

Tranche-2 evidence (2026-08-05): the shared provider-aware action boundary is
implemented. Local and remote Register/New re-probe the exact selected
provider with no fallback; local and SSH attachment share exact
Runtime/observer/endpoint/session preflight; provider-scoped startup leaves an
eligible OpenCode lane actionable while Codex observer review is pending; and
OpenCode Rename, Fork, and recovery remain bounded no-effect refusals. The
deterministic all-target suite, fake OpenCode/private-tmux acceptance, and
mixed-provider disposable acceptance cover provider identity separation,
independent local and RemoteAttach native attachment/detachment, exact pane
and observer process/port/socket cleanup, bounded launch-flag/privacy checks,
unsupported OpenCode no-effect actions, and ordinary-tmux non-interference.

Real-provider acceptance (2026-08-06) passed the local and real-loopback-SSH
OpenCode Register/New, exact resume, helper crash/Unknown, attach/detach, park,
identity, fixed SSH TTY command, and complete process/socket/provider-root
cleanup checks. A real same-project Codex/OpenCode pair also retained distinct
bindings and native TUIs across Codex to OpenCode to Codex switching, with no
model, prompt, or raw provider marker in WSNav state. The sanitized
[D8.1 acceptance record](evidence/acceptance/d8.1-multi-provider.md) contains
the bounded results.

That mixed run also falsified the native-workflow assumption behind the exact
profile lifecycle: native Codex `/model` selection wrote `model` and
`model_reasoning_effort` before the managed `wsnav-observer` declaration.
Exact profile removal correctly refused the then-unknown modification, and the
complete disposable Codex home was removed only after all Runtimes stopped.

The explicit D8.1 correction preserves a bounded provider-owned prefix
containing only those two opaque string settings while retaining byte-exact
WSNav declaration ownership and the existing narrow native-trust suffix.
Update preserves the prefix; removal leaves it as a model-only foreign profile
instead of erasing the native choice.

Focused reacceptance on Codex CLI `0.146.0` selected Luna/medium through native
`/model` in a disposable profile-selected TUI. Trust reconciliation accepted
the exact three-region document; removal retained the provider prefix with the
same hash and mode `0600`, removed all WSNav declaration/native-trust content,
and a later setup refused to adopt the unowned model-only file. The TUI exited,
no disposable process remained, and the complete provider/state root was
removed. The final `scripts/check` gate passes 314 library tests plus 5 local
transport tests, formatting, Clippy, package/license/advisory verification,
disposable local and mixed-provider acceptances, and diff checks. D8.1 is
complete; D8.2 was activated on 2026-08-06.

### D8.2 - OpenCode Fork, recovery, and integrated acceptance

Status: Corrective implementation in progress after the 2026-08-06 cleanup
falsification; the original real acceptance used OpenCode `1.18.11`, production
compatibility remains contract-based, and a corrective operator-gated rerun is
still required.

Scope:

- implement explicit lost-Runtime recovery for an OpenCode Workstream only
  when its exact bound root session is known and the prior private tmux
  Runtime is conclusively missing. Validate and stop only the recorded
  sidecar by PID plus process birth, remove only the matching old-generation
  handle and private Runtime artifacts, reserve a fresh generation with a new
  endpoint and observer, and resume the same session. An unbound, live, or
  ambiguous Runtime is refused without provider discovery, a native picker,
  session adoption, or fallback;
- implement same-provider OpenCode Fork at the exact settled `messageID`
  boundary and the documented terminal `external_effect_unknown` outcome when
  a response is lost without structural lineage;
- keep cross-provider Fork unavailable and route different-provider work only
  through independent New; and
- complete mixed local/SSH provider acceptance and provider-aware operational
  diagnostics without onboarding, filters, presets, or context transfer.

Exit gate:

- deterministic tests cover exact bound-session Runtime recovery, ambiguous
  and unbound recovery refusal, prior-generation sidecar/handle cleanup,
  settled-prefix Fork exactness, no cross-provider Fork, lost-response terminal
  failure without retry/adoption, and provider-specific diagnostics; and
- sanitized operator-gated local/SSH acceptance plus `scripts/check` pass with
  complete cleanup and no provider-pane management traffic.

Completion evidence (2026-08-06): OpenCode Fork now revalidates the exact live
Runtime, root-session binding, handle, loopback endpoint, provider process,
observer, and last completed `messageID` before crossing one recorded provider
effect boundary. A lost response is terminal `external_effect_unknown`; it is
never retried, reconciled from display text, or adopted. Exact recovery accepts
only a recovery-required Workstream with a conclusively missing private tmux
Runtime, matching bound root session, and matching prior-generation handle and
observer identity, then replaces the generation, endpoint, and sidecar while
resuming that same session.

Real acceptance first exposed two integration races and retained the strict
contract: observer `ready` now follows successful SSE stream establishment, a
trailing provider busy status no longer erases an already completed assistant
candidate before idle corroboration, and only Runtime-creating/recovery SSH
mutations receive the longer 45-second bounded process deadline needed to
contain the adapter's readiness gates. Read-only control remains at eight
seconds.

The operator-gated production harness passed both local and real loopback-SSH
Fork and lost-Runtime recovery on the installed OpenCode release (`1.18.11` in
the recorded run). Each Fork produced a distinct same-provider bound session;
each recovery retained the source
session while replacing generation, endpoint, and observer. WSNav state
contained no provider marker or content, every disposable process, port,
socket, provider root, repository, and SSH artifact was removed, and the
ordinary tmux inventory was unchanged. The sanitized
[D8.2 acceptance record](evidence/acceptance/d8.2-opencode-fork-recovery.md)
contains the bounded original results. Corrective closure now requires the
expanded `scripts/check` gate: all-target Rust plus formatting, Clippy,
package/license/advisory, shell/Python/fixture, disposable
hangup-surviving-provider, mixed-provider, and diff checks.

Cleanup falsification and corrective closure (2026-08-06): later live process
inspection found 13 OpenCode TUIs from the recorded D8.2 acceptance runs still
using roughly one CPU core each. Their private tmux servers, listeners, PTYs,
and temporary roots had been removed, but the provider processes had survived
terminal hangup, been reparented to the user service manager, and recreated
parts of their deleted roots. This falsified only the recorded process/root
cleanup claim and reopened the D8.2 exit gate.

The corrective implementation expands the design's existing
`Runtime.provider_pid` contract in host schema 12. Before releasing the launch
barrier it persists PID plus process birth and proves that the pane/provider is
the leader of its own process group. Park terminates that exact group before
removing tmux, using bounded TERM/KILL and a revalidated group/session boundary;
the observer remains exact-PID cleanup. Missing, reused, inaccessible,
malformed, or otherwise ambiguous process evidence is never signaled. A
schema-11 live Runtime may backfill only the PID from a freshly corroborated
private pane whose cwd and durable birth already match; a missing legacy
Runtime remains fail-closed.

The same closure journals OpenCode blank-session creation as a bounded `Start`
operation. Only a failure proven to precede `POST /session` may retry; any
crossed or unknown provider boundary leaves the exact Runtime generation
recovery-required and cannot create a second unmanaged session.

The new disposable lifecycle harness makes a provider and descendant ignore
terminal hangup/TERM and fails if either member of the owned group survives
Park, recovery, or Archive. The real production harness now treats park errors,
surviving recorded providers, process references to its disposable root, or a
root that reappears after removal as falsification. `scripts/check` and the
operator-gated local/real-loopback-SSH rerun must both pass before this status
returns to Complete.

One corrective gate remains open: the short-lived OpenCode server used for
blank-session creation receives bounded exact cleanup on every returned action
path, and inconclusive cleanup is terminally recovery-required rather than
retryable. Its PID, birth, and process-group cleanup authority is not yet
durable or supervised across abrupt loss of the owning WSNav action. D8.2
cannot return to Complete until a crash-surviving guardian or equivalent
durable exact-process authority is implemented and a kill-at-the-session-
creation-boundary acceptance proves that no server or descendant survives.

Post-completion contract correction (2026-08-06): the initial adapter promoted
the spike's observed `1.18.11` release into an exact production allowlist. That
confused acceptance evidence with compatibility and would disable OpenCode on
every routine upgrade. Discovery now accepts any installed executable with a
successful bounded version command, while actual compatibility is enforced by
the exact endpoint health, root-session, SSE, settled-boundary, Fork,
process-ownership, and cleanup contracts. The owned endpoint's version is
stored only as an opaque Runtime-generation fingerprint; it must remain exact
within that generation, while a recovered generation may record a newer value.
Disposable acceptance advertises deliberately non-accepted future/development
version strings and still passes the unchanged API contract. Malformed health
metadata and mid-generation mismatches remain fail-closed.

## Deferred beyond V1

The roadmap does not include arbitrary existing-session adoption, hard
Workstream/provider-session deletion, worktree or branch removal, checkout
synchronization, task/context transfer, transcript or memory features,
automatic plan rollover, provider/model/role launch presets, provider filters
or grouping, generalized provider onboarding, unproven OpenCode navigator
Rename, profile composition, Claude parity, multiple-controller catalog
synchronization, a public daemon, or a replacement provider UI.
