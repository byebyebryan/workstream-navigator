# Workstream Navigator V1 Roadmap

Date: 2026-09-04

Status: D0-D25 implementation is complete. D25, developed from `076b0a7`, now
includes an immediate shell-first exit correction after operator
falsification. The corrected source passes the full local/disposable gate; its
current remote Rust 1.88 result remains pending. Its locked release is
byte-identically installed for operator inspection with executable SHA-256
`1cb2518100afdb2dd1944674a4e59c690495bb31d90673ae3a89b22c2a738e5d`,
and sanitized native-exit acceptance on that exact artifact passes with Codex
0.153.2 and OpenCode 1.18.27. D25 stabilizes the existing D24 surface and
schema 15; it adds no user-facing session-management capability. No passing
current-source remote CI is included in the D25 acceptance record.
D18 checkpoint `c961c7e` retains the older separately
authorized destructive-reset and native observer-trust evidence.

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

## Completed checkpoint: D25 current-product stabilization and closure

Implementation status: the corrected source is locally and live-provider
accepted and installed for operator inspection. Its current remote Rust 1.88
result remains pending.

D25 closes deterministic and operational gaps found while reviewing the
current simplified product. It does not change the D24 product surface. The
initial Shell attachment now survives promotion as a hidden exact-identity
helper, so native provider exit reaches the same retained-pane reconciler used
by ordinary managed reattachment. Process and tmux publication races are
bounded at their observation seams without weakening identity or ambiguity
handling.

Scope:

- replace fixed scheduling in the private-client exit regression with exact
  client PID/session identity plus an exact session-change notification and a
  subsequent control-command ordering acknowledgement;
- keep `pane-died` as the normal attachment-return path, but when the exact
  provider PID disappears or becomes the same-birth zombie, allow one
  read-only private-tmux topology check to detach clients only from the exact
  generated session with one dead `provider:0` pane; lifecycle classification
  and mutation remain in the separately fenced reconciler;
- treat Linux `ESRCH` as a vanished process only for a `/proc` entry already
  found during process-group enumeration, while keeping direct identity,
  permission, malformed, and other I/O failures strict;
- distinguish a still-running exact provider detach from an exact zombie or
  narrow matching-process disappearance, and give only the latter a bounded
  retained-pane convergence window before requiring matching PID, exact
  ordinary or shell-promoted cwd proof, topology, and clean-exit evidence;
- normally require tmux status `0` plus process absence, but on Linux, when
  both tmux exit fields remain empty, accept only stable same-birth zombie
  evidence whose field-52 raw wait status decodes as normal exit `0`; refuse
  signals, malformed or conflicting status, live/reused identity, and any
  changed second read;
- after proven native clean exit, wait boundedly and read-only for the recorded
  numeric process group to drain, then re-fence Workstream/Runtime revisions,
  retained pane identity/topology/status, and group emptiness before private
  server removal; never signal an absent leader's unproven group, and leave
  generic stop semantics unchanged;
- remove an older stopped Runtime's retained private server before reusing its
  Runtime ID only through that same exact status-zero proof; refuse live,
  non-zero, reused, inaccessible, malformed, or ambiguous evidence;
- keep the provisional outer attachment alive through promotion, then join
  its immutable presentation ID/revision, slot generation, and candidate
  Runtime ID to one exact `provider_exec_proven` operation, owned Workstream,
  Runtime generation, and canonical private paths before delegating cleanup;
  an unpromoted live detach performs no state mutation, while an exact dead
  candidate may wait boundedly for durable proof and marker retirement;
- keep ordinary retained-exit launch-cwd equality strict, but allow a
  still-starting shell-promoted Runtime to use its exact current-generation
  `provider_exec_proven` project root when its pane was seeded before the shell
  changed into that root;
- make focus and retained-status tests wait on the exact current terminal/tmux
  evidence they assert, including a retained proof that begins at exact dead
  topology rather than assuming tmux has published separate exit fields, and
  accept an archived process only when it is absent or the exact same-birth
  Linux zombie after a bounded wait; preserve a disposable fixture's private
  socket until its exact tmux server and provider pane have stopped;
- source the exact generated account-shell wrapper noninteractively in wrapper
  semantics tests, while keeping separate exact tests for production's
  interactive shell argv and bootstrap contract;
- accept Codex's exact provider-native `[tui.model_availability_nux]` suffix
  with bounded model slugs and unsigned 32-bit counters, while refusing every
  other `tui` shape; accept an exact reviewed hook's optional provider-written
  `enabled = true` while refusing disabled or extended records, and keep
  incomplete hook review `trust_pending`;
- reconcile present-tense product documentation with the implemented compact
  name/marker rows while preserving all historical D0-D24 evidence; and
- build, atomically install, and checksum the locked release, and exercise
  sanitized Codex/OpenCode native exit on that exact artifact with complete
  disposable cleanup.

Non-goals:

- do not add or restore Fork, Rename, Ack, Park/Unpark, provider-thread
  archive/delete, transcript preview, bulk pruning, or any other public action;
- do not add a page, status, schema migration, compatibility route, provider
  traffic, terminal-content persistence, project cleanup, packaging change,
  or automatic provider relaunch; and
- do not generate or claim a current UI capture; capture selection and
  publication remain operator-owned.

Exit gate:

- focused regressions prove exact client identity/command ordering and status
  readiness, scoped Linux
  disappearance, detach-versus-exit convergence, stopped-retained cleanup,
  provisional promotion identity, shell-seed versus proven-project cwd,
  state-before-marker convergence, missing-tmux-status zombie reconciliation,
  signaled/conflicting-status refusal, transient group-drain convergence,
  persistent/probe-error refusal, bounded timeout, no-mutation live detach,
  exact Codex NUX acceptance, and arbitrary-TUI-state refusal;
- the corrected complete locked suite passes locally and on the declared Rust
  1.88 baseline, including Debian Bookworm with tmux 3.3a where the retained
  zombie fallback is exercised;
- `scripts/check` passes formatting, strict Clippy, all disposable tests,
  packaging, dependency policy, documentation links, and source boundaries;
- on the corrected D25 release, sanitized Codex 0.153.2 and OpenCode 1.18.27
  each reached `provider_exec_proven` without a work prompt, then immediate
  native `/exit` proved parked/stopped state, provider and private-server
  absence, and a live inert presentation surface; and
- corrected source and installed `wsnav 0.1.0` binaries are mode `0755`, size
  7,394,744 bytes, and SHA-256
  `1cb2518100afdb2dd1944674a4e59c690495bb31d90673ae3a89b22c2a738e5d`.

Remote evidence boundary:

- authenticated GitHub inspection from Starship recovered historical run
  `33883382546`, job `101057180882`, whose MSRV failure was the private-client
  detach timing regression;
- run `33931301731` at `993dc028c0818d40e861b3c3aae733bfcbae702a`
  passed its MSRV job but failed check job `101210258146`: tmux exposed the
  exact dead pane with blank exit fields while the configured `pane-died` hook
  had not fired, leaving the native client attached; and
- those failing runs are diagnosis evidence only. A passing current-source
  remote-CI result is still pending.

Evidence record:

- [D25 current-product stabilization and closure](evidence/acceptance/d25-current-product-closure.md)

## Completed checkpoint: D24 archived catalog and WSNav-owned Forget

Implementation status: locally accepted and installed for operator inspection.
No remote-CI or live-provider claim is made.

D24 keeps Archived as a secondary catalog and buffer zone for session
management. It offers the ordinary exact attach/start/resume/recover action on
an archived row while preserving `archived_at`; provider exit still stops an
explicitly opened archived Runtime. `u` restores visibility only. `x` opens a
distinct Forget confirmation and the revision-fenced public command
`wsnav forget <workstream-id> <revision>` provides scripting parity.

Scope:

- preserve active-only Archive, exact stop, and `n` semantics;
- authorize archived Enter/start/recover/attach through an explicit typed
  catalog scope without clearing `archived_at`;
- make Restore-only visibility reversal preserve any live Runtime;
- distinguish ordinary detach from native provider exit after attachment:
  preserve an exact live PID/birth unchanged, but exact-clean and park only one
  retained dead pane with matching PID/start cwd, absent process, and status
  zero; use an exact private-server `pane-died` hook to detach the client while
  retaining that pane as evidence, and repeat the proof at preflight for an
  older stranded clean exit, returning either proof through the same stopped
  presentation outcome;
- after a proven stopped outcome only, clear the stale right-hand terminal
  display using control sequences with no WSNav prose, and render a static gray
  `■` on stopped/internally parked cards that remains readable when selected
  while keeping live/idle cards unmarked;
- exact-stop a live owned Runtime before Forget, then commit one schema-15
  transaction that removes only the selected Workstream and its WSNav-owned
  graph, including dependent OpenCode rows, binding, Runtime, attention,
  creation-request metadata, and target-owned completed operation/exec-target
  rows; sever nullable child source lineage;
- refuse stale, ambiguous, unresolved, or shared operation effects while
  retaining the Workstream and preserving provider-native history,
  Project/Location/Git/files, and unrelated records; and
- keep the footer/help stable (`. view`, `? help`, `q quit`) with active
  `n new x archive` and Archived `u restore x forget` controls.

Non-goals:

- do not add provider-thread archive/delete, transcript preview, tmux
  read-only mode, bulk/automatic pruning, migration/reset, project cleanup,
  page-change lifecycle effects, or automatic restore/stop;
- do not make `n` available from Archived; and
- do not weaken private-tmux, observer/recovery, native TUI/naming/branching,
  or no-transcript boundaries.

Exit gate:

- focused state tests prove exact graph deletion, child-lineage severing,
  revision fencing, and refusal on unresolved operation effects;
- focused action/controller tests prove archived opening, restore-only
  behavior, exact-stop-before-Forget, retained-row failure semantics, detach
  preservation, exact zero-status provider-exit cleanup, retained-pane client
  detachment, and non-zero refusal;
- Navigator tests prove Archived Enter/Restore/Forget dispatch and the exact
  footer/help/modal copy plus the stopped-versus-idle marker distinction;
- CLI/source acceptance proves `forget` is public and revision-fenced while
  active-only `n`/Archive behavior remains; and
- `scripts/check` and the locked release build/install are recorded with exact
  artifact identity; live-provider operator acceptance remains separate.

Evidence record:

- [D24 archived catalog and WSNav-owned Forget](evidence/acceptance/d24-archived-catalog-forget.md)

## Completed checkpoint: D23 provider-native stop and contextual visibility

Implementation status: locally accepted and installed for operator inspection.
No remote-CI or live-provider claim is made.

D23 removes the duplicate user-facing Park lifecycle. Exiting the provider's
native TUI is the ordinary stop-and-keep-visible path; `Enter` remains the sole
attach, resume, or recovery action. WSNav retains its exact internal Runtime
stop only where a compound catalog action requires it.

Scope:

- retain contextual `n` for a separate Workstream and `Enter` for
  attach/start/resume/recovery;
- remove `p`, Park help/footer/status copy, the parked card marker, and the
  public `wsnav park` command while retaining current-schema internal exact-stop
  authority;
- make Archive available only from Workstreams and perform exact Runtime stop
  before hide, including terminal onboarding-recovery resolution after exact
  cleanup so no recovery row is stranded by Park removal;
- make Restore available only from Archived, clear visibility without launch,
  and atomically normalize only internal `parked` to `open` while leaving the
  Runtime stopped;
- render existing active schema-15 `parked` records like stopped Workstreams
  and keep them resumable through `Enter`, without migration or schema change;
  and
- keep a stable `. view`/`? help`/`q quit` footer row, add page-local session
  actions above it, and keep Archive and Restore from being advertised
  together in either the footer or floating Help panel.

Non-goals:

- do not remove the schema-15 `parked` lifecycle value, add a migration, or
  weaken exact process/tmux/revision/operation ownership;
- do not intercept provider exit, add an Unpark action, automatically start on
  Restore, or hide a provider whose cleanup is ambiguous;
- do not add hard deletion, provider-history deletion, or Project/Git cleanup;
  and
- do not rewrite historical acceptance evidence that truthfully exercised the
  former public Park surface.

Exit gate:

- focused state tests prove Restore clears archive and normalizes only
  `parked` to `open` transactionally while preserving stopped Runtime, binding,
  attention, and revision fencing;
- focused action/controller tests prove Archive exact-stops before hide,
  resolves terminal onboarding recovery only after exact cleanup, and leaves
  the Workstream visible on every ambiguity or failure;
- Navigator tests prove page-local Archive/Restore dispatch, footer, and help,
  no public Park key or marker, and unchanged `n`/`Enter` behavior for active
  stopped and legacy parked records;
- CLI/source acceptance proves `park` is absent while Archive/Restore and all
  unrelated supported commands remain available;
- `scripts/check` passes against disposable state, provider, and private-tmux
  fixtures without ordinary-state mutation; and
- the locked release is built, atomically installed, and verified by version
  plus executable hash for operator inspection.

Evidence record:

- [D23 provider-native stop and contextual
  visibility](evidence/acceptance/d23-native-stop-contextual-visibility.md)

## Completed checkpoint: D22 exact live recovery confirmation

Implementation status: correction complete in `8338a04`, locally accepted,
installed with executable SHA-256
`30fd8bf0c6ac220b9c10088ddd623eec7dc301ffd77f1a5a4d4f36fccdfa5784`,
and accepted by an explicit operator recovery re-test. The first candidate in
`ed74b0b` passed its local/disposable gate and was installed with SHA-256
`1bbf53aa5ca1a02930140cca1ad8358e8f9b0b632311bd18ceee82017c084fe1`,
but subsequent operator inspection falsified its executable-name proof.

D22 makes an interrupted Codex recovery handshake retryable without stopping,
restarting, steering, or otherwise mutating the live provider. A missed or
rejected native `SessionStart(source=resume)` may leave an exact WSNav-launched
Runtime alive while its Workstream remains `recovery_required` and its Runtime
remains `starting`. Selecting Recover again must no longer return an inert
`AlreadyLive`: it may reconcile only the retained session under a second exact,
bounded evidence path.

Scope:

- recognize only a non-archived Codex Workstream in `recovery_required` with a
  `starting` Runtime, a retained exact ProviderBinding, and an exact live
  private-tmux/process identity;
- on explicit Recover, prove that the live executable and argument vector are
  the exact WSNav-generated `codex --profile wsnav-observer -C <cwd> resume
  <retained-session>` invocation, then require bounded read-only
  `thread/read(includeTurns=false)` to return that same retained native session;
- revalidate Workstream, Runtime, binding, generation, session, and revision
  evidence in one transaction, rotate only the existing binding to the current
  Runtime generation, and reopen the Workstream while leaving the Runtime
  `starting` until native lifecycle evidence advances it;
- return a distinct successful action outcome so the controller can refresh the
  card immediately; on unavailable or ambiguous proof, retain `!` and show
  bounded retry guidance outside provider content; and
- reconcile current product documentation and disposable acceptance evidence
  without changing schema 15.

Non-goals:

- do not synthesize a hook, infer a provider session from inventory or order,
  accept an unbound/native-picker recovery, or weaken `SessionStart` plus
  `thread/read` corroboration for initial or changed-session binding;
- do not read provider pane content or persist prompts, responses, terminal
  capture, raw provider payloads, command output, or a new recovery journal;
- do not automatically poll App Server from rendering, clear recovery from
  process liveness alone, or change OpenCode recovery; and
- do not stop, restart, signal, steer, or send input to a live provider while
  confirming it.

Exit gate:

- focused tests prove exact live retained-session recovery succeeds and every
  executable, argument, provider, session, generation, lifecycle, and revision
  mismatch fails without mutation;
- executable tests accept only the ordinary absolute `codex` name or Linux's
  exact `codex (deleted)` `/proc/<pid>/exe` tombstone while rejecting relative,
  alternate, and suffixed lookalikes;
- tests prove the successful transaction updates only the binding generation
  and Workstream lifecycle/revision while Runtime status remains `starting`,
  and that initial, changed-session, unbound, OpenCode, and ambiguous recovery
  boundaries remain unchanged;
- `scripts/check` passes against disposable state roots, repositories, provider
  homes, fake App Servers/process metadata, and private tmux sockets; and
- the locked release is built, atomically installed, and verified by version
  and executable hash. Opening or mutating the currently live provider remains
  separately authorized operator acceptance.

Falsification and correction boundary:

- after closing and reopening WSNav, explicit Recover against the retained live
  session still refused without state mutation;
- read-only inspection proved the exact Runtime topology, recorded PID/birth,
  cwd, and generated resume argv still matched, while `/proc/<pid>/exe`
  reported `/usr/bin/codex (deleted)` because the executable had been replaced
  during a Codex upgrade; and
- the correction recognizes only that exact Linux kernel suffix and retains all
  other D22 proof and transaction fences. It does not resolve an ambient PATH,
  adopt the replacement file, or compare against the obsolete executable inode
  retained from an earlier Runtime generation; and
- after reopening the corrected installed binary, explicit Recover succeeded:
  the Workstream advanced from `recovery_required` to `open`, the retained
  binding advanced to the current generation, the Runtime remained the same
  `starting` record and revision, the same provider process remained live, and
  unresolved operations remained empty.

Evidence record:

- [D22 exact live recovery acceptance](evidence/acceptance/d22-exact-live-recovery.md)

## Completed checkpoint: D21 provider-derived attention

Implementation status: complete in `868ee85`; locally accepted and installed
for operator inspection. See the
[D21 acceptance record](evidence/acceptance/d21-provider-derived-attention.md).

D21 makes the session card a projection of provider and recovery lifecycle,
not a second inbox whose read state the operator must manage. A completed turn
renders from the Runtime's observed `attention` status and naturally yields to
the next observed provider transition. Displaying, selecting, or focusing a
Workstream never writes an acknowledgment.

Scope:

- remove Navigator `a`, public `acknowledge`, their help/footer entries,
  controller action, revision fence, and result-seen mutation;
- render the completion marker directly from `RuntimeStatus::Attention`, so a
  subsequent provider prompt correctly renders `Working` without an unrelated
  manual clear;
- derive recovery presentation only from Workstream/onboarding recovery state,
  which can be cleared only by the existing exact recovery lifecycle;
- remove `AttentionState` from the current domain, snapshot, registry, and
  lifecycle-write paths, including its duplicated native session/turn
  identities; the exact current provider binding remains the sole retained
  conversation-tip authority;
- preserve schema 15 without reset or migration by retaining the existing
  `attention_states` table and columns as ignored historical storage until a
  future intentional state epoch removes them; and
- reconcile current product documentation and generated CLI acceptance while
  preserving dated evidence and prior checkpoint records as historical facts.

Non-goals:

- do not auto-acknowledge on Enter, mouse activation, tmux focus, attachment,
  or provider cycling;
- do not change Runtime/Workstream recovery transitions, activity ordering,
  provider binding, last-settled-turn evidence, archive/park semantics,
  private-tmux topology, or provider hooks; and
- do not remove or fold the diagnostic `operations` command in this
  checkpoint.

Exit gate:

- focused tests prove the acknowledgment CLI/key/action surfaces are absent,
  completion and Working markers follow exact Runtime state, recovery markers
  follow exact recovery lifecycle, and legacy attention rows neither affect
  snapshots nor receive lifecycle writes;
- `scripts/check` passes against disposable state roots and private tmux
  sockets;
- the installed schema-15 state is inspected before replacement and the new
  build opens it without rewriting or requiring a reset; and
- the locked release is built, atomically installed, and verified by version
  and executable hash. Live provider interaction remains separately authorized.

## Completed checkpoint: D20 native-owned conversation branching

Implementation status: complete in `00a4937`; locally accepted and installed
for operator inspection. See the
[D20 acceptance record](evidence/acceptance/d20-native-owned-branching.md).

D20 removes every ordinary and break-glass route by which WSNav creates,
reconciles, or recovers a provider conversation Fork. A provider-native
conversation cutover remains inside the same durable Workstream and rotates
only its exact current provider binding when ordinary observer evidence proves
the new native session. `n` remains the explicit route to a separate blank
Workstream at the selected Location.

Scope:

- remove Navigator `f`/Fork and `r`/Fork-recovery controls, Fork operation
  rows, public `fork-workstream` and `recover-operation`, and their controller
  actions;
- remove Codex `thread/fork`/fork-reconciliation and OpenCode fork mutation,
  plus the Fork-specific action and durable-operation state machines;
- keep the compound-operation journal required by onboarding and OpenCode
  blank-session Start, but expose no provider-conversation mutation through it;
- retain historical `WorkstreamOrigin::Fork` records and their ordinary
  attach, park, archive, restore, and native-resume behavior without rewriting
  or deleting provider history;
- fail closed with explicit typed diagnosis when schema-15 state contains a
  previously attempted unresolved Fork effect; never discard, retry, infer, or
  adopt it;
- keep completed historical Fork journal rows inert, and make no state epoch,
  migration, reset, Git, worktree, prompt, transcript, provider-input, or
  private-tmux topology change; and
- reconcile current product documentation and generated CLI acceptance while
  preserving dated evidence and prior checkpoint records as historical facts.

Exit gate:

- focused tests prove the removed CLI/key/action/provider surfaces are absent,
  native same-Workstream session rotation remains exact, historical Fork-origin
  Workstreams remain readable, and unresolved legacy Fork effects refuse
  without mutation;
- `scripts/check` passes against disposable state roots and private tmux
  sockets;
- the installed state root is inspected with the previously accepted binary
  before replacement and contains no unresolved Fork operation; and
- the locked release is built, atomically installed, and verified by version
  and executable hash. Live provider interaction remains separately authorized.

## Completed checkpoint: D19 tmux-derived presentation navigation

Implementation status: complete in `a0ec38b`; locally accepted and installed
for operator inspection. See the
[D19 acceptance record](evidence/acceptance/d19-tmux-navigation.md).

D19 tightens the existing two-pane shell-first presentation without adding a
page, pane, window, provider, state schema, provider effect, or persisted UI
preference. Its governing separation is:

- tmux alone owns which presentation pane receives keyboard input;
- the Navigator owns its process-local row selection;
- the presentation controller owns which exact shell, review, or managed
  Runtime surface appears in the right pane; and
- lifecycle actions retain their existing revision-fenced state authority.

The checkpoint first established disposable tmux behavior and recorded three
baseline falsifications, then implemented the single focus authority, closed
tables on both private tmux layers, bounded provider-pane Workstream switching,
the full gate, and installation as one coherent candidate.

[Spike 0028](evidence/spikes/0028-d19-navigation-readiness.md) records the
pre-implementation study. It falsifies reuse of D18's permissive Runtime
tables/topology probe, mutating attachment preflight, and ProjectId-ordered
group projection for D19. The checkpoint corrects those implementation gaps;
it does not weaken the focus, no-provider-effect, or shared-order boundaries.

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
Its mode-`0600` attachment status is the presentation-private synchronization
boundary by which Navigator observes the destination's provider-cycle
`Running` phase and aligns its page/selection once; D19 adds no listener,
general event bus, tmux
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
- tmux focus events drive only the Navigator page-title color, making the exact
  active pane visible without a separate header, Navigator polling, focus
  authority, or provider-pane write; a real disposable client proves the
  initial and both directional focus transitions from Navigator-only output;
- repeated fresh starts prove the exact two-pane roles and closed presentation
  controls are published before startup returns; the private socket identity is
  committed before the exact Navigator pane is launched, only the bounded
  transient provider-topology `InvalidTopology` observation may retry, and
  persistent or unrelated failures remain closed; and
- `scripts/check`, the declared MSRV job, nested terminal input/fidelity tests,
  and staged/unstaged diff checks pass before the locked release is installed
  for operator inspection.

No partial D19 slice is an install candidate. Live-provider acceptance, if the
final implementation needs it, remains separately authorized and
artifact-bound.

The complete local gate passed for `a0ec38b`: `scripts/check` passed 369
library and 8 presentation integration tests together with formatting, strict
Clippy, packaging, dependency policy, semantic acceptance, documentation, and
diff checks. A clean Rust 1.88.0/Debian/tmux 3.3a container passed the same 377
locked all-targets/all-features tests. The locked release was installed
byte-identically for operator inspection. No real provider was launched for
D19 acceptance; the composed deterministic switching proof and remaining
evidence limitations are recorded in the acceptance record.

Before the startup closure, the focus-and-frame refinement plus compact-footer
tightening passed `scripts/check` with 372 library and 8 presentation
integration tests. That source mounted read-only in a Rust
1.88.0/Debian/tmux 3.3a container passed the same 380 locked
all-targets/all-features tests before its locked release was installed
byte-identically.

The final startup/focus candidate passes `scripts/check` with 372 library and
10 presentation integration tests. The current source mounted read-only in a
Rust 1.88.0/Debian/tmux 3.3a container passes the same 382 locked tests,
including 16 consecutive fresh detached starts and the real-client
green/dark-gray/green focus proof, before its locked release is installed
byte-identically for operator inspection.

The post-rename documentation review then exposed a second, independent
fresh-start race on tmux 3.3a: Navigator could inspect the ownership marker
while its parent rewrote that marker with the new private socket identity. The
failure stayed closed as a dead Navigator pane and `InvalidTopology`. The
current correction starts pane `0.0` inert, captures the socket identity, and
only then launches Navigator in that exact pane; it preserves the existing
20-by-5-ms provider-topology retry. In the exact Rust 1.88.0/Debian/tmux 3.3a
environment, five focused startup fixtures and five uninterrupted full locked
matrices pass serially, covering 160 fresh starts in that environment.

An operator capture then exposed presentation-local guidance that outlived the
transient reconciliation failure which created it. The correction preserves the
fail-closed warning while proof is unavailable, then clears only that exact
warning after successful provider-exec proof or normal completed-marker
retirement. Later unrelated guidance remains visible. This changes no durable
state, provider process, pane content, or reconciliation authority.

The same refinement replaces the stacked list/footer outlines with one
continuous green frame around the entire Navigator. The adjacent tmux boundary
uses a white foreground and default background in both focus states, with
half-border indicators disabled. Focus remains indicated only by the page-title
color. The compact footer omits `↑↓` selection, `Enter` open/shell, and `a`
acknowledge-result hints while retaining them in the complete `?` reference.

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
| D19 | Tmux-derived presentation navigation; locally accepted for operator inspection | [D19 acceptance](evidence/acceptance/d19-tmux-navigation.md) |
| D20 | Provider-native conversation branching; managed Fork creation and recovery retired | [D20 acceptance](evidence/acceptance/d20-native-owned-branching.md) |
| D21 | Provider-derived attention; Navigator acknowledgment and duplicate sticky state retired | [D21 acceptance](evidence/acceptance/d21-provider-derived-attention.md) |
| D22 | Exact live retained-session Codex recovery confirmation | [D22 acceptance](evidence/acceptance/d22-exact-live-recovery.md) |
| D23 | Provider-native stop; public Park retired and archive/restore made contextual | [D23 acceptance](evidence/acceptance/d23-native-stop-contextual-visibility.md) |
| D24 | Archived secondary catalog and WSNav-owned Forget; locally accepted and installed for inspection | [D24 evidence](evidence/acceptance/d24-archived-catalog-forget.md) |
| D25 | Current-product lifecycle stabilization; corrected local artifact installed, prior artifact retains MSRV and sanitized live-provider evidence | [D25 evidence](evidence/acceptance/d25-current-product-closure.md) |

## Deferred product decisions

- Worktree detection, never worktree management.
- Any future provider, release channel, packaging system, daemon, or richer
  metadata workflow.
