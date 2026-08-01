# Workstream Navigator V1 Roadmap

Date: 2026-08-01

Status: D0 through D6.9 complete. D7 is expanded into navigator workflow and
lifecycle management; its observer-activation slice is implemented and ready
for bounded native acceptance. D7.1's navigator page, grouping, key-reference,
and mouse foundation is implemented; its stateful management slices remain
pending.

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
| D6.9 | Codex observer authority repair | Complete |
| D7 | Navigator workflow and lifecycle management | In progress |

The completed checkpoints describe the source-installed operator-beta at the
time of their acceptance. [Spike 0009](spikes/0009-codex-hook-environment-boundary.md)
subsequently falsified its launch-environment observer authority. [Spike
0010](spikes/0010-codex-hook-ancestry-authority.md) validates a strict
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

## D5.2 - Correctness closure

Implementation status: complete. The declared Rust 1.88 toolchain now matches
the locked production graph and passes its pinned CI job. Stable project
labels, fail-closed tmux probes, authority-before-provider launch, observable
attachment outcomes, process-group deadlines, scrolled mouse targeting, and
cursor-paged snapshots passed the full local and disposable gates. See the
[D5.2 correctness closure acceptance](acceptance-d5.2-correctness-closure.md).

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
the [D6 operator-beta acceptance](acceptance-d6-operator-beta.md).

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

Implementation status: complete. Linked-checkout normalization, canonical
remote fingerprinting, client-side cross-host Project grouping, development
schema migration, and the full repository gate passed. See the
[D6.1 project-identity acceptance](acceptance-d6.1-project-identity.md).

Refine the accepted operator beta without changing provider, Runtime, or
worktree ownership. Make the existing client-side Project concept useful when
the same repository is registered at different paths or on different hosts.

Scope:

- normalize new registrations to the selected Git worktree root while keeping
  the primary worktree as a separate stable repository command path;
- derive one credential-free, transport-normalized fetch-remote fingerprint
  through bounded local Git inspection without network access;
- expose only that opaque fingerprint and a bounded repository name through a
  versioned host snapshot;
- reuse a client Project ID when exact fingerprints match, while keeping
  missing or ambiguous identities separate;
- migrate current development schemas without importing the Python prototype
  or weakening matching-build remote checks; and
- retain per-host Location and Workstream authority beneath the presentation
  grouping.

Exit gate:

- linked-worktree registration records the selected checkout and primary
  repository path separately;
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
or presentation-tmux ownership boundary.

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
selection, and deterministic terminal coverage. No durable or provider-facing
behavior changed.

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

Implementation status: observer activation is ready for native acceptance.
D7.1 supplies the Workstreams, Projects, and Hosts navigation foundation;
D7.2 now supplies revision-guarded archive/restore through local and SSH host
contracts, Active/Archived navigator scopes, bounded Workstream status,
canonical rename, and exact local/remote unresolved-operation reconciliation.
D7.3 now exposes bounded host-owned ProjectLocations with active/archived
counts and supports starting at a selected retained location; checkout
registration remains planned. Host management remains planned.
D7 makes ordinary WSNav administration available through the navigator without
turning it into a task manager or replacing the provider surface.

Scope:

- make the two-pane TUI sufficient for every ordinary WSNav-owned operation
  after external installation prerequisites, with CLI commands retained only
  as optional scripting, diagnostics, direct attachment, and break-glass
  parity;
- retain Workstreams as the default page and add sibling Projects and Hosts
  pages inside the existing navigator pane, with mouse and keyboard switching,
  nested detail, and page-specific help;
- retain page-local single-key actions as the canonical terminal control path,
  with a separate status line, a compact action-boundary-wrapped key strip, and
  a `?`-toggled single-column expanded reference at the bottom of the pane;
- add reversible Workstream archive/restore as a visibility concern separate
  from runtime lifecycle, preserving provider binding, attention, lineage,
  checkout, branch, and native history;
- expose bounded Workstream status, canonical rename, attention acknowledgement,
  and exact unresolved-operation recovery through the Workstreams page;
- add Project inventory and local/remote ProjectLocation registration without
  cloning, syncing, deleting, or exposing remote repository paths, including
  the empty-navigator flow and starting at a selected ProjectLocation;
- add Host registration, health, verification, observer activation, and
  exact observer removal plus client-only forget, while protecting the local
  host and leaving remote state untouched;
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
2. **D7.1 - Management navigation foundation.** Add the three top-level pages,
   list/detail navigation, mouse behavior, and direct page-local keys without
   changing provider state. Refine the narrow Workstreams pane with two-line
   Recent rows, explicit two-line tree children in grouped views, the `Recent`
   / `By project` / `By host` cycle, compact bottom key hints, and a
   single-column expanded reference while retaining the accepted Workstreams
   bindings. Each later stateful action owns its bounded text entry,
   confirmation, and non-blocking progress path; D7.1 deliberately does not
   ship an unused generic modal.
3. **D7.2 - Workstream lifecycle and recovery.** Add bounded status and
   canonical rename, preserve existing open/new/fork/park/acknowledge keys, add
   revision-guarded local/remote archive visibility and restore-without-start,
   and make exact unresolved Start/Fork reconciliation available through the
   Workstreams Recovery page. The page carries only an opaque operation handle
   long enough to issue exact local or SSH reconciliation; its renderer exposes
   host, operation kind, and phase only. Archive/restore, scope selection,
   bounded status, canonical rename, and recovery are complete.
4. **D7.3 - Project management.** List logical Projects and their host-owned
   locations, show active/archived counts, register the first or an additional
   existing checkout on a selected local or SSH host, and start a Workstream at
   a selected location without requiring an existing active row. Location
   inventory, counts, and starting from a retained archived source are
   implemented; navigator checkout registration follows as a separate commit.
5. **D7.4 - Host management.** Register, verify, activate, remove the exact
   observer from, and forget SSH hosts through the navigator while preserving
   client/host ownership boundaries. Carry the native review boundary proven
   in D7.0 through the remote Host detail flow.
6. **D7.5 - Integrated acceptance.** Exercise fresh local and remote setup,
   Project registration, Workstream lifecycle/recovery, observer removal, and
   host forget/re-register using only the two-pane TUI after installation,
   without provider-pane management traffic or remote Runtime interference.

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
  restore, unresolved-operation recovery, observer removal, and host
  forget/re-register without entering another `wsnav` shell command;
- the native provider result and input surface remain untouched until the user
  explicitly chooses a Workstream or observer-review action; and
- formatting, tests, lint, package checks, and `git diff --check` pass.

## Deferred beyond V1

The roadmap does not include arbitrary existing-session adoption, hard
Workstream/provider-session deletion, worktree or branch removal, checkout
synchronization, task/context transfer, transcript or memory features,
automatic plan rollover, profile composition, Claude parity,
multiple-controller catalog synchronization, a public daemon, or a replacement
provider UI.
