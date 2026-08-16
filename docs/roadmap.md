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
an operator-gated local and real-loopback-SSH production reacceptance passed on
2026-08-07. A later review confirmation reopened only the current-harness
production confirmation after an unclassified SSH Fork rejection and two
provider-driver timeouts. The hardened harness now requires a stable exact
Fork boundary, permits one proven pre-effect revision refresh without changing
that boundary or Runtime identity, and fails closed on churn or any durable
effect. Its final local and real-loopback-SSH confirmation passed with complete
cleanup, closing D8.2. There is no generic
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

This does not reproduce in the same way when a provider is launched manually
inside one ordinary tmux pane because that path has one tmux renderer. The
retained WSNav path is terminal -> private presentation tmux -> private Runtime
tmux -> provider, with another renderer in front when WSNav itself is launched
from ordinary tmux. The Runtime server preserves the exact provider process and
completed output; the presentation server owns the Navigator/provider split.
Its provider pane is a nested tmux attachment rather than a transparent byte
pipe, so each layer parses and re-renders the inner terminal stream. This is the
source of the measured amplification, not a provider hook or pane-management
write.

Cursor state must not be confused with that amplification. Both private tmux
servers retained `cursor-style default`; the effective Ghostty configuration
had no blink override. A metadata-only live check on 2026-08-13 reported
`cursor_blinking=1` for the private OpenCode Runtime and `cursor_blinking=0` for
the private Codex Runtime, the Codex-attached presentation pane, and two
ordinary Codex panes. No pane content was captured or input injected. The
[OpenCode TUI configuration](https://opencode.ai/docs/tui/) confirms that a
blinking block cursor is OpenCode's default and exposes `cursor.blinking` as the
native control. That control belongs to the operator's OpenCode configuration,
not WSNav. The V1 decision is to leave OpenCode's blinking default unchanged and
add no WSNav, tmux, Ghostty, or managed OpenCode cursor override. The irregular
flicker remains nested redraw traffic disturbing the cursor. The operator
reports Claude is steady too, but Claude is outside the V1 provider surface and
was not independently verified here.

The following WSNav-controllable candidates were each ruled out with the
instrument and left the `civis`/`cnorm` emission unchanged:

- `set -g cursor-style block` (steady, non-blinking) - only selects the cursor
  shape; the hide/show toggle during redraw is independent;
- `set -g extended-keys always` / `terminal-features` from commit `c0ce139`;
- `set -g update-scroll-region on`; and
- the `sync` (`CSI ?2026`) terminal feature, which is already active for
  Ghostty clients.

A live A/B also applied steady `cursor-style block` overrides at both server
and pane scope on the active private presentation and Runtime layers. OpenCode
continued blinking because its own TUI cursor policy remained enabled. Every
override was restored to `default`; no cursor workaround entered WSNav,
Ghostty, or ordinary tmux configuration.

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

Date: 2026-08-15

Status: D0 through D11.4 are complete. D12 implementation, automated
validation, and local operator acceptance are complete; its equivalent SSH
visual confirmation remains pending. V1 remains a source-installed operator
beta.

Roadmap organization note (2026-08-14): the completed checkpoints that
followed the original multi-provider outcome are grouped below by the product
or engineering outcome they actually delivered. This documentation-only
reclassification preserves their delivery order and completion evidence:

| Former checkpoints | Current checkpoints | Stage |
| --- | --- | --- |
| D8.0-D8.2 | unchanged | Multi-provider Workstreams |
| D8.3-D8.10 | D9.0-D9.7 | Architecture and runtime reliability |
| D8.11-D8.17 | D10.0-D10.6 | Navigator responsiveness and interaction |
| D8.18-D8.22 | D11.0-D11.4 | Project browser usability |
| D8.23 | D12 | Ephemeral Workstream shell |

Historical commit subjects and acceptance-artifact filenames retain the
identifiers in use when they were created. They are not rewritten, and this
mapping changes no implementation, protocol, schema, acceptance result, or
product boundary.

This roadmap turns the reconciled [V1 design](design.md) into reviewable
delivery checkpoints. The design remains the product and architecture contract.
This document owns sequencing, exit gates, and progress.

## Delivery rules

- Each checkpoint ends in a working, reviewable repository state.
- Commit by coherent capability; do not hold unrelated layers for one large
  checkpoint commit.
- Close a stage when its named product or engineering outcome passes. A later
  unrelated capability, hardening series, or UX series starts a new stage
  instead of extending the prior stage suffix indefinitely.
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
| D8 | Multi-provider Workstreams | Complete through D8.2 |
| D8.0 | Provider identity foundation and Codex parity | Complete (2026-08-05) |
| D8.1 | Provider-aware New and OpenCode New/Resume vertical slice | Complete (2026-08-06) |
| D8.2 | OpenCode Fork, recovery, and integrated acceptance | Complete (2026-08-07) |
| D9 | Architecture and runtime reliability | Complete through D9.7 |
| D9.0 | Behavior-neutral internal architecture consolidation | Complete (2026-08-08) |
| D9.1 | Fail-closed Linux process-group probe reliability | Complete (2026-08-09) |
| D9.2 | Behavior-neutral action and CLI orchestration decomposition | Complete (2026-08-09) |
| D9.3 | tmux 3.4 attachment compatibility and CI acceptance reliability | Complete (2026-08-09) |
| D9.4 | Private tmux terminal configuration drift guard | Complete (2026-08-09) |
| D9.5 | Inactive Codex binding and failed-presentation recovery | Complete (2026-08-10) |
| D9.6 | OpenCode observer lifetime across bounded local actions | Complete (2026-08-11) |
| D9.7 | OpenCode settled-state and activity-age reconciliation | Complete (2026-08-12) |
| D10 | Navigator responsiveness and interaction | Complete through D10.6 |
| D10.0 | Navigator steady-state latency and redraw containment | Complete (2026-08-13) |
| D10.1 | Workstream context-row scanability | Complete (2026-08-13) |
| D10.2 | Initial presentation-width convergence | Complete (2026-08-14) |
| D10.3 | Workstream card hierarchy cleanup | Complete (2026-08-14) |
| D10.4 | Expanded shortcut alignment | Complete (2026-08-14) |
| D10.5 | Finite-control authority and repository drift cleanup | Complete (2026-08-14) |
| D10.6 | Navigator-retained mouse Workstream switching | Complete (2026-08-14) |
| D11 | Project browser usability | Complete through D11.4 |
| D11.0 | Home-root Project browser default | Complete (2026-08-14) |
| D11.1 | Modal-local hidden Project-directory toggle | Complete (2026-08-14) |
| D11.2 | Human-facing Project-directory ordering | Complete (2026-08-14) |
| D11.3 | Repository-first Project-directory ordering | Complete (2026-08-14) |
| D11.4 | Directional Project-browser navigation | Complete (2026-08-14) |
| D12 | Presentation-scoped ephemeral Workstream shell | Operator acceptance pending |

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
   narrow Workstreams pane with initially two-line Recent rows (later split
   into three lines by D10.3), explicit two-line tree children in grouped views,
   the `Recent` / `By project` / `By host` / `Archived` cycle, compact bottom key
   hints, and a single-column expanded reference while retaining the accepted
   Workstreams bindings. Each later stateful action owns its bounded text entry,
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
   bounded directory browser. Each host defaults to `~` after the D11.0
   refinement and exposes an explicit Hosts-page root setting. The protocol
   returns only a safe root label, relative cursor, and direct-child names;
   host-side registration reconstructs the chosen directory locally. The
   direct `register` and `host register-checkout` commands remain optional
   scripting and break-glass paths. This slice is complete.

Exit gate:

- deterministic tests cover page navigation, modal input isolation,
  confirmation, duplicate-action suppression, status/action-line separation,
  compact/expanded key-help state, narrow-width row truncation, variable-height mouse
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

Implementation status: D8.0 completed on 2026-08-05, D8.1 completed on
2026-08-06, and D8.2 completed on 2026-08-07 after its corrective cleanup,
crash-guardian, deterministic lifecycle, and hardened real
local/loopback-SSH acceptance passed. The installed OpenCode release reported
`1.18.11`; compatibility remains contract-based rather than version-gated. The
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

Status: Complete on 2026-08-07 after corrective implementation, deterministic
cleanup/crash-guardian gates, and a final hardened operator-gated real
local/loopback-SSH confirmation. The installed OpenCode release reported
`1.18.11`; production compatibility remains contract-based.

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
root that reappears after removal as falsification. `scripts/check` passes, and
the operator-gated local/real-loopback-SSH harness produced complete passing
runs before and after its review-driven revision-stability hardening.

Crash-guardian corrective closure (2026-08-07): the short-lived OpenCode server
now starts behind a state-free pre-exec barrier. Its guardian proves the future
server leader's PID, birth, process group, and session before release; `exec`
preserves that authority, and the selected loopback listener must belong to the
exact process tree before readiness. The owning action revalidates the same
authority after health, immediately before `POST /session`, and around blank
verification. An anonymous owner lease survives ordinary action return and
closes automatically on action process loss, causing bounded
TERM/KILL, exact group/session revalidation, direct-child reap, and listener-
absence corroboration. A Start operation abandoned in `prepared` is now
recovery-required because synchronous cleanup was not observed; only a normal
terminal known-absent failure may retry.

The new disposable kill-boundary harness gates the fake provider inside its
single POST, kills only the exact Start action after a PID/birth recheck, and
proves the isolated guardian, TERM/HUP-ignoring server and descendant, listener,
and private tmux artifacts are gone. It also proves one POST with no committed
session, no retry, recovery attention, ordinary-tmux preservation, and survival
of an unrelated exact-identity sentinel. The complete `scripts/check` gate
passes 362 library tests plus 5 local transport tests, formatting, strict
Clippy, package/license/advisory policy, every disposable acceptance, and diff
checks. The automated corrective gate is closed. During review, a later
confirmation completed the local path but received an unclassified SSH Fork
rejection; two subsequent attempts timed out in the external provider driver
before Fork. Sanitized failure categorization and bounded final-cleanup retry
coverage were added. A subsequent exact SSH revision conflict showed that one
immediate refresh was insufficient while observer tail events were still
advancing the optimistic revision. The harness now waits for a stable exact
revision/settled-boundary tuple, permits at most one retry only after the exact
pre-effect rejection and proof that no operation or destination was created,
and refuses boundary/Runtime changes or sustained churn. The final real local
and loopback-SSH run passed every assertion with complete cleanup, closing the
production confirmation. Independent simultaneous loss of both action and
guardian would require a deferred external supervisor or cgroup authority and
is not claimed by this V1 in-process boundary.

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

## D9 - Architecture and runtime reliability

Implementation status: D9.0 through D9.7 are complete. This stage groups the
behavior-neutral architecture work, bounded process and tmux hardening, failed
presentation recovery, and provider lifecycle reconciliation delivered after
the D8 multi-provider outcome. The roadmap reclassification changes none of
their original scope, delivery order, or completion evidence.

## D9.0 - Internal architecture consolidation

Status: Complete on 2026-08-08.

This checkpoint reduces implementation concentration before further product
work. It is a behavior-neutral source-organization change, not a new provider,
runtime, lifecycle, presentation, or persistence design.

Scope:

- split the host/client persistence, schema, model, operation, Runtime, and
  observation responsibilities currently concentrated in `src/state/mod.rs`
  into cohesive modules while preserving the existing `wsnav::state` public
  paths and transactional boundaries;
- split navigator snapshot projection, asynchronous remote monitoring, view
  state, rendering, controller/action handling, and inline tests into cohesive
  modules while preserving the existing `wsnav::navigator` public paths;
- move the large inline state and navigator test modules beside their new
  implementation modules without renaming, weakening, or deleting coverage;
- correct present-tense documentation that still describes the pre-D8 provider
  boundary or D8.2 as incomplete; and
- keep commits independently reviewable, with the full repository gate before
  each checkpoint commit.

Non-goals and hard boundaries:

- no protocol, host/client schema, control ABI, dependency, CLI, shortcut,
  diagnostic, provider command, provider capability, or persisted-state change;
- no UI layout, styling, polling, redraw, attachment, tmux configuration,
  timeout, retry, process-signal, cleanup, or recovery-semantic change;
- no generic provider trait, plugin framework, process-supervision
  consolidation, or action/runtime redesign; and
- no work from the deferred product scope below.

Exit gate:

- public state and navigator paths remain source-compatible at their existing
  `wsnav::state` and `wsnav::navigator` locations, and every existing test
  retains its meaning and passes;
- protocol 17, host schema 12, client schema 5, control ABI 1, Cargo manifests,
  native provider command vectors, and both private tmux configurations remain
  unchanged;
- state migrations, transactional operation recovery, lifecycle observation,
  provider selection, snapshot projection, remote-unreachable caching,
  navigator input/rendering, attachment, and visible-working spinner regressions
  remain covered by the existing deterministic suites;
- no production implementation file remains an avoidable multi-responsibility
  monolith; any retained large file has one reviewable ownership boundary; and
- `scripts/check` and staged/unstaged `git diff --check` pass. Live provider or
  SSH acceptance remains operator-gated and is not implied by structural source
  movement.

Completion evidence (2026-08-08):

- `src/state/mod.rs` is a 24-line explicit facade over ten production modules
  plus the unchanged 74-test `state::tests` surface. Cross-module helpers are
  confined to `crate::state`; transactional compound operations and Runtime
  persistence retain cohesive ownership boundaries.
- `src/navigator.rs` is a 23-line explicit facade over model, snapshot/remote
  monitoring, view, rendering, and controller modules plus the unchanged
  72-test `navigator::tests` surface. Cross-module helpers are confined to
  `crate::navigator`.
- the combined Rust suite passes 367 tests. Formatting, Clippy with warnings
  denied, Cargo packaging and dependency policy, shell/Python/fixture checks,
  all disposable D4 through D8.2 acceptance harnesses, and staged/unstaged diff
  checks pass through one uninterrupted ordinary-host `scripts/check` run.
- protocol 17, host schema 12, client schema 5, control ABI 1, `Cargo.toml`,
  `Cargo.lock`, command help, provider command ownership, and private tmux
  configuration remain unchanged. No live provider or SSH acceptance was run.
- candidate validation exposed an intermittent fail-closed Linux process-group
  probe under heavy unrelated host process churn. The affected unchanged
  harnesses also passed independently and in the final uninterrupted gate.
  D9.0 does not weaken the probe or add retry behavior; any reliability change
  remains separate correctness work.

## D9.1 - Fail-closed Linux process-group probe reliability

Status: Complete on 2026-08-09.

This checkpoint corrects the intermittent ordinary-host process-group probe
failure exposed during D9.0 validation. It preserves the existing ownership
and signal-authority boundary: retry may turn a transient read into exact
evidence, but it may never turn ambiguous evidence into absence, ownership, or
permission to signal.

Scope:

- add a small fixed retry budget when a numeric process-table candidate's
  `/proc/<pid>/stat` record is transiently malformed during Linux group-member
  enumeration;
- accept a reread only when it returns a fully parsed exact record, or omit the
  candidate when that exact proc entry has conclusively disappeared;
- keep direct provider identity, birth-token, process-group, and session reads
  strict, and propagate persistent malformed, inaccessible, or I/O evidence;
- retain exact leader and group/session revalidation before every TERM or KILL
  authority boundary; and
- cover transient recovery and persistent ambiguity with deterministic tests,
  plus the existing disposable lifecycle acceptance harnesses.

Non-goals and hard boundaries:

- no best-effort parsing, silent skipping of persistent ambiguity, broader
  process discovery, PID-only ownership, or signal fallback;
- no retry around provider commands, tmux operations, state transactions,
  network I/O, lifecycle observation, or signal delivery;
- no protocol, schema, control ABI, persistence, CLI, UI, provider command,
  dependency, or private tmux configuration change; and
- no structural cleanup from the subsequent orchestration-decomposition work.

Exit gate:

- a transient malformed candidate followed by exact disappearance or a valid
  record completes enumeration without weakening member filtering;
- persistent malformed, inaccessible, and I/O evidence still fails closed,
  and no failing proof authorizes a group signal or parked transition;
- exact provider birth, group-leader, session, zombie, TERM, KILL, and
  post-signal revalidation regressions retain their existing meaning and pass;
- protocol 17, host schema 12, client schema 5, control ABI 1, Cargo manifests,
  public APIs, CLI help, native provider command vectors, and both private tmux
  configurations remain unchanged; and
- focused runtime tests, the applicable disposable lifecycle harnesses,
  `scripts/check`, and staged/unstaged `git diff --check` pass. Live provider or
  SSH acceptance remains operator-gated and is not required by this correction.

Completion evidence (2026-08-09):

- Linux group-member enumeration retries only a malformed numeric candidate's
  stat record, with three total reads and a one-millisecond wait between
  malformed attempts. A later exact record is filtered normally and a later
  missing proc entry is omitted; persistent malformed, inaccessible, and I/O
  evidence still propagates as a probe failure.
- direct provider birth, process-group, and session reads remain strict and
  single-read. Existing pidfd-backed birth/group/session revalidation remains
  unchanged at every signal boundary, and a new proof-level regression confirms
  ambiguous membership authorizes no group signal.
- five deterministic regressions cover malformed-to-valid,
  malformed-to-disappeared, persistent malformed, immediate inaccessible/I/O
  failure, and no-signal ambiguity. The 367-test library suite, five integration
  tests, focused 32-test runtime surface, disposable lifecycle-correctness, and
  OpenCode creation-guardian acceptance all pass.
- formatting, Clippy with warnings denied, Cargo packaging and dependency
  policy, shell/Python/fixture checks, every disposable D4 through D8.2
  acceptance harness, and staged/unstaged diff checks pass through
  `scripts/check`. No live provider or SSH acceptance was run.

## D9.2 - Behavior-neutral action and CLI orchestration decomposition

Status: Complete on 2026-08-09.

This checkpoint completes the bounded source-organization pass begun in D9.0.
It separates action orchestration and CLI dispatch responsibilities without
changing product behavior, command surfaces, or the runtime/process boundary
corrected in D9.1.

Scope:

- make `src/actions.rs` a small explicit facade over cohesive creation,
  start/recovery, attachment, lifecycle, launch-program, cleanup/identity, and
  error/model modules while preserving every existing `wsnav::actions` path;
- make `src/app.rs` a small explicit facade over CLI definitions, local and
  host dispatch, observer management, lifecycle command handling, output, and
  error modules while preserving `wsnav::app::run`;
- move the existing inline action and app tests beside the decomposed modules
  without renaming, weakening, or deleting their coverage; and
- keep cross-module helpers private to their owning `actions` or `app` module
  tree and retain the current concrete Codex/OpenCode orchestration.

Non-goals and hard boundaries:

- no action-flow, recovery, cleanup, attachment, signal, timeout, retry,
  transactional revision, or external-effect semantic change;
- no CLI command, option, alias, hidden-command, help, stdout/stderr, exit-code,
  diagnostic, navigator shortcut, or presentation change;
- no protocol, host/client schema, control ABI, persistence, dependency,
  provider capability, provider command vector, or private tmux configuration
  change;
- no new trait, generalized provider/action framework, deduplication rewrite,
  or error-taxonomy redesign; and
- no structural split of `runtime` or the OpenCode adapter. Their remaining
  size follows cohesive process-supervision/provider boundaries and requires a
  separate future contract if those boundaries prove too broad.

Exit gate:

- `src/actions.rs` and `src/app.rs` are small readable facades, responsibility
  ownership is apparent from module names, and helpers do not leak into the
  crate-wide public surface;
- every existing `wsnav::actions` path, `wsnav::app::run`, `ActionError` and
  application-error behavior, CLI parse/help surface, hidden command, exact
  provider command vector, and test meaning remains unchanged;
- protocol 17, host schema 12, client schema 5, control ABI 1, Cargo manifests,
  runtime process-probe logic, provider modules, presentation, and both private
  tmux configurations remain unchanged;
- focused action and app suites, exact pre/post CLI help hashes,
  `scripts/check`, and staged/unstaged `git diff --check` pass. Live provider or
  SSH acceptance remains operator-gated and is not implied by structural source
  movement.

Completion evidence (2026-08-09):

- `src/actions.rs` is a 58-line explicit facade over seven production modules
  plus the unchanged 17-test `actions::tests` surface. Creation, attachment,
  lifecycle, start/recovery, provider-program, cleanup/identity, and model/error
  ownership are separated; all prior public and crate-visible action paths are
  reexported at their original locations.
- `src/app.rs` is a 72-line facade over eight production modules plus the
  unchanged 17-test `app::tests` surface. Clap definitions, top-level dispatch,
  local lifecycle handling, SSH operations, observer management, launch
  helpers, and application errors retain explicit module ownership while
  `wsnav::app::run` remains the public entrypoint.
- the root, `host`, `start`, `fork-workstream`, and `recover` help output hashes
  match their pre-move values exactly. `ActionError`, `AppError`, hidden command
  parsing, provider-surface diagnostic suppression, and existing test names and
  meanings remain unchanged.
- the 367-test library suite and five integration tests pass. Formatting,
  Clippy with warnings denied, Cargo packaging and dependency policy,
  shell/Python/fixture checks, every disposable D4 through D8.2 acceptance
  harness, and staged/unstaged diff checks pass through one uninterrupted
  `scripts/check` run.
- protocol 17, host schema 12, client schema 5, control ABI 1, Cargo manifests,
  D9.1 process-probe behavior, provider modules, presentation, native provider
  command vectors, and both private tmux configurations remain unchanged. No
  live provider or SSH acceptance was run.

## D9.3 - tmux 3.4 attachment compatibility and CI acceptance reliability

Status: Complete on 2026-08-09.

This checkpoint restores the GitHub Actions signal after the D4 nested-attach
proof exposed a real private-config compatibility boundary. tmux 3.4 supports
extended keys and emits their fixed CSI-u representation, but it predates the
tmux 3.5 `extended-keys-format` option; the unknown option currently becomes a
client-visible config diagnostic and prevents the exact attached surface from
appearing on Ubuntu 24.04 runners.

Scope:

- make both private tmux configurations quietly ignore only the unavailable
  `extended-keys-format` option while retaining `extended-keys always`, RGB and
  extkeys terminal features, UTF-8 attachment, and explicit CSI-u selection on
  tmux releases that support it;
- retain the D4 requirement that the same deterministic provider marker is
  visible in both the owned Runtime pane and the nested attached client;
- replace silent D4 attachment timeouts with bounded diagnostics containing
  only tmux version, attachment/client presence, marker-presence booleans, and
  pane exit status; and
- validate the exact proof on Ubuntu 24.04 tmux 3.4 and the ordinary tmux 3.7
  development host before publishing.

Non-goals and hard boundaries:

- no removal of extended-key support, no fallback to process/socket liveness,
  and no weakening of the two-surface native UI assertion;
- no raw provider-pane or driver-pane capture in diagnostics, no ordinary tmux
  access, and no mutation outside disposable test state;
- no change to provider commands, Runtime identity, process ownership,
  attachment authority, presentation layout, protocol, schema, persistence,
  dependencies, or product-visible CLI behavior; and
- no general tmux-version abstraction or unrelated acceptance cleanup.

Exit gate:

- generated Runtime and presentation configs start cleanly on tmux 3.4, retain
  `extended-keys always`, and keep explicit CSI-u selection on supporting tmux
  releases;
- D4 proves both native surfaces and complete detach/cleanup on tmux 3.4 and
  3.7, with bounded metadata-only failure diagnostics;
- existing terminal-fidelity, private-runtime, presentation, provider, and
  acceptance regressions retain their meaning and pass;
- `scripts/check` and staged/unstaged `git diff --check` pass locally; and
- both the declared-MSRV and full-check GitHub jobs pass twice for the exact
  published commit. No live provider or SSH acceptance is required.

Completion evidence (2026-08-09):

- the failing GitHub runner and a disposable Ubuntu 24.04 reproduction both
  used tmux 3.4. The Runtime pane contained the exact fixture marker, while the
  nested client displayed `invalid option: extended-keys-format`; the same D4
  assertion failed on the preceding D8.2 and D9.0 publications.
- both private configs now use quiet assignment for the tmux-3.5-only format
  option. tmux 3.4 ignores that unavailable selector while retaining
  `extended-keys always` and its fixed CSI-u output; tmux 3.5 and later retain
  explicit CSI-u selection. RGB/extkeys features and every other configuration
  line remain unchanged.
- D4 still requires the deterministic native marker in both the owned Runtime
  pane and attached client. Surface and completion timeouts now report only a
  bounded tmux version, marker/client/completion booleans, and bounded pane
  exit metadata; no pane content, paths, identifiers, commands, environment,
  or provider data enter logs.
- focused 32-test Runtime and 15-test presentation suites pass. D4 passes its
  exact two-surface, detach, fork, park, and cleanup proof on tmux 3.4 and 3.7.
- formatting, Clippy with warnings denied, Cargo packaging and dependency
  policy, shell/Python/fixture checks, every disposable D4 through D8.2
  acceptance harness, and diff checks pass through `scripts/check`. The exact
  published commit passes both GitHub jobs twice. No live provider or SSH
  acceptance was run.

## D9.4 - Private tmux terminal configuration drift guard

Status: Complete on 2026-08-09.

This checkpoint removes one demonstrated source of nested-terminal drift after
D9.3 required the same compatibility correction in two separately owned tmux
configurations. It is a behavior-neutral configuration-ownership cleanup, not
a new tmux compatibility layer or runtime design.

Scope:

- give the terminal capability lines shared by the private Runtime and
  presentation tmux servers one crate-private source of truth;
- keep Runtime-only and presentation-only tmux behavior in their existing
  owning modules;
- preserve the generated Runtime and presentation configuration bytes exactly;
  and
- strengthen focused regressions from individual-line presence checks to the
  complete generated configuration contract.

Non-goals and hard boundaries:

- no tmux version detection, compatibility matrix, command retry, fallback, or
  new configuration option;
- no change to private socket/session ownership, attachment, pane layout,
  terminal features, mouse behavior, UTF-8 handling, or default tmux access;
- no protocol, schema, persistence, CLI, provider, process, lifecycle,
  diagnostic, dependency, or acceptance-harness change; and
- no structural split of Runtime, presentation, Navigator, state, or provider
  modules.

Exit gate:

- both private tmux configurations consume the same exact terminal capability
  fragment while retaining their module-specific prefix and suffix settings;
- focused tests lock the complete generated configuration bytes to the D9.3
  baseline;
- tmux 3.4 and 3.7 D4 attachment acceptance retain the exact two-surface proof;
  and
- `scripts/check` plus staged and unstaged `git diff --check` pass. Live
  provider or SSH acceptance is not required for this source-only cleanup.

Completion evidence (2026-08-09):

- `src/private_tmux.rs` owns the six terminal capability lines that must remain
  identical across nested private tmux layers. Runtime retains only its
  status/mouse prefix, while presentation retains its status/mouse/
  `remain-on-exit` prefix and mouse-binding suffix.
- focused Runtime and presentation regressions compare the complete generated
  configurations byte for byte with the D9.3 baseline. Both configs retain
  quiet tmux 3.4 handling, explicit CSI-u selection where supported, RGB,
  extkeys, and every topology-specific line.
- the 32-test Runtime and 15-test presentation suites pass. D4 retains its
  exact native marker in both the Runtime and attached-client surfaces, plus
  detach, fork, park, and cleanup, on Ubuntu 24.04 tmux 3.4 and the ordinary
  tmux 3.7 development host.
- the 367-test library suite and five integration tests pass. Formatting,
  Clippy with warnings denied, Cargo packaging and dependency policy,
  shell/Python/fixture checks, every disposable D4 through D8.2 acceptance
  harness, and diff checks pass through a final uninterrupted `scripts/check`
  run. No live provider session was launched for this source-only cleanup.
- two earlier full-gate attempts exposed separate existing load-sensitive
  timing failures in the OpenCode recovery and creation-guardian harnesses.
  Each harness passed immediately in isolation and the final complete gate
  passed unchanged; recurrence remains separate acceptance-reliability work,
  not permission to weaken either lifecycle proof.

## D9.5 - Inactive Codex binding and failed-presentation recovery

Status: Complete on 2026-08-10.

This checkpoint repairs a live operator-beta failure in which an exact Codex
binding corroborated by an earlier Runtime generation becomes retained history
after a replacement generation is explicitly parked before its `SessionStart`
hook arrives. That retained binding must remain usable for exact later resume,
but it must not authorize hooks or other mutations as though it belonged to
the inactive generation. A navigator process that fails during startup must
also not leave a provider-wait pane that makes the failed presentation appear
reconnectable forever.

Scope:

- represent a previously corroborated Codex binding as retained resume state
  for an inactive `stopped` or `unknown` Runtime without weakening the exact
  current-generation binding check;
- let bounded workstream snapshots and the proven-absent Codex start/recovery
  paths consume that retained binding while keeping stale hook evidence
  rejected;
- retire an owned presentation whose navigator pane is dead even when its
  blank provider-wait pane still keeps the private tmux server alive; and
- cover both behaviors with disposable regressions matching the observed
  persisted lifecycle sequence.

Non-goals and hard boundaries:

- no acceptance of stale-generation hook, attachment, rename, or live Runtime
  mutation authority;
- no provider-session adoption, database rewrite, schema/protocol change, or
  manual repair of operator state;
- no provider-pane diagnostics or capture, ordinary tmux access, presentation
  topology/layout change, or automatic provider launch; and
- no change to OpenCode recovery ownership or later deferred scope.

Exit gate:

- an exact prior Codex binding remains available to a parked or
  recovery-required Workstream after an uncorroborated replacement generation
  stops, while `binding_for_runtime` and lifecycle hooks still reject the
  generation mismatch;
- later Codex start/recovery selects that exact retained native session only
  after the owned Runtime is conclusively absent;
- discovery closes only an owned presentation with a dead navigator pane and
  creates a fresh presentation, without touching any provider Runtime or the
  ordinary tmux server;
- a disposable copy of the observed host-state shape produces a bounded
  navigator snapshot without editing the live database; and
- focused state/action/presentation tests, `scripts/check`, and staged and
  unstaged `git diff --check` pass.

Completion evidence (2026-08-10):

- one live `starship` record reproduced the failure without database
  corruption: an exact Codex binding retained its earlier Runtime generation
  while the replacement Runtime was `stopped` and its Workstream `parked`.
  The installed D9.3 navigator exited with `hook evidence does not match the
  managed runtime`, and its dead navigator pane remained discoverable because
  the blank provider-wait pane kept the private presentation server alive.
- the crate-private retained-binding reader accepts that previously
  corroborated Codex session only for an inactive `stopped` or `unknown`
  Runtime. The existing exact-current binding reader is unchanged, active and
  starting generation mismatches still fail closed, stale lifecycle evidence
  remains rejected, and OpenCode has no retained-binding path.
- Codex start and recovery read retained resume state only after the exact
  private Runtime probe is conclusively `Missing`. Snapshot hydration preserves
  the parked or recovery-required row and its exact resume session without
  granting attachment, rename, hook, or live-mutation authority.
- presentation discovery now reads only bounded `#{pane_dead}` metadata for the
  exact owned navigator pane. A dead navigator closes that disposable private
  server even when its provider-wait pane remains alive; no provider pane is
  captured and the ordinary tmux server is never addressed. A disposable
  integration regression constructs that exact failed topology on its own
  private tmux socket, proves the failed owner is retired, and receives a
  distinct fresh presentation.
- focused state, action, and presentation suites pass with 77, 17, and 17
  tests respectively. A disposable SQLite backup of the observed host state
  was rejected by D9.3 as unavailable, while the candidate returned a valid
  three-Workstream snapshot containing both parked rows. The live candidate
  retired the failed presentation and rendered the bounded workstream list;
  zero WSNav processes or private presentation sockets remained after exit,
  and the ordinary tmux fingerprint was unchanged.
- the 372-test library suite and six integration tests pass. Formatting,
  Clippy with warnings denied, Cargo packaging and dependency policy,
  shell/Python/fixture checks, every disposable D4 through D8.2 acceptance
  harness, and diff checks pass through one uninterrupted `scripts/check` run.
  No Workstream provider Runtime was launched.
- running the uninstalled candidate against live state rotated exact observer
  ownership as designed and discarded the prior native trust suffix. Cleanup
  restored the canonical `/home/bryan/.local/bin/wsnav` hook declaration and
  left the integration honestly `trust_pending`; completing native review is
  an operator-only deployment follow-up, not source acceptance evidence.

## D9.6 - OpenCode observer lifetime across bounded local actions

Status: Complete on 2026-08-11.

This checkpoint repairs a local Navigator failure in which an OpenCode observer
reached `ready` and was then terminated when the successful finite `start`
action cleaned up its owned process group. The provider Runtime remained healthy,
but exact attachment correctly failed closed after the observer PID disappeared.

Scope:

- isolate the deliberately long-lived OpenCode observer from the finite local
  control command's process group before spawning it;
- retain exact observer PID/birth ownership and the existing bounded Park
  cleanup path;
- prove the observer helper owns an independent process group; and
- validate the ordinary Navigator start-and-attach path against the installed
  release without capturing or writing provider-pane content.

Non-goals and hard boundaries:

- no weakening of `output_bounded` descendant cleanup for finite control
  commands;
- no observer adoption, restart-in-place, session rebinding, endpoint fallback,
  provider-input injection, or payload logging;
- no change to OpenCode lifecycle evidence, attachment preflight authority,
  private Runtime ownership, or ordinary tmux state; and
- no remote-host, protocol, schema, dependency, or provider-version change.

Exit gate:

- a successful local Navigator `start` cannot terminate the disconnected
  observer when its finite action process group is cleaned up;
- the observer remains exact and `ready` through attachment preflight, while
  Park still stops it by its persisted PID/birth evidence;
- focused observer-command and process tests plus the uninterrupted
  `scripts/check` gate pass; and
- sanitized live acceptance leaves the OpenCode provider pane attached and the
  observer live, with all superseded provider/observer processes absent.

Completion evidence (2026-08-11):

- the live localhost failure was reproduced with OpenCode 1.18.11: the observer
  reached `ready`, the finite Navigator action returned, the observer process
  disappeared, and attachment marked only that exact handle `unknown` while the
  provider endpoint remained healthy.
- source review traced the failure to the Navigator's bounded local `run_action`
  path using `output_bounded`, whose successful cleanup terminates the action's
  entire owned process group. The observer inherited that group and was
  therefore treated as a finite descendant even though its standard streams
  were disconnected.
- the observer spawn now enters its own process group. Existing Park cleanup
  remains exact-PID/birth based, so isolation does not broaden mutation
  authority or leave helper descendants behind.
- the observer-command regression proves the production command builder gives
  its helper an independent process group. One uninterrupted `scripts/check`
  run passes 373 library tests, six integration tests, formatting, Clippy with
  warnings denied, packaging, dependency policy, shell/Python/fixture checks,
  disposable D4 through D8.2 acceptance harnesses, and diff checks.
- the rebuilt release was installed locally and the existing presentation was
  refreshed without replacing its provider pane. A normal Navigator activation
  resumed the exact bound OpenCode session; delayed metadata-only verification
  found the observer still `ready`, the attachment `running`, the provider pane
  active, and every superseded provider/observer PID absent. No provider content
  was captured or written, and the ordinary tmux server was not addressed.

## D9.7 - OpenCode settled-state and activity-age reconciliation

Status: Complete on 2026-08-12.

This checkpoint repairs a live OpenCode lifecycle regression without changing
the exact settled-message boundary. The observer accepted and persisted the
latest completed assistant message, then a trailing incomplete
`message.updated` moved the Runtime from `attention` back to `working` even
though the exact bound root session was already idle. OpenCode observations
also advanced activity ordering without recording the wall-clock time required
by the Navigator's relative-age contract.

Scope:

- require exact root-session busy status before an SSE working hint may change
  Runtime lifecycle;
- keep the existing completed-assistant SSE candidate plus exact idle-status
  corroboration as the only OpenCode settled-message authority;
- record wall-clock activity on the first observed OpenCode working transition
  and on every accepted settled result; and
- cover trailing idle events, repeated working observations, settled attention,
  and activity timestamps with deterministic regressions.

Non-goals and hard boundaries:

- no message ID derived from polling, message lists, content, title, ordering,
  or timestamps;
- no raw provider payload persistence, diagnostics, or provider-pane capture;
- no session adoption, binding rewrite, observer restart-in-place, provider
  input, or manual live-database repair; and
- no Codex lifecycle, schema, protocol, UI layout, remote-host, dependency, or
  provider-version change.

Exit gate:

- an exact idle root session cannot regress from `attention` to `working` on a
  trailing incomplete `message.updated` event;
- an exact busy root session still becomes `working`, and status polling remains
  the bounded fallback if the event/status ordering races;
- OpenCode working and settled observations establish and refresh the persisted
  conversation activity time without changing identity or mutation authority;
- focused observer/state tests and one uninterrupted `scripts/check` run pass;
  and
- operator-gated local acceptance shows a new OpenCode turn settle to attention
  with a known age while the observer, binding, endpoint, and private Runtime
  remain exact and provider content is neither captured nor written.

Completion evidence (2026-08-12):

- source review and metadata-only live evidence reproduced the ordering race:
  the observer had already accepted the exact latest completed assistant
  message and moved the Runtime to `attention`, then a trailing incomplete
  `message.updated` moved it back to `working` while the exact root session was
  idle. OpenCode activity ordering also lacked the wall-clock timestamp needed
  for a relative age.
- working SSE evidence now requires exact root-session Busy corroboration
  before it can move a non-working Runtime to `working`. A completed assistant
  SSE candidate plus exact idle corroboration remains the only settled-message
  authority; polling still cannot supply or infer a message identifier.
- the first accepted OpenCode working transition and every accepted settled
  result now update conversation activity time. Focused observer and state
  regressions cover trailing events, repeated working evidence, settled
  attention, and timestamps without persisting provider payloads.
- one uninterrupted `scripts/check` run passes 374 library tests, six
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks.
- the exact release candidate was installed locally and the existing bound
  OpenCode session was resumed under a fresh private Runtime generation.
  Metadata-only acceptance found its observer exact and `ready`, endpoint and
  root-session evidence healthy, and the completed turn at `attention` with a
  known activity age. The Navigator pane alone showed the settled marker; no
  provider pane was captured or written and the ordinary tmux server was not
  addressed.

## D10 - Navigator responsiveness and interaction

Implementation status: D10.0 through D10.6 are complete. This stage groups the
performance, visual hierarchy, terminal-presentation, finite-control cleanup,
and mouse-selection work that refined the everyday Navigator experience. The
roadmap reclassification changes none of their original scope, delivery order,
or completion evidence.

## D10.0 - Navigator steady-state latency and redraw containment

Status: Complete on 2026-08-13.

This checkpoint responds to operator-visible flicker and delayed typing echo
in the retained presentation topology. Live inspection found three avoidable
steady-state costs: a 10 FPS working animation forces outer-presentation
redraws; every 500 ms local snapshot repeats executable/version probes; and
OpenCode supervision performs synchronous HTTP ownership/status work at SSE
event frequency during streaming. The existing tmux 3.7b nested-redraw
amplification remains upstream, but WSNav must stop feeding it avoidable churn.

Scope:

- cache fixed local executable and installation evidence once per Navigator
  process while recomputing dynamic observer/trust readiness from durable host
  state on every snapshot;
- preserve fresh provider-capability validation at every stateful host action
  boundary so presentation caching never becomes mutation authority;
- short-circuit repeated OpenCode working events before HTTP corroboration and
  rate-limit health/endpoint supervision independently of SSE event volume,
  while retaining the exact 500 ms root-status fallback and strict Runtime
  generation, PID, birth, directory, session, and endpoint checks;
- replace the animated working spinner with one static single-cell marker so
  steady working state does not itself schedule presentation redraws; and
- add deterministic regressions plus a disposable synthetic key-to-echo study
  that records aggregate timing only and leaves ordinary tmux unchanged.

Non-goals and hard boundaries:

- no weakening of exact OpenCode settled-message authority, status
  corroboration, failure limits, observer cleanup, or action-time capability
  validation;
- no provider-pane capture or input, prompt/output persistence, raw provider
  payload diagnostics, hook/plugin installation, or live database repair;
- no redesign of SSH polling, the two-private-tmux topology, navigator layout,
  provider native UI, terminal capability settings, or tmux itself; and
- no schema, protocol, dependency, remote-host, provider-version, session
  adoption, or binding change.

Exit gate:

- repeated steady-state snapshots reuse one fixed installation probe result,
  while a durable Codex observer/trust lifecycle change remains visible on the
  next snapshot and every provider action still performs fresh validation;
- an already-working OpenCode Runtime performs no per-event root-status HTTP
  request, periodic health/endpoint supervision is cadence-bounded, and an
  `attention` to `working` transition still requires exact Busy evidence;
- a visible working row causes no timer-driven Navigator redraw, while actual
  input, resize, snapshot, attachment, and transient-message changes still
  redraw normally;
- focused tests, the sanitized disposable key-to-echo study, and one
  uninterrupted `scripts/check` run pass; and
- operator-gated local acceptance shows responsive native typing and bounded
  working-state flicker without capturing or writing provider content or
  touching ordinary tmux.

Completion evidence (2026-08-13):

- the long-lived Navigator now caches only fixed executable/installation
  evidence. Dynamic Codex observer readiness is still read from durable host
  state on every snapshot, while every stateful action retains its existing
  fresh capability validation. A production-path disposable Navigator was
  sampled 150 times after startup and spawned zero `codex`, `opencode`, or
  `tmux` version-probe children during steady state; its exact private
  presentation was then removed and ordinary tmux remained unchanged.
- repeated OpenCode working evidence now reads durable Runtime state before
  constructing its lazy exact-status request. Deterministic coverage proves an
  already-working or missing Runtime performs zero status calls, while an
  attention Runtime calls once and accepts only exact Busy evidence. Health
  and endpoint ownership remain fail-closed but are cadence-gated independently
  of SSE event volume; the existing exact root-status poll remains the bounded
  fallback.
- the animated spinner, timer, frame state, and 100 ms redraw path are absent.
  One static yellow `●` retains visible working status and still supersedes a
  stale result marker, while the 500 ms snapshot and ordinary input, resize,
  attachment, and message redraw paths remain.
- [Spike 0018](evidence/spikes/0018-navigator-input-latency.md) returned all 90
  samples per case on tmux 3.7b. The static nested case measured 0.385 ms p95
  input delivery and 0.557 ms p95 echo, with complete cleanup and ordinary-tmux
  noninterference. The diagnostic 10 FPS case did not increase local synthetic
  latency, so the evidence does not misattribute the reported SSH/provider lag
  to animation alone.
- one uninterrupted `scripts/check` run passes 377 library tests, six
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks.
- release SHA-256
  `df25ec4660bb381063a1e4d548787513c34133073915556a936da7785e6d850f`
  is installed locally. The exact OpenCode Runtime was rotated through
  Park/Start: its native-session hash was unchanged, both superseded processes
  were absent, and the fresh provider/observer pair was live with observer
  status `ready`. No provider content was captured or written.
- with the retained presentation open on the installed candidate, the operator
  reported that typing lag was much better and the remaining native cursor
  blink was materially less flickery and acceptable. A scoped steady-cursor A/B
  confirmed that OpenCode explicitly requests a blinking cursor through both
  private tmux layers; the ineffective pane/server overrides were reverted and
  no terminal-setting workaround entered the repository or ordinary tmux.

## D10.1 - Workstream context-row scanability

Status: Complete on 2026-08-13.

At delivery, this checkpoint refined the first display line of each two-line
Workstream card. D10.3 later splits only Recent into three lines while retaining
the provider palette and grouped-view layouts established here. It makes
Project, provider, and host identity easier to scan without changing grouping,
selection, action routing, or the native provider surface.

Scope:

- split every Workstream context line into left- and right-justified sections;
- show Project on the left and provider plus host on the right in `Recent`;
- show provider on the left and host on the right in `By project`;
- show Project on the left and provider on the right in `By host`, removing the
  adjacent separator and Project-marker dots from the former composition;
- give Codex and OpenCode labels stable, distinct provider accents instead of
  the white/neutral Workstream-title foreground; and
- retain the full provider label at the supported narrow width while truncating
  variable Project and host labels first.

Non-goals and hard boundaries:

- no view-order, grouping, selection, mouse-target, lifecycle, state, schema,
  protocol, host-action, provider, tmux, or terminal-setting change; and
- no provider content capture, provider-pane input, durable color identity, or
  new user preference.

Exit gate:

- deterministic rendering tests cover the exact left/right identity order in
  all three active views, absence of the redundant grouped-view dots, full
  OpenCode visibility at the narrow width, and provider-palette separation;
- selected-row styling preserves provider, host, Project, and lifecycle
  foregrounds; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

Completion evidence (2026-08-13):

- one width-aware context-line renderer now places the view-specific identity
  sections at opposite edges. `Recent` renders Project versus provider/host,
  `By project` renders provider versus host, and `By host` renders Project
  versus provider without the former `provider · • Project` composition;
- Codex and OpenCode use distinct 256-color accents outside the host and Project
  palettes. Workstream titles remain white, selected rows retain their semantic
  foregrounds, and the colors carry no action or durable identity meaning;
- deterministic 30-cell tests cover the three layouts, long-label truncation,
  full OpenCode visibility, separator removal, palette separation, and selected
  styling; and
- one uninterrupted `scripts/check` run passes 378 library tests, six
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks.

## D10.2 - Initial presentation-width convergence

Status: Complete on 2026-08-14.

This checkpoint fixes a fresh presentation opening with an expanded Navigator
pane and reaching its intended 32-column width only after the outer terminal is
resized. The fault is presentation startup ordering: detached tmux first lays
out the panes at its default window size, then proportionally expands both when
the real client dimensions arrive.

Scope:

- establish the 32-column Navigator width at private tmux `client-attached` and
  `window-resized` event boundaries;
- retain the Rust TUI resize correction as a defensive path; and
- target only the exact private presentation session and Navigator pane.

Non-goals and hard boundaries:

- no provider Runtime resize command, provider-pane input or capture, ordinary
  tmux access, terminal-size polling, layout preference, schema, protocol,
  lifecycle, or provider behavior change.

Exit gate:

- a production-path disposable presentation expands from an 80-column detached
  window to 150 columns while its Navigator remains exactly 32 columns;
- deterministic command coverage proves both hooks target only the exact owned
  Navigator pane without a shell;
- formatting, tests, lint, package checks, disposable acceptance, and
  `git diff --check` pass; and
- a fresh installed presentation opens at 32 columns without requiring an
  operator-generated resize event.

Implementation evidence (2026-08-13):

- a disposable tmux reproduction started at 80 columns with a 32-column
  Navigator, then expanded the window to 150 columns; uncorrected tmux grew the
  Navigator to 67 columns. The production-path regression performs the same
  transition with the private hooks installed and retains exactly 32 columns;
- the hook-command regression proves `client-attached` and `window-resized`
  target only the generated private presentation session's Navigator pane and
  invoke no shell or provider command;
- one uninterrupted `scripts/check` run passes 379 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks; and
- release SHA-256
  `58ec66122c6800becd225c761bbac496133b051a54e08fbd998da6c94111c3e9`
  was installed locally. Operator confirmation of a fresh real presentation
  was the remaining gate at implementation time.

Operator confirmation (2026-08-14):

- after installing the current reviewed checkout, a fresh real presentation
  opened at the intended 32-column Navigator width without an
  operator-generated resize event.

## D10.3 - Workstream card hierarchy cleanup

Status: Complete on 2026-08-14.

This checkpoint reduces identity crowding in the flat Recent view by spending
one additional vertical row, and removes the redundant accent bullet before a
By project group name that already carries the same accent. It does not change
global recency, group ordering, Archived, or any Workstream action.

Scope:

- render each active Recent Workstream as exactly three rows: Project name,
  provider versus host, then lifecycle/thread versus activity age;
- preserve the provider, host, Project, lifecycle, and age color semantics from
  D10.1 while removing their competition for one context line;
- keep `By project`, `By host`, and Archived cards at two rows;
- use the colored Project name as the sole By project group-header identity cue;
  and
- map every visible row of either card height to the same exact Workstream for
  selection and mouse activation.

Non-goals and hard boundaries:

- no recency order, grouping, card selection, action routing, provider pane,
  host/protocol/state, terminal, tmux, schema, or durable preference change.

Exit gate:

- deterministic rendering proves Project, environment, and thread/age occupy
  three separate Recent rows at the default 30-cell inner width;
- mouse mapping covers all three rows, scrolled selection remains exact, and
  Archived is explicitly distinct at two rows;
- grouped-view layout, long-label truncation, and provider visibility tests
  remain green, and the By project rendering contains no decorative bullet;
- formatting, tests, lint, package checks, disposable acceptance, and
  `git diff --check` pass; and
- the installed Recent view is visually confirmed less crowded without losing
  actionable density in grouped or Archived views.

Implementation evidence (2026-08-13):

- Recent now has a distinct three-row entry height and renderer, while Archived
  has its own explicit two-row context instead of sharing Recent's internal
  variant. Grouped views remain unchanged at two rows;
- deterministic rendering places Project, OpenCode/host, and thread/age on
  three separate 30-cell inner rows. Mouse tests map all three rows to the exact
  same Workstream; the By project header keeps its colored name without the
  former bullet; and the existing scrolled-selection and grouped-tree suites
  pass;
- one uninterrupted `scripts/check` run passes 380 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks; and
- release SHA-256
  `cd47a23d063e0bf3f191ade8728fef7885ee58486894c48e07f4d4c8565fa0d2`
  was installed locally. The exact live private Navigator pane was refreshed at
  32 columns while the provider attachment pane PID remained unchanged;
  operator visual confirmation was the remaining gate at implementation time.

Operator confirmation (2026-08-14):

- the installed Recent view was confirmed less crowded with its three-row
  hierarchy, without losing useful density in grouped or Archived views.

## D10.4 - Expanded shortcut alignment

Status: Complete on 2026-08-14.

This checkpoint corrects uneven description columns in the Navigator-local `?`
reference. The long `↑/↓ or j/k` label overflowed the hand-padded column, while
the `←/→` row was also padded as if its key label were one cell wide.

Scope:

- render every expanded-help binding through one display-width-aware alignment
  function with descriptions beginning at column 11;
- advertise only the canonical `↑/↓` selection keys, retaining `j/k` as
  unadvertised compatibility aliases;
- keep `←/→` and every page/detail action aligned to the same column; and
- use concise descriptions no wider than the 19 cells remaining at the normal
  32-column Navigator width.

Non-goals and hard boundaries:

- no key-handler, compact-footer, help scrolling, page/action, provider pane,
  state, protocol, terminal, tmux, schema, or durable preference change.

Exit gate:

- deterministic help rendering proves `↑/↓` and `←/→` are present, `j/k` is
  absent, and every advertised binding description begins at column 11;
- every Workstreams, Archived, Projects, Hosts, ordinary-detail, and
  recovery-detail help row fits the 30-cell inner pane without clipping;
- the existing help modal-isolation and page-specific content tests pass;
- formatting, tests, lint, package checks, disposable acceptance, and
  `git diff --check` pass; and
- the installed `?` reference is visually confirmed aligned at the 32-column
  Navigator width.

Implementation evidence (2026-08-13):

- every expanded-help key/description pair now uses one terminal-cell-aware
  column calculation. Deterministic coverage checks the resulting column,
  confirms both arrow labels remain visible while `j/k` is omitted, and proves
  that the Workstreams, Archived, Projects, Hosts, ordinary-detail, and
  recovery-detail descriptions fit the normal 30-cell inner pane;
- one uninterrupted `scripts/check` run passes 381 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks; and
- release SHA-256
  `84794788c9b2cedb38456bbe4ed8d68e29e401e13c9ff9e903b860f45ffa2660`
  was installed locally. The exact live private Navigator pane was refreshed at
  32 columns while the provider attachment pane PID remained unchanged;
  operator visual confirmation was the remaining gate at implementation time.

Operator confirmation (2026-08-14):

- the installed Navigator-local `?` reference was confirmed aligned and
  unclipped at the normal 32-column Navigator width.

## D10.5 - Finite-control authority and repository drift cleanup

Status: Complete on 2026-08-14.

This checkpoint removes repeated low-level ownership and cleanup mechanics from
three finite child-command paths while preserving their existing public
behavior. It also reconciles present-tense repository status, generated product
captures, and dependency-policy drift found by the project-wide cleanup audit.

Scope:

- give bounded local commands, bounded SSH host commands, and the ephemeral
  Codex App Server one crate-private authority for child PID-to-PGID/session
  capture, Linux process-table membership proof, guarded process-group
  signaling, and direct-child cleanup/reap mechanics;
- retain each caller's existing timeout, stream, output-bound, public-error,
  and cleanup-error precedence contract through explicit local adapters and
  deterministic tests;
- make the roadmap the sole present-tense checkpoint-status authority while,
  at D10.5 completion, leaving D10.2 through D10.4 pending their stated
  operator confirmation;
- refresh the privacy-safe fixture captures from the current real Navigator
  renderer; and
- remove license allowlist entries unused by the locked dependency graph.

Non-goals and hard boundaries:

- no persistent Runtime, observer, provider-process, attachment, or interactive
  SSH signal-authority change;
- no transient process-table retry, timeout, stream, provider command,
  lifecycle, recovery, error wording, or error-precedence change;
- no protocol, host/client schema, control ABI, Cargo dependency, private tmux
  configuration, provider-pane input, or provider-content capture; and
- no completion claim for operator-visible checkpoints that remain under daily
  use evaluation.

Exit gate:

- raw finite-child PGID/session capture, process-table membership parsing,
  guarded group signaling, and direct-child cleanup/reap mechanics exist only
  in `src/process.rs` and all three consumers use that authority;
- deterministic tests lock Linux stat parsing, group-plus-session matching,
  invalid-PID mapping, and each caller's exact cleanup-error precedence;
- protocol 17, host schema 12, client schema 5, control ABI 1, Cargo manifests,
  private tmux configurations, and native provider command vectors remain
  unchanged;
- regenerated product captures contain only deterministic fixture data and the
  dependency policy passes without obsolete license-allow warnings; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass. Live provider or SSH acceptance is neither required
  nor performed for this behavior-neutral cleanup.

Completion evidence (2026-08-14):

- `src/process.rs` now owns the finite-child PID-to-PGID/session capture,
  Linux process-table parser and membership proof, guarded process-group
  signal, direct-child kill, and reap mechanics consumed by bounded local
  commands, `SystemCommandRunner`, and `EphemeralAppServer`. The consumers keep
  only their caller-specific adapters;
- deterministic tests cover last-parenthesis stat parsing, malformed identity
  rejection, exact group-plus-session matching, invalid-PID mapping, and the
  established cleanup-error precedence for all three callers. Existing
  deadline, descendant cleanup, output-bound, and App Server process-group
  regressions retain their meaning and pass;
- the README and documentation map now defer changing checkpoint status to this
  roadmap, while D10.2 through D10.4 were still pending operator confirmation.
  The three SVG/PNG frames and GIF tour were regenerated from the deterministic
  fixture renderer and visually inspected without accessing a provider pane;
- Cargo Deny passes after removing only the unused BSD-2-Clause and
  BSD-3-Clause allow entries. Its existing duplicate-version warnings remain;
- protocol 17, host schema 12, client schema 5, control ABI 1, `Cargo.toml`,
  `Cargo.lock`, native provider commands, and both private tmux configurations
  are unchanged; and
- one uninterrupted `scripts/check` run passes 387 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and diff checks. No live provider or SSH acceptance,
  installation, or Runtime rotation was performed.

## D10.6 - Navigator-retained mouse Workstream switching

Status: Complete on 2026-08-14.

This checkpoint makes Workstream-card clicks behave like navigation-first
selection. A card click still displays the selected Workstream through the
existing exact open/start/recover and attachment path, but keyboard control
remains in the Navigator so the user can continue browsing. Entering the
native provider remains an explicit `Enter`, `Tab`, or provider-pane click.

Scope:

- give Workstream activation an explicit post-activation focus policy;
- retain provider focus for keyboard `Enter` while retaining Navigator focus
  after a primary Workstream-card click;
- preserve exact selection and activation for every rendered card line,
  including scrolled Recent, grouped, and Archived layouts; and
- retain the presentation tmux mouse binding so clicking the right pane still
  transfers control directly to the native provider.

Non-goals and hard boundaries:

- no lifecycle, start, recover, attachment, retry, archived, unreachable-host,
  selection, scrolling, grouping, provider-pane input, tmux topology, state,
  schema, protocol, or provider behavior change;
- no hover preview, double-click action, context menu, drag behavior, focus
  persistence, or durable current-Workstream record; and
- no provider content capture or ordinary tmux access.

Exit gate:

- deterministic controller coverage proves a Workstream-card click selects
  and activates the exact Workstream without issuing provider-focus control;
- deterministic coverage proves keyboard `Enter` retains its existing
  provider-focus behavior, including an already attached Workstream;
- the existing multi-line, grouped, scrolled, blank, management, failure, and
  attachment-retry mouse behavior remains green;
- the design contract records the navigation-first mouse focus boundary; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-14):

- Workstream activation now carries one typed input source. Keyboard `Enter`
  and internal creation/recovery handoffs retain provider focus, while a
  primary card click applies Navigator focus after the same exact lifecycle
  and attachment path;
- the already-attached fast path uses the same typed routing instead of
  unconditionally focusing the provider. A narrow focus-only test seam leaves
  attachment and presentation ownership concrete while deterministic tests
  prove `MouseClick -> Navigator` and `Enter -> Provider`;
- existing tests continue to cover exact multi-line and scrolled row mapping,
  blank and management clicks, attachment failure and same-row retry, and the
  private tmux binding that transfers focus when the provider pane itself is
  clicked;
- protocol 17, host schema 12, client schema 5, control ABI 1, provider
  commands, Runtime behavior, and presentation tmux configuration are
  unchanged; and
- one uninterrupted `scripts/check` run passes 389 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and staged and unstaged diff checks. No provider
  content was captured and no live provider, SSH, or Runtime rotation was
  required.

Operator confirmation (2026-08-14):

- release SHA-256
  `b60df4942096a21706d6b73e53efe3f6d3b5e21a474b1475affe23a75809d93b`
  was installed locally without a state reset or Runtime rotation. One existing
  live Runtime remained untouched, and the operator confirmed that clicking a
  Workstream card switches the provider pane while keyboard control remains in
  the Navigator.

## D11 - Project browser usability

Implementation status: D11.0 through D11.4 are complete. This stage groups the
home-root default, hidden-directory control, human-facing and repository-first
ordering, and directional navigation refinements into one Project-discovery
outcome. The roadmap reclassification changes none of their original scope,
delivery order, or completion evidence.

## D11.0 - Home-root Project browser default

Status: Complete on 2026-08-14.

This checkpoint makes a host's home directory the default Project-browser
boundary instead of assuming repositories live under `~/code`. The browser
remains bounded by one host-private configured root and cannot navigate above
it; operators who prefer a narrower workspace can still set one explicitly
from Hosts.

Scope:

- resolve an absent host Project-browser setting to the selected host's `HOME`;
- initialize the Hosts-page root-setting form with `~` and describe that default
  consistently in the product documentation; and
- preserve every explicitly persisted local or remote root without migration.

Non-goals and hard boundaries:

- no navigation above the configured root, arbitrary typed path registration
  in the ordinary picker, dot-directory visibility, root-path snapshot or
  protocol exposure, schema migration, host registration, or Git behavior
  change; and
- no provider, Runtime, attachment, presentation, or ordinary tmux change.

Exit gate:

- deterministic state coverage proves a fresh host resolves its browser root
  to canonical `HOME` while existing explicit-root coverage remains green;
- deterministic Navigator coverage proves the root-setting form starts at `~`;
- README, design, and roadmap descriptions agree on the new default; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-14):

- an absent `project_browser_settings` row now resolves directly to the
  selected host's `HOME`, which the existing host registry canonicalizes and
  bounds before listing or registration. Explicitly persisted roots retain
  their existing resolution and storage behavior;
- the Hosts-page root-setting form now begins with `~`, and its inline example,
  README guidance, design contract, and D7.6 summary use the same default;
- deterministic state coverage proves a fresh registry resolves the root to
  canonical `HOME`, while Navigator coverage proves the root form begins with
  `~`. Existing custom-root, safe-label, relative-cursor, escape-rejection,
  local/SSH protocol, and Project-registration coverage remains green;
- protocol 17, host schema 12, client schema 5, control ABI 1, persisted state,
  provider commands, Runtime behavior, and private tmux configuration are
  unchanged; and
- one uninterrupted `scripts/check` run passes 391 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and staged and unstaged diff checks. No live provider,
  SSH, state reset, or Runtime rotation was required for checkpoint validation.

Operator confirmation (2026-08-14):

- release SHA-256
  `8d515e4477e6e96da8b3f42d31a23b893d35f03f197207ab6525fb9c45667e67`
  was installed locally on `snap`, and the existing bounded host-control action
  persisted the home directory as that host's Project-browser root;
- a subsequent bounded Project-directory response reported the safe root label
  `~`; and
- no state reset or Runtime rotation was performed, and both existing open
  Workstreams remained intact.

## D11.1 - Modal-local hidden Project-directory toggle

Status: Complete on 2026-08-14.

This checkpoint makes a Project under a dot-directory reachable without
changing the host's configured browser root or adding arbitrary path entry.
Each newly opened Project browser continues to omit dot-directories; `.`
explicitly shows or hides them for that modal, and the selection persists while
navigating down or up inside the same bounded browser.

Scope:

- add one explicit modal-local hidden-directory visibility state, default it to
  off for each new Project browser, and expose its current action in the narrow
  browser footer;
- carry that state through the then-current child `Enter` and parent `h` key
  paths, preserving the current filter and selected directory where possible
  when the visibility toggles; D11.4 later rebinds those navigation paths to
  `Right` and `Left`; and
- extend the bounded local/SSH Project-directory request and response with one
  exact visibility flag, bumping protocol 17 to 18 so hidden directory names
  cross the host-control boundary only after an explicit request.

Non-goals and hard boundaries:

- no typed or absolute path entry, hidden-file listing, persisted preference,
  navigation above the configured root, root-path response, arbitrary host
  filesystem discovery, schema migration, or compatibility behavior; and
- no Project registration, Git inspection, provider, Runtime, attachment,
  presentation, ordinary tmux, or dependency change.

Exit gate:

- deterministic host-state coverage proves hidden-off excludes dot-directories
  while hidden-on includes safe dot-directories and still rejects files,
  unsafe names, and canonical escapes;
- protocol and transport coverage proves protocol 18 carries the explicit flag
  across local and SSH requests, refuses protocol 17, and rejects a hidden name
  in a response whose flag is false;
- deterministic Navigator coverage drives `.`, the then-current non-Git
  `Enter`, and `h` key paths, proving default-off toggle behavior,
  filter/selection preservation, navigation persistence, failure preservation,
  and narrow discoverability; D11.4 supersedes the two navigation bindings; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-14):

- the host-private directory listing accepts one explicit `include_hidden`
  request, echoes that visibility in its bounded response, and filters safe
  direct-child directories before canonical root-containment and directory
  checks. Dot-directories remain absent unless requested; files, `.`, `..`,
  unsafe names, and canonical escapes remain unavailable;
- the Project browser begins hidden-off, consumes `.` as a visibility toggle,
  preserves its prior modal on refresh failure, and threads the selected state
  through actual child and parent navigation. Its footer renders the current
  `Hidden: on/off` state and corresponding `. show/hide` action;
- protocol 18 fails closed on the former version and on a hidden entry returned
  without the visibility flag. Host schema 12, client schema 5, control ABI 1,
  persisted roots and state, provider commands, Runtime behavior, and both
  private tmux configurations are unchanged; and
- one uninterrupted `scripts/check` run passes 397 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and staged and unstaged diff checks. No live provider,
  SSH, installation, state reset, or Runtime rotation was required.

## D11.2 - Human-facing Project-directory ordering

Status: Complete on 2026-08-14.

This checkpoint replaces the Project browser's raw case-sensitive name order
with one deterministic human-facing order. Ordinary mixed-case directories no
longer split into uppercase and lowercase blocks, while explicitly shown
dot-directories remain a predictable leading group.

Scope:

- group dot-directories before visible directories only when the explicit
  D11.1 visibility request includes them;
- sort each group by a locale-independent Unicode lowercase key and then by the
  original exact name so equal folded keys remain deterministic; and
- apply the same visible-directory ordering whether hidden directories are on
  or off, without adding a dependency.

Non-goals and hard boundaries:

- no natural-number, locale-sensitive, modification-time, Git-status, or
  user-configurable ordering;
- no change to the existing bounded filesystem scan, response truncation, or
  completeness claim for directories beyond that scan; and
- no hidden visibility, path filtering, canonical containment, registration,
  protocol, schema, persistence, provider, Runtime, presentation, ordinary
  tmux, or dependency change.

Exit gate:

- deterministic host-state coverage proves mixed-case visible names follow
  their lowercase keys instead of raw uppercase-first order;
- equal lowercase keys use the exact original name as their stable tie-breaker;
- hidden-on responses contain one leading, internally sorted dot-directory
  group followed by the sorted visible group, while hidden-off responses retain
  only the same correctly sorted visible group; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-14):

- the bounded Project-directory response now computes one cached tuple per
  accepted entry: hidden-group rank, Unicode lowercase name, and exact original
  name. Sorting that tuple avoids repeated fold allocation while retaining a
  deterministic exact-name tie;
- deterministic state coverage uses mixed-case visible names, same-fold names,
  and mixed hidden/visible names to prove the complete ordering contract;
- protocol 18, host schema 12, client schema 5, control ABI 1, the D11.1
  visibility request, scan and response bounds, persisted state, provider
  commands, Runtime behavior, and both private tmux configurations are
  unchanged; and
- one uninterrupted `scripts/check` run passes 398 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and staged and unstaged diff checks. No live provider,
  SSH, installation, state reset, or Runtime rotation was required.

## D11.3 - Repository-first Project-directory ordering

Status: Complete on 2026-08-14.

This checkpoint makes the Project browser's actionable result the primary
ordering signal. Direct Git repositories now appear before directories that
only navigate deeper, while D11.2's hidden grouping and deterministic
case-insensitive name order remain subordinate within both tiers.

Scope:

- rank entries already identified as direct Git repositories before ordinary
  navigation folders, regardless of their names;
- retain hidden-before-visible grouping independently inside the repository and
  folder tiers when D11.1 visibility is enabled; and
- retain the Unicode lowercase key and exact original-name tie-breaker inside
  each resulting group without adding another filesystem probe.

Non-goals and hard boundaries:

- no Git detection, repository inspection, registration, filtering, marker,
  hidden-toggle, or selected-row behavior change;
- no header, separator, natural-number, locale-sensitive, modification-time,
  Git-status, or user-configurable ordering; and
- no scan/response bound, path/root, protocol, schema, persistence, provider,
  Runtime, presentation, ordinary tmux, or dependency change.

Exit gate:

- deterministic host-state coverage proves a repository whose name would sort
  late still precedes an alphabetically earlier navigation folder;
- hidden-off responses exclude hidden entries and order visible repositories
  before the D11.2-sorted visible-folder tier;
- hidden-on responses order hidden repositories, visible repositories, hidden
  folders, and visible folders, retaining mixed-case and same-fold exact-name
  behavior inside those groups; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-14):

- the existing cached Project-directory sort key now begins with the already
  computed `is_git_repository` marker, followed by hidden-group rank, Unicode
  lowercase name, and exact original name. No new Git or filesystem probe was
  introduced;
- deterministic state coverage combines hidden and visible repositories,
  hidden and visible navigation folders, mixed case, same-fold names, and a
  late-sorting repository to prove the complete ordering hierarchy;
- protocol 18, host schema 12, client schema 5, control ABI 1, Git detection,
  the D11.1 visibility request, D11.2 name keys, scan and response bounds,
  persisted state, provider commands, Runtime behavior, and both private tmux
  configurations are unchanged; and
- one uninterrupted `scripts/check` run passes 398 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and staged and unstaged diff checks. No live provider,
  SSH, installation, state reset, or Runtime rotation was required.

## D11.4 - Directional Project-browser navigation

Status: Complete on 2026-08-14.

This checkpoint separates browsing from registration in the Project picker.
Directional arrows now express directory navigation, while `Enter` has one
action meaning: add the selected Git repository.

Scope:

- make `Right` enter any selected directory, including a directory already
  marked as a Git repository, while preserving the modal-local hidden setting;
- make `Left` move to the parent without crossing the configured browser root;
- make `Enter` register only the selected marked Git repository, retaining the
  picker with bounded `Right` guidance when a plain folder is selected;
- retain `.` as the hidden-directory toggle, `Esc` as picker dismissal, and
  `Up`/`Down` as selection controls;
- remove the picker-local `j`/`k` selection aliases and `r` current-directory
  registration action so every letter, including `h`, is ordinary filter input;
  and
- render only the canonical arrow, add, and quit controls in the bounded
  footer.

Non-goals and hard boundaries:

- no filesystem, Git detection, directory ordering, filtering model,
  configured-root, registration authority, or provider-selection change;
- no navigation above the configured browser root and no typed or absolute
  path input; a repository that is itself the configured root must instead be
  selected from a configured parent; and
- no protocol, schema, persistence, Runtime, presentation, ordinary tmux, or
  dependency change.

Exit gate:

- deterministic Navigator coverage drives the actual `Right` and `Left` key
  paths, proving that a marked repository can be entered and hidden visibility
  persists in both directions;
- deterministic coverage proves root-bounded `Left`, plain-folder `Enter`
  guidance, marked-repository `Enter` registration intent, unrestricted letter
  filtering, and `Esc` dismissal without live host or provider effects;
- narrow rendering exposes the canonical hidden, navigation, add, and quit
  controls without relying on the advanced `r` action; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-14):

- the Project browser now routes `Right` through the existing bounded child
  request regardless of Git marker, routes `Left` through the bounded parent
  request, and refuses to request a parent when its relative cursor is already
  empty. Both directions retain the explicit hidden-directory visibility state;
- `Enter` now sends only a selected marked repository into the existing
  registration/provider-selection path. A plain directory remains selected
  with bounded `Right` guidance, while `h`, `j`, `k`, and `r` reach ordinary
  filter input and `Esc` dismisses the modal;
- the footer gives the hidden toggle, arrow navigation, `Enter` add, and `Esc`
  quit controls their own narrow-safe lines, without a secondary letter-key
  command path. Empty listings reserve enough height to keep every canonical
  control visible;
- protocol 18, host schema 12, client schema 5, control ABI 1, host scanning,
  repository detection, ordering, registration authority, persisted state,
  provider commands, Runtime behavior, and both private tmux configurations are
  unchanged; and
- one uninterrupted `scripts/check` run passes 401 library tests, seven
  integration tests, formatting, Clippy with warnings denied, packaging,
  dependency policy, shell/Python/fixture checks, disposable D4 through D8.2
  acceptance harnesses, and staged and unstaged diff checks. No live provider,
  SSH, installation, state reset, or Runtime rotation was required.

## D12 - Presentation-scoped ephemeral Workstream shell

Status: Implementation, automated validation, explicitly authorized local plus
real-SSH machine acceptance, and local normal-environment visual confirmation
complete on 2026-08-15; the equivalent SSH visual confirmation is pending.

This checkpoint adds quick, unmanaged terminal access beside the currently
attached provider without turning WSNav into a general-purpose terminal
multiplexer. The normal presentation still begins with the Navigator and one
native provider pane. One private tmux chord may temporarily add one ordinary
shell below the provider for short host-local work such as inspecting or
manually operating Git. The shell is never a Workstream or durable session.

Scope:

- replace the private presentation's inherited tmux prefix and root tables with
  explicit allowlists. Bind only `Ctrl+b "` for create-or-focus utility shell,
  `Ctrl+b %` for bounded guidance, confirmed `Ctrl+b x` for shell-only close,
  `Ctrl+b d` for detach, `Ctrl+b o` and directional keys for owned-pane focus,
  `Ctrl+b Ctrl+b` for literal `Ctrl+b` delivery to the focused application
  without exposing the nested Runtime prefix table, and `Ctrl+b ?` for curated
  help;
- retain in the root table only the primary mouse selection/forwarding and
  bounded scrolling/copy interactions required by the existing Navigator and
  native provider surfaces. Remove default right-click management menus,
  mouse split/swap/kill/respawn controls, and other topology-changing root
  bindings so a future tmux default cannot silently widen the surface;
- bind `Ctrl+b "` to create exactly one shell below the provider and transfer
  focus into it. Repeating the chord, including while the shell has focus,
  focuses the existing pane and never creates or rearranges another pane;
- suppress `Ctrl+b %` with bounded guidance to use `Ctrl+b "`. Expose no raw
  alternate split, arbitrary command prompt, new window or session,
  break/join/swap/rotate/layout command, or Navigator/provider kill or respawn
  binding;
- authorize shell creation only from one exact live provider surface whose
  presentation-private attachment phase is `Running`, whose provider pane is
  alive, and whose host alias and Workstream ID remain unambiguous. Pending,
  completed, failed, blank, observer-review, dead, stale, and malformed
  surfaces fail closed without changing layout;
- resolve the Workstream to its canonical registered ProjectLocation root on
  the authoritative host. Start the host account's ordinary interactive shell
  at that root without introducing a WSNav shell configuration or policy;
- for SSH Workstreams, preflight the fixed registered endpoint and invoke a
  fixed `ssh -tt` remote wsnav command carrying only the opaque Workstream ID.
  Resolve the absolute project root on the remote host and never place it in an
  SSH argument or protocol response;
- tag only bounded pane-role and Workstream context in disposable private tmux
  state, detect zero/one/multiple utility panes deterministically, and refuse
  ambiguous or unexpected topology rather than deleting panes;
- set shell-pane `remain-on-exit` off while retaining the Navigator/provider
  dead-pane behavior. Normal shell exit, `Ctrl+d`, remote disconnect, or the
  guarded shell-only close removes the pane and restores the two-pane layout
  without restarting WSNav; and
- keep a live shell fixed to its launch host and ProjectLocation until it exits.
  Provider switching neither retargets nor kills it. Client detach may leave it
  alive only as part of the same disposable presentation; there is no durable
  restoration after presentation loss.

Planned implementation slices:

1. land this design and roadmap contract without production behavior;
2. add presentation pane-role authority, the curated prefix table, idempotent
   below-provider split/focus/close behavior, host-local root resolution, and
   disposable local tmux coverage;
3. add the fixed remote interactive shell helper, SSH argument-vector and
   remote-root tests, and bump control ABI 1 to 2 so an older remote executable
   cannot be mistaken for shell-compatible; and
4. complete repository validation plus explicit operator-gated local and real
   SSH terminal acceptance with sanitized evidence and complete cleanup.

Implementation evidence (2026-08-15; checkpoint not yet complete):

- the private presentation now rebuilds explicit root and prefix allowlists,
  owns exact Navigator/provider/utility pane roles and the supported geometry,
  creates or focuses at most one below-provider utility shell, routes literal
  `Ctrl+b` directly to the exact provider Runtime, and removes a normally or
  abnormally exited utility pane without weakening Navigator/provider
  `remain-on-exit` behavior;
- local shell launch resolves the exact running Workstream's canonical
  ProjectLocation and account shell. Remote launch uses fixed `ssh -tt` and
  bounded literal-input command vectors carrying only the endpoint executable
  and opaque Workstream ID; the authoritative host repeats exact state/root
  validation before launch, and control ABI 2 rejects an older endpoint before
  any interactive effect;
- deterministic and disposable coverage exercises allowlist drift, hostile
  tmux-format paths, exact geometry and guarded close, concurrent idempotence,
  failed-launch cleanup, local cwd, remote context retention, ABI preflight,
  literal nested-prefix delivery, and ordinary-tmux non-interference; and
- one uninterrupted `scripts/check` run passed 419 library tests, 10
  presentation recovery tests, 5 local transport tests, package and dependency
  policy, formatting and lint, every disposable acceptance harness, plus
  staged and unstaged whitespace checks. This evidence does not substitute for
  operator-visible terminal confirmation below; and
- the explicitly authorized acceptance harness subsequently passed every
  non-visual local and real-loopback-SSH assertion with complete cleanup and an
  unchanged ordinary-tmux inventory. It retained no terminal content and
  therefore left the local and SSH completed-output visual assertions to the
  operator. A subsequent installed-build check completed the local visual gate;
  only the SSH visual gate remains pending.

Non-goals and hard boundaries:

- no second shell, `Ctrl+b %` layout, persistent terminal entity, shell list,
  restoration record, shell title/rename model, or remembered shell policy;
- no command, output, history, scrollback, transcript, terminal capture,
  environment, credential, repository path, or raw SSH payload persistence;
- no Git lifecycle ownership or automatic fetch, pull, commit, push, branch,
  worktree, or conflict action;
- no provider Runtime split, manager-originated provider input injection,
  provider-pane management diagnostic, provider process/lifecycle change, or
  completed-output loss;
- no Navigator-side shell shortcut, generic terminal launcher, arbitrary tmux
  command surface, ordinary tmux configuration change, or claim that the
  private socket is a security boundary against an operator deliberately
  invoking tmux out of band; and
- no host/client schema or JSON host-protocol change. Protocol 18, host schema
  12, and client schema 5 stay fixed; only the independently versioned remote
  control ABI advances for the new interactive command.

Exit gate:

- deterministic configuration tests prove the presentation prefix and root
  tables are allowlists, `Ctrl+b "` is the only shell creator, `Ctrl+b %`
  cannot split, guarded close cannot target Navigator/provider panes, required
  existing mouse behavior remains intact, unsafe mouse menus and topology
  actions are absent, and the ordinary tmux server plus private Runtime
  configuration remain unchanged;
- deterministic nested-input coverage proves `Ctrl+b Ctrl+b` reaches the
  focused application as one literal control character and cannot leave the
  provider Runtime tmux client in prefix mode or invoke its key table;
- disposable tmux tests prove zero-to-one creation, repeated and concurrent
  create-or-focus idempotence, focus from every owned pane, both normal and
  failed shell-process cleanup, exact two-pane layout restoration, and
  fail-closed duplicate/unrecognized topology;
- host-state and command-vector tests prove exact local cwd, unknown or stale
  Workstream rejection, fixed remote SSH arguments with no repository path,
  remote host-side root resolution, and control-ABI mismatch rejection before
  interactive launch;
- switching Workstreams while a shell is open leaves its process and launch
  context unchanged while provider attachment continues to target only its
  exact owned pane;
- explicit operator-gated local and real SSH confirmation verifies hostname,
  cwd, a harmless Git inspection, provider interactivity and completed output,
  shell exit cleanup, detach/reattach behavior, and non-interference with an
  ordinary tmux session. Evidence contains no provider or shell capture; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

## Deferred beyond V1

The roadmap does not include arbitrary existing-session adoption, hard
Workstream/provider-session deletion, worktree or branch removal, checkout
synchronization, task/context transfer, transcript or memory features,
automatic plan rollover, provider/model/role launch presets, provider filters
or grouping, generalized provider onboarding, unproven OpenCode navigator
Rename, profile composition, Claude parity, multiple-controller catalog
synchronization, a public daemon, or a replacement provider UI.
