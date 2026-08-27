# Workstream Navigator V1 Roadmap

Date: 2026-08-26

Status: D0-D16 complete; D16 host-local implementation, disposable repository
gate, and operator-gated live local and SSH-entered-host acceptance complete;
source-installed operator beta. D17 shell-first managed-session onboarding is
the approved next checkpoint. Its D17.0 falsification studies, D17.2 test-only
ownership/private-runtime model, dormant presentation-private marker-backed
materialization/evidence storage, marker-to-state prepare/consume broker,
typed helper/pre-exec/post-exec reconciliation fences, dormant account-shell
bootstrap, account-shell context/system-gate and post-exec-reconciliation
composition, dormant direct-Codex and OpenCode exec preparation with the
provider-specific final-effect fences,
and dormant D17.3 grammar, command-classification,
onboarding-phase, capability-journal, atomic
reservation, and ownership-consumption foundations are in progress; no D17
user-facing behavior or routed provider launch path is
implemented yet. The explicit schema-13-to-14 migration, stable provisional
lease, schema-14-only open, lease acquisition, reservation, and
ownership-consumption seams, plus marker-backed materialization/evidence
storage, are dormant, and no product path can invoke any of them.

The D0-D15 entries below preserve truthful historical implementation and
acceptance evidence. D3 and later SSH, remote, cross-host, and combined
local/remote descriptions document the former WSNav-managed surface and are
retired or superseded by D16 for the current product contract.

## 2026-08-26 D17 shell-first onboarding decision

D17 dissolves the Projects page into the ordinary Workstreams experience.
Workstreams always shows one pinned `New session · shell` card outside Project
groups. At presentation creation WSNav captures, validates, and canonicalizes
the invocation cwd as a presentation-private seed cwd. Selecting the card
lazily materializes exactly one opaque candidate `RuntimeId` and creates its
provisional tmux directory, socket, configuration, and session using the
existing final full-UUID `RuntimePaths` fields (directory, socket,
configuration, and session). The candidate ID, exact
`RuntimePaths` fields (directory, socket, configuration, and session), seed, and
shell/server ownership evidence live only in the presentation-private marker;
they are not a registry Runtime or Workstream row. Before creating those
artifacts, materialization proves the candidate ID and all four path fields are
absent and unused; it never adopts pre-existing artifacts. A marker-backed
candidate is excluded from ordinary registry inventory, probe, park, remove,
and recovery discovery/action until durable adoption; only the exact
presentation marker plus the stable host-private `provisional.lock` lease may
manage it. Markerless/registryless, foreign, or collision artifacts remain
untouched, and a clean replacement allocates a fresh candidate RuntimeId.
Every newly materialized clean shell starts at that seed, while detach/reattach
preserves a live shell's actual cwd. A missing, deleted, unsafe, or ambiguous
seed makes onboarding unavailable with guidance; it never falls back or becomes
Project authority. A new presentation captures its own seed. The pinned card is
a derived singleton with no durable card row, and each materialization mints a
fresh opaque `slot_generation` bound by the marker, capability, and onboarding
journal.

The one serialized ownership handoff is a stable host-private
`provisional.lock`, distinct from D16's schema-cutover `transition.lock`. It is
operational state, not a Runtime/card/Workstream row or presentation-private
storage. Schema-14 host-operational lease metadata stores only a planned
`lease_generation`, install phase `pending` or `ready`, and expected lock
device/inode once ready; it is not a card, Runtime, Workstream, or
presentation-private row. The schema/HostId transaction commits schema-14
ownership and this pending metadata first; schema-13 code and path never create
or recognize `provisional.lock`.

Only after that database commit is durable may schema-14 startup reconcile the
stable artifact. In `pending`, an absent artifact is created lazily as a
mode-`0600` current-owner regular file with create-new/no-follow semantics;
startup writes bounded file contents, fsyncs the file, then fsyncs the containing
state-root directory, and transactionally
finalizes metadata as `ready` with expected device/inode. An exact file left by
a crash after file creation may instead be validated and locked, then finalized
the same way. Pending foreign or mismatched evidence fails closed. In `ready`,
a missing, replaced, or device/inode-mismatched artifact fails closed and is
never recreated. The file contains only bounded format version, HostId, and
`lease_generation`; it contains no cwd, command, argv, provider/user content, or
provider payload. Malformed, symlinked, foreign, replaced, or locked evidence
fails closed. Normal D17 operation never unlinks/recreates it; state-root
reset/removal is outside this flow. A lock artifact seen before schema-14
ownership is unexpected/ambiguous, remains untouched, and is never adopted or
deleted. A crash between the database commit and file creation is retried
safely in `pending`; no cross-store atomicity is claimed. Every
materializer, broker, helper, confirmed close/loss
cleanup, and singleton reconciler opens it no-follow/CLOEXEC, acquires one
nonblocking exclusive kernel lock, retains the FD, and revalidates canonical
root/path plus open-FD device/inode identity before mutation. Crash releases
the kernel lock without changing the file; restart reacquires that same
artifact and reconciles marker/journal. The FD never crosses provider exec;
busy/timeout returns bounded guidance and never creates a second lock or
proceeds unlocked. Marker, capability, and journal bind both
`lease_generation` and `slot_generation`.

Each presentation derives one pinned provisional card, but the shared host
`provisional.lock` and classifier permit at most one unregistered materialized
candidate server across all presentations. A valid marker/artifact belonging to
another presentation is busy/owned, not unknown or adoptable; that presentation's
card remains visible but unavailable until its slot promotes or conclusively
cleans. Under the lock, a bounded classifier cross-checks the exact marker and
unfinished onboarding operations against registered Runtime IDs and the bounded
`run/runtime-*` namespace only to detect conflicts. It never passively adopts or
deletes unknown artifacts. Missing/changed marker evidence plus an unregistered
Runtime-shaped artifact, multiple candidates, or ambiguous journal/path/process
evidence blocks every fresh materialization and leaves artifacts untouched; it
cannot evade ambiguity by choosing a new UUID. A clean replacement is allowed
only after exact prior absence or conclusive cleanup and receives a fresh
`slot_generation` and candidate RuntimeId.

Every participant revalidates the marker, onboarding journal, and
presentation revision and registry generation while holding `provisional.lock`. The broker
validates the shell's current cwd, detects the exact non-bare Git worktree root,
and transactionally generates/reserves the durable Runtime generation and graph
for the exact candidate ID and unchanged full-UUID `RuntimePaths` fields
(directory, socket, configuration, and session), then marks the request handoff
issued. A prepared reservation does not revoke provisional cleanup. Before the
helper successfully revalidates every bound marker/process/cwd/path/revision/
token claim and atomically consumes the capability while committing durable
`Runtime-owned` authority, close/loss may win only under the same
`provisional.lock` lease by
atomically canceling/revoking an issued but unconsumed capability, proving
pre-effect absence, rolling back attempt-only rows, and cleaning exact
provisional artifacts. The helper instead reacquires the lock and, while
holding it, revalidates every bound claim; only on successful revalidation does
it atomically compare-and-consume the capability and commit durable
`Runtime-owned` authority for the candidate. A mismatch does not advance
ownership. It then, still under `provisional.lock` and before releasing it,
revokes/removes presentation cleanup authority; durable transition precedes
marker cleanup, and only afterward may provider effects be prepared or
executed. After that exact helper commit, presentation cleanup never signals
the pane, process, or server. Ambiguous cross-store crash windows stay in the
onboarding journal for recovery; conclusive pre-effect rollback after transfer
belongs to onboarding recovery.

For a promotable fresh interactive native TUI shape, the controlled function
invokes a bounded prepare broker as a child over private non-terminal control
I/O. The broker returns only an exact one-shot opaque launch capability, never a
provider command or argv. Its claims bind the request/operation,
presentation/slot, candidate ID and exact `RuntimePaths` fields (directory,
socket, configuration, and session), fixed provider, exact shell cwd and
root/Location, reserved Runtime generation, captured revisions, shell
PID/birth/process group, grammar-approved argv digest, and short monotonic
expiry. The helper's lease-held revalidation covers every bound
marker/process/cwd/path/revision/token claim, including the candidate ID and all
four `RuntimePaths` fields. Only on successful revalidation does the helper
atomically compare-and-consume the capability and commit durable `Runtime-owned`
authority; a mismatch does not advance ownership. The helper then, still under
`provisional.lock` and before releasing it, revokes/removes presentation cleanup
authority; durable transition precedes marker cleanup, and only afterward builds
provider argv internally, prepares provider effects, and `exec`s the provider,
preserving the shell leader PID,
birth token, and process group. Persisted state keeps only bounded token
identifier/verifier, phase, and claim references or digests; the live token,
argv, shell command line, environment, terminal capture, and provider payload
are never persisted.

The helper's successful revalidation, atomic capability consume, and durable
`Runtime-owned` commit do not yet make the Runtime ordinarily attachable or
actionable. The request-keyed `CompoundOperation` enters
`runtime_owned_launching` (no provider effect), then provider-specific
preparation/external-effect phases, and `provider_exec_started` immediately
before `execve`; terminal outcomes are `provider_exec_proven`, known-absent
exec failure, or `recovery-required`/`unknown`. Until full exec proof or terminal
reconciliation, attachment and action authority for that unproven Runtime
remains fenced. Its
originating presentation may retain its existing Runtime attachment/pane or
detach through ordinary card switching, but no new attachment to that Runtime
is allowed. Selecting/materializing the fresh derived singleton card attaches
only its separate provisional server under `provisional.lock` and grants no
authority over the unproven Runtime. Park/Resume/Fork/contextual
`n`/`new-workstream`, archive, Rename, recovery/start retry, and cleanup actions
for that Runtime refuse or wait with bounded `onboarding-in-progress` guidance.
Passive snapshot/probe may show `starting`/`onboarding` and reconcile, but must
not adopt helper/preparation processes, mark the Runtime lost, signal it, or
expose ordinary action authority. Once terminal `recovery-required`, only exact
recovery or explicit Park rules apply. A terminal known-absent exec result is
not itself action authority: the reconciler must atomically resolve it. When
provider-specific journal evidence proves no prior external effect or binding,
guarded rollback ends onboarding and leaves the derived singleton card available
but unmaterialized. When OpenCode has a known blank-session POST or binding, the
same atomic resolution ends onboarding in the exact stopped/recovery state; only
binding-preserving Resume/recovery or explicit Park is then allowed. A possible
effect remains `recovery-required`. No ordinary action is enabled directly by
exec-error evidence, and no operation remains fenced after terminal
reconciliation. A host-local reconciler invoked by passive snapshot/action
preflight or restart recovery performs no provider effect; only after
revalidating the operation/revisions, RuntimeId/generation and exact
`RuntimePaths` fields
(directory, socket, configuration, and session), tmux pane/session,
same PID/birth/PGID/session, and expected executable does it atomically commit
`provider_exec_proven` and activate ordinary attachment/action authority.
An authoritative Codex hook contributes only through that same identity/revision
proof; an OpenCode sidecar or server identity is never native-TUI exec proof.
An exact helper `execve` error proves only absence of the final provider TUI
exec. Attempt-only graph rollback is allowed only when the provider-specific
journal also conclusively proves no prior external effect or binding; a crash
after exec-start without proof is ambiguous and never rollback authority.

Card and server state key off Runtime ownership, not provider success. Before
the exact helper commit the selected card remains the exact shell. Once
ownership commits, it becomes the managed Workstream and the UI derives one
fresh unmaterialized singleton card even when native binding is not ready;
the launch fence above still applies until exec proof or exact recovery. For
OpenCode, any possible non-idempotent `POST /session` effect leaves the same
server Runtime-owned and the card visibly `recovery-required`, even if no
native TUI remains; presentation cleanup cannot touch it and recovery never
issues a second POST. A conclusive pre-effect failure after the exact helper
commit is classified by onboarding recovery; it rolls back attempt-only graph
state only when provider-specific evidence proves no effect or binding, leaving
the derived singleton card available but unmaterialized. An ambiguous-effect
slot is never reusable. Codex may remain managed `starting` and unbound until
`SessionStart`; a known OpenCode blank-session POST or binding remains on the
same Runtime/Workstream/binding for exact recovery/resume after a final TUI
failure and is never rolled back or posted again.

D17 supports Bash and Zsh interactive non-login account shells only. The
launcher rejects login-shell mode before it starts either shell: interactive
login Bash does not load a supplied `--rcfile`, so a Bash wrapper cannot be the
enforcement point. A later nested login shell is an unmanaged bypass. Shell-
specific private wrapper startup files inherit the validated presentation
environment, original `HOME`, and (for Zsh) original `ZDOTDIR`, reproduce the
ordinary non-login interactive startup graph in system/user order exactly once,
then remove conflicting `codex`/`opencode` aliases/functions and install exact
WSNav functions. Observable environment, options, aliases, functions, and
prompt readiness match an ordinary disposable baseline except bounded wrapper
state and intentional interception. WSNav never parses or persists RC
contents. Startup abort, wrapper replacement, and ambiguous startup contexts
leave onboarding unavailable with guidance. Provider grammar
is closed and adapter/version-contract validated: only fresh native TUI shapes
promote; broker-owned cwd/profile, resume/session, attach/server,
host/port/endpoint, and equivalent identity flags refuse before reservation.
Explicitly enumerated provider-owned non-session commands such as `--help`,
`--version`, and `login` may run directly as explicitly unmanaged commands;
their effects remain provider-owned. Other shapes refuse with bounded
guidance. Safe native options are admitted only when proven.

The broker detects the exact containing non-bare Git worktree root from the
shell's current cwd and registers it atomically with the new Workstream. A
linked worktree remains its own ProjectLocation; WSNav never normalizes it to a
primary checkout, creates or switches worktrees, or retargets a Workstream
when the provider later works elsewhere. Only this broker-time check creates
ProjectLocation/launch authority; arbitrary cwd history is not persisted in the
host registry. `n` remains the fast path from a selected managed Workstream:
another independent blank session with the same provider at that exact stored
root. A different provider or directory begins through the provisional shell;
`f` remains a conversation Fork. New provider or Location creation is
broker-only; public `new-workstream` remains source-based parity for contextual
`n`, inherits the exact source provider/Location, rejects provider/path
overrides, and cannot accept source-less arbitrary creation.

Passive process detection, hook-only adoption, pane-text inference, and
provider launches that bypass the broker remain unmanaged.

The dormant account-shell control adapter now has no routed CLI but composes
both provider-specific final handoffs under the retained lease. Before either
provider preparation it resolves the canonical native executable and commits
only its bounded device/inode identity with `provider_preparation`; paths and
argv remain transient. Post-exec proof compares that durable identity with
`/proc/<pid>/exe`, never a later ambient `PATH` resolution. Codex then commits
`provider_exec_started` and can classify only a direct `execve` error as known
absence. OpenCode records an external-effect fence in the same callback
immediately before its temporary private server can `POST /session`, binds only
the exact returned blank session, then records `provider_exec_started`. Any
failure after ownership consumption that is not Codex's direct-exec known
absence becomes `recovery_required`; OpenCode never retries or invents a
replacement session. A dormant presentation-owned reconciliation adapter can
then compare the adopted pane's `/proc/<pid>/exe` identity against that durable
record without provider I/O or process control. These adapters remain dormant
and have exercised only disposable unit seams, not a user shell, provider
installation, or live Runtime.

[Spike 0019](evidence/spikes/0019-brokered-onboarding-shell.md) validates a
single-phase controlled-function-plus-`exec` candidate in a synthetic
private-tmux harness. [Spike
0021](evidence/spikes/0021-d17-two-phase-handshake.md) validates the narrow D17
prepare-token-helper topology, synthetic closed grammar, exact claim
comparison, one-shot semantics, and shell-identity-preserving provider exec
across Bash/Zsh and Codex/OpenCode routes. It does not validate the account
startup wrapper, schema-14 ownership, races/recovery, or real provider effects.
[Spike 0022](evidence/spikes/0022-d17-account-shell-wrapper.md) separately
validates the controlled non-login wrapper and records Bash's required launcher
login preflight. [Spike 0023](evidence/spikes/0023-d17-provisional-lock.md)
separately validates the isolated schema-14 stable-lock lifecycle. Neither
probe validates the required cross-actor onboarding races or provider effects.
[Spike 0024](evidence/spikes/0024-d17-provider-grammar.md) separately pins the
conservative fresh-TUI parser for Codex `0.150.0` and OpenCode `1.18.23`; it
does not validate helper/broker integration or provider effects.
[Spike 0025](evidence/spikes/0025-d17-provisional-ownership.md) separately
validates the serialized marker-to-owned-runtime winner model and action fence;
concurrent implementation, recovery, and provider effects remain unproven.
[Spike 0026](evidence/spikes/0026-d17-provider-effect-journal.md) separately
validates synthetic Codex no-effect and OpenCode known/ambiguous POST journal
ordering; it does not validate the real state/helper/provider integration.
The separate observer-ancestry revalidation passed on Codex `0.150.0`, while a
real brokered Codex launch remains a D17 gate. D17.0 remains the first
implementation gate for the complete handshake, grammar, wrapper, ownership,
and recovery contract.
OpenCode activation requires the D17-specific staged observer proof in
design.md: it may not become attachable merely because the pane executable was
proven. The retained blank-session endpoint/session handle is bound before the
final exec, then a post-exec controller must record the unchanged native
PID/birth, establish the exact detached observer, and commit final activation
only after the observer is Ready. That work is not yet user-facing.
[Spike 0020](evidence/spikes/0020-opencode-1.18.23-revalidation.md) revalidates
the OpenCode fresh-session/provider lifecycle assumptions on `1.18.23`; it does
not replace the required D17 broker implementation and Bash/Zsh acceptance.
The detailed target contract is in [design.md](design.md).

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
resume, and observation. This paragraph records the D8 creation UI that shipped;
D17 supersedes its chooser and Projects-registration path with explicit
shell-command provider choice and brokered Git-root registration. Cross-provider
work remains an independent New Workstream with an empty conversation, never
Fork, migration, or automatic context transfer.

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

At the time of this version-bound study, tmux `3.7b` was current in Arch
`extra`, the AUR `tmux-git` package was stale, and the tested upstream master
did not contain a fix. D16 acceptance later ran on tmux `3.7c`, but did not
rerun this cursor-fidelity study or infer a fix from the newer version alone.
WSNav therefore keeps its best-available private-server configuration. Revisit
this note when a candidate `#5419` fix is available; the instrument's
`nested_motion_not_amplified` and `nested_bytes_not_amplified` assertions are
the objective confirmation gate.

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

Historical status note (2026-08-18): D0 through D15 were complete and V1 was
a source-installed operator beta. The current D16 status is at the top of this
roadmap.

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
| D3 | Local and SSH hosts through one protocol | Complete (historical; retired by D16) |
| D4 | Independent and conversation-forked Workstreams | Complete |
| D5 | Recovery, combined acceptance, and V1 closure | Complete |
| D5.1 | Operational closure for recovery, release diagnostics, and bounded I/O | Complete |
| D5.2 | Correctness closure for release, identity, recovery, and presentation | Complete |
| D6 | Source-installed operator-beta closure | Complete |
| D6.1 | Repository identity and cross-host Project grouping polish | Complete (historical; retired by D16) |
| D6.2 | Navigator shortcut-reference polish | Complete |
| D6.3 | Cross-host activity ordering polish | Complete (historical; retired by D16) |
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
| D12 | Presentation-scoped ephemeral Workstream shell | Complete (2026-08-17) |
| D13 | Initial native-agent geometry convergence | Complete (2026-08-18) |
| D14 | Private tmux copy-mode scroll convergence | Complete (2026-08-18) |
| D15 | Fluid local Workstream switching | Complete |
| D16 | Host-local product simplification | Complete (2026-08-20) |
| D17 | Shell-first managed-session onboarding | Planned |

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

## D3 - SSH hosts (historical; retired by D16)

Extend the accepted local semantics across pre-registered hosts. This was the
former cross-host control surface; D16 preserves its evidence but retires its
WSNav-managed SSH behavior from the current contract.

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

## D5 - Recovery and V1 acceptance (historical; cross-host surface retired by D16)

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

The remote release probe, SSH protocol, and unreachable-host diagnostics in
this historical checkpoint are superseded by D16's host-local control
boundary; their recorded acceptance remains unchanged.

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

Remote project labels, local/remote parity, and SSH attachment references in
this historical checkpoint remain evidence only; D16 supersedes them with
host-local Project roots and operator-composed SSH.

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

## D6 - Source-installed operator-beta closure (historical; SSH smoke retired by D16)

Implementation status: complete. Present-tense documentation, the explicit
source-installed distribution posture, exact-candidate local/SSH release
parity, clean navigator shutdown, and bounded native operator smoke passed. See
the [D6 operator-beta acceptance](evidence/acceptance/d6-operator-beta.md).

Close the implemented V1 as an operator-ready beta without adding another
workflow or changing the approved ownership boundaries.

The local/SSH release parity and operator smoke in this historical entry are
retired by D16. The source-installed posture and host-local acceptance gates
remain applicable.

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

## D6.1 - Repository identity and cross-host Project grouping polish (historical; retired by D16)

Implementation status: historical completion; its cross-host association is
superseded by D16. Credential-free origin fingerprinting, safe origin labels,
and exact unambiguous matching remain active only for locations on the current
execution host. Linked-worktree input was normalized to the primary project
root rather than retained as a separate workstream cwd; the development schema
migration was superseded by the explicit host-state reset boundary.

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

## D6.3 - Cross-host activity ordering polish (historical; retired by D16)

Implementation status: historical completion, superseded by D16. The former
combined navigator projection ordered known local and remote activity
timestamps newest first, then used stable identity fallbacks without changing
host or provider authority.

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

## D7 - Navigator workflow and lifecycle management (historical; remote host surface retired by D16)

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

The local/SSH and host-inventory behavior described in this historical D7
summary remains evidence of the former surface. D16 supersedes it with one
current-host Hosts setup page and host-local Projects; ordinary SSH composition
is no longer a D7 or current WSNav operation.

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

## D8 - Multi-provider Workstreams (historical SSH acceptance; provider contract retained)

Implementation status: D8.0 completed on 2026-08-05, D8.1 completed on
2026-08-06, and D8.2 completed on 2026-08-07 after its corrective cleanup,
crash-guardian, deterministic lifecycle, and hardened real
local/loopback-SSH acceptance passed. The installed OpenCode release reported
`1.18.11`; compatibility remains contract-based rather than version-gated. The
[multi-provider design](design.md#multi-provider-and-multi-agent-design) is
authoritative for the shared provider boundary and privacy invariants.

The SSH and local/loopback-SSH acceptance named in this historical D8 record
remains truthful evidence of the former cross-host implementation. D16 retires
that transport and keeps the provider boundary host-local; any later D8
references to remote adapters or remote acceptance are historical, not current
scope.

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

Any remote-monitor, SSH, or cross-host references in these historical D9
records describe the pre-D16 implementation and are not current architecture;
D16 retires those surfaces while retaining the behavior-neutral local evidence.

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

## D12 - Presentation-scoped ephemeral Workstream shell (historical; remote shell surface retired by D16)

Status: Complete on 2026-08-17. Implementation, automated validation,
explicitly authorized local plus real-SSH machine acceptance, and local
normal-environment visual confirmation completed on 2026-08-15. Corrected
normal-environment SSH launch, installed automatic cross-Workstream cleanup,
and SSH completed-output preservation were confirmed on 2026-08-17.

This checkpoint adds quick, unmanaged terminal access beside the currently
attached provider without turning WSNav into a general-purpose terminal
multiplexer. The normal presentation still begins with the Navigator and one
native provider pane. One private tmux chord may temporarily add one ordinary
shell below the provider for short host-local work such as inspecting or
manually operating Git. The shell is never a Workstream or durable session.

The remote-shell and SSH-helper portions of this completed checkpoint are
historical evidence. D16 retains only the host-local utility-shell semantics;
ordinary SSH composition is outside WSNav.

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
- keep a live shell fixed to its launch host and ProjectLocation until it exits
  or the user selects a different Workstream. A cross-Workstream switch closes
  the exact utility without prompting, verifies the two-pane layout, and only
  then replaces the provider attachment; failure refuses the switch rather than
  leaving mixed contexts. Reselecting or reconnecting the same exact host and
  Workstream keeps its shell. This deliberately preserves a zero-friction core
  switching workflow and treats the utility as short-lived scratch space for
  the currently displayed Workstream. Client detach may leave it alive only as
  part of the same disposable presentation; there is no durable restoration
  after presentation loss.

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

Completion evidence (through 2026-08-17):

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
  failed-launch cleanup, local cwd, remote launch-context non-retargeting,
  same-context retention, automatic cross-Workstream cleanup, ambiguous
  topology refusal, ABI preflight, literal nested-prefix delivery, and
  ordinary-tmux non-interference; and
- the current uninterrupted `scripts/check` run passes 421 library tests, 14
  presentation recovery tests, 5 local transport tests, package and dependency
  policy, formatting and lint, every disposable acceptance harness, plus
  staged and unstaged whitespace checks. This includes the automatic
  cross-Workstream cleanup, same-context retention, and fail-closed extra-window
  regressions, but does not substitute for operator-visible terminal
  confirmation below; and
- the explicitly authorized acceptance harness subsequently passed every
  non-visual local and real-loopback-SSH assertion with complete cleanup and an
  unchanged ordinary-tmux inventory. It retained no terminal content and
  therefore initially left the local and SSH completed-output visual assertions
  to the operator; and
- a normal-environment remote follow-up found that the SSH helper rejected an
  exact live Runtime while its durable lifecycle was still `Starting`, then
  would have required a `SHELL` variable that ordinary SSH command environments
  need not provide. The corrected path retains exact process preflight,
  accepts that pending-hook state only after it, resolves the effective
  account's login shell through the account database, passed a bounded
  content-free live probe with complete cleanup, and was confirmed usable by
  the operator on the installed remote build; and
- the operator confirmed the installed local presentation automatically closes
  the shell when selecting a different Workstream, restores the two-pane
  layout, does not restore the old shell on return, and leaves the shell intact
  on same-Workstream reselection; and
- the operator confirmed the completed provider result on the installed SSH
  Workstream remains visible and unchanged through utility-shell use and that
  the provider remains interactive afterward. No terminal content was retained
  from either confirmation, closing the final D12 gate.

Non-goals and hard boundaries:

- no second shell, `Ctrl+b %` layout, persistent terminal entity, shell list,
  restoration record, per-Workstream parked-shell surface, shell title/rename
  model, or remembered shell policy;
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
- switching to a different Workstream while a shell is open closes only the
  exact utility, restores two-pane geometry before provider replacement, adds
  no confirmation step, and never exposes a provider/shell context mismatch;
  reselecting the same exact host and Workstream leaves its shell unchanged;
- explicit operator-gated local and real SSH confirmation verifies hostname,
  cwd, a harmless Git inspection, provider interactivity and completed output,
  shell exit cleanup, detach/reattach behavior, and non-interference with an
  ordinary tmux session. Evidence contains no provider or shell capture; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

## D13 - Initial native-agent geometry convergence

Status: Complete on 2026-08-18.

Daily use falsified the D10.2 assumption that converging only the outer
Navigator width was sufficient for a correct initial native provider render.
Each provider Runtime starts detached at tmux's default `80x24` dimensions. A
disposable nested reproduction showed the inner client surface present first
at 47 columns and then settle at 117 columns when a `150x40` presentation
client attached. Native agent TUIs can remain visually stale after that
transition until another operator-generated terminal resize forces a redraw.

Scope:

- immediately before attaching a terminal to the private presentation, read
  that terminal's dimensions and pre-size only the exact owned presentation
  window;
- immediately before the local or SSH host attachment enters a private
  Runtime, read the provider attachment PTY dimensions and pre-size only the
  exact owned Runtime window;
- return both windows to tmux's `window-size latest` policy before attaching so
  later native terminal resizes retain their existing behavior; and
- share the Runtime handshake across local and remote attachment paths while
  keeping geometry transient and bounded to the current attach attempt.

Non-goals and hard boundaries:

- no terminal-size polling, timing delay, synthetic resize pulse, provider
  process signal, manager-originated provider input, or provider-content
  capture;
- no default or ordinary tmux access, persisted geometry, presentation or
  Runtime topology change, layout preference, provider command change, or
  compatibility behavior; and
- no lifecycle, state, schema, protocol, control ABI, SSH argument, terminal
  capability, or completed-output retention change.

Exit gate:

- deterministic command coverage proves both handshakes target only the exact
  owned window with direct `resize-window` arguments and restore
  `window-size latest`, including bounded rejection behavior;
- a disposable nested tmux regression starts from detached default dimensions,
  attaches through a larger final PTY, and proves both private windows have the
  final geometry without a second or manual resize;
- existing presentation recovery, local and SSH attachment, native terminal
  capability, D12 topology, and ordinary-tmux non-interference coverage remain
  green; and
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass.

Completion evidence (2026-08-18):

- the presentation and Runtime attach boundaries now pre-size only their exact
  owned window and restore `window-size latest` before the native attach. The
  shared Runtime handshake covers local, SSH, and native trust-review paths;
- eight deterministic geometry tests cover exact arguments, both bounded tmux
  rejection points, and zero-dimension rejection without invoking tmux;
- a disposable real nested-tmux regression proves detached `80x24` private
  presentation and Runtime windows converge to the final PTY dimensions during
  the first attach, without a second resize; and
- all 430 library tests, 14 presentation-recovery tests, 5 transport tests,
  package and dependency-policy checks, formatting, lint, the disposable
  acceptance harnesses, and `git diff --check` pass through `scripts/check`.
  No live provider or SSH acceptance was performed, and no provider content,
  persistent geometry, state, schema, protocol, or ABI surface was added.

## D14 - Private tmux copy-mode scroll convergence

Status: Complete on 2026-08-18.

Daily use exposed a second mismatch between ordinary tmux muscle memory and
WSNav's intentionally hermetic private servers. The operator's ordinary tmux
profile binds wheel movement in both copy-mode tables to one line, but neither
private WSNav profile sources that executable configuration. A disposable tmux
3.7c comparison proved the resulting gap: the private-server baseline retained
tmux's `-N 5` bindings while the ordinary profile produced `-N 1`.

Scope:

- give both private tmux layers one shared, fixed interaction fragment that
  binds wheel up and down in `copy-mode` and `copy-mode-vi` to exactly one line;
- preserve the presentation root table's existing alternate-screen,
  pane-in-mode, and mouse-flag forwarding decisions so provider-owned native
  scrolling remains unchanged;
- apply the same four fixed bindings when a new private Runtime or presentation
  is created; and
- before every local, SSH, or native trust-review Runtime attachment,
  idempotently reconcile those bindings through only the exact owned Runtime
  socket so already-running providers converge without restart.

Non-goals and hard boundaries:

- no source, parse, execution, or query of the user's ordinary tmux
  configuration or default tmux server, and no general tmux inheritance or
  public scroll preference;
- no plugin, hook, shell command, environment, status, title, prefix, root
  table, terminal capability, history limit, clipboard, default shell, or
  topology import;
- no provider-owned alternate-screen scrolling change, provider restart,
  provider input or content capture, pane inspection, or ordinary tmux
  mutation; and
- no lifecycle, geometry, state, schema, protocol, control ABI, SSH argument,
  dependency, provider command, or completed-output retention change.

Exit gate:

- complete generated-configuration tests prove both private layers consume the
  same exact four one-line copy-mode bindings while all topology-specific and
  terminal-capability bytes retain their existing meaning;
- deterministic Runtime attach tests prove the exact socket and argument
  vectors, idempotent success, bounded rejection before attach, and no command
  on invalid geometry;
- a disposable real-tmux regression proves both private layers report one-line
  bindings in both copy-mode tables, including convergence of an existing
  Runtime that began with tmux's five-line baseline;
- presentation recovery proves the D12 root and prefix allowlists remain exact
  and alternate-screen wheel events retain native forwarding; and
- all existing attachment and non-interference coverage, one uninterrupted
  `scripts/check` run, and staged and unstaged `git diff --check` pass.

Completion evidence (2026-08-18):

- one typed four-binding profile now generates the startup configuration for
  both private tmux layers and the exact argument vectors used to reconcile an
  existing Runtime, so those paths cannot choose different scroll counts;
- Runtime attachment validates terminal geometry before mutation, applies the
  idempotent profile through only the exact owned socket, fails before geometry
  or native attach on a bounded binding rejection, then retains the D13 window
  handshake unchanged;
- a disposable real nested-tmux regression proves both private layers expose
  `-N 1` for wheel up and down in both copy-mode tables and that an existing
  Runtime deliberately returned to `-N 5` converges without restarting its
  provider process; and
- all 432 library tests, 14 presentation-recovery tests, 5 transport tests,
  package and dependency-policy checks, formatting, lint, disposable
  acceptance harnesses, and `git diff --check` pass through `scripts/check`.
  The D12 root and prefix allowlists remain byte-exact outside the added shared
  copy-mode fragment. No live provider or SSH acceptance was performed; the
  real-tmux regression used local tmux 3.7c.

## D15 - Fluid local Workstream switching

Status: Complete.

Fast switching among independent live Workstreams is a fundamental daily-use
path. Switching must replace only the outer presentation's temporary tmux
client: the invisible Workstream's private tmux server, Runtime generation,
provider process, and native session remain live until an explicit Park or an
independent provider failure. Returning to that Workstream attaches to the same
Runtime instead of restarting or resuming its provider session.

A disposable local study on 2026-08-18 isolated a fixed control-process cost.
`Command::output` completed `/bin/true` in 0.432 ms p50, while WSNav's bounded
runner completed it in 24.053 ms p50 because successful short-lived children
can wait for a fixed 20 ms completion poll before the existing mandatory
process-group cleanup. A 40-switch private-presentation sample measured the
outer attachment at 291.177 ms p50, post-attachment focus at 48.473 ms p50,
and the complete synchronous outer switch at 339.585 ms p50 and 344.509 ms
p95. The replacement helper then performs ten additional bounded Runtime tmux
commands. Their roughly 240 ms contribution and the resulting 580-650 ms
end-to-visible range are code-derived estimates, not an end-to-end
measurement; D15 must establish that baseline before claiming improvement.

Implementation order:

1. Add a disposable warm local A-to-B-to-A timing study that separates
   Navigator focus, outer presentation replacement, provider focus, exact
   helper-start observation, and proof that the new nested tmux client attached.
   Record aggregate timings and process metadata only; never read provider pane
   content.
2. Replace the bounded runner's fixed short-child completion quantum on Linux
   with an event-driven process file descriptor (`pidfd`) wait. Retain a
   bounded adaptive polling fallback when that facility is unavailable,
   without changing the existing deadline, output-drain, process-group cleanup,
   or error contract.
3. Rerun the same study. Only if the measured target is still missed, remove
   redundant bounded-command work. Any empty-process-group shortcut must use a
   non-mutating proof; a live or uncertain group must retain the existing exact
   PGID-plus-session authority, cleanup, and diagnostics.
4. Run the full repository and disposable acceptance gates, then perform one
   operator-gated installed-build confirmation without parking or rotating the
   sampled Runtimes.

Scope:

- measure warm local switching from activation through metadata-only proof of
  attachment to the destination Runtime, using the same disposable fixture
  topology and one unchanged long-lived Runtime pair per measured build;
- make successful finite control-child completion responsive without weakening
  bounded output, timeouts, child and process-group authority, descendant
  cleanup, or error precedence;
- preserve exact local attachment preflight, D13 geometry convergence, D14
  copy-mode convergence, presentation-shell exclusion, and attachment status;
- prove that a provider continues working while invisible and that switching
  away and back retains its exact Runtime ID, tmux generation, provider PID,
  process-birth token, and native-session binding; and
- keep performance thresholds in the controlled disposable study rather than
  adding wall-clock-sensitive assertions to ordinary shared CI runners.

Non-goals and hard boundaries:

- no shared Runtime tmux server, ordinary tmux access, cross-server
  `switch-client` workaround, replacement terminal emulator, provider-pane
  proxy, or change to the one-private-server-per-live-Runtime invariant;
- no provider restart, native Resume, Runtime rotation, Park, lifecycle
  transition, background suspension, or completed-output loss during an
  ordinary switch;
- no skipped or optimistic attachment identity check, detached background
  mutation after reporting success, weakened output cap or deadline, or
  reduced child/process-group cleanup authority;
- no provider input, pane capture, prompt, response, transcript, terminal
  payload, raw provider payload, or credential storage or diagnostics;
- no Navigator layout, card, focus-policy, keyboard, mouse, animation, or
  broader UI/UX redesign; and
- no schema, protocol, control ABI, provider command, SSH transport, remote
  latency target, or selective user tmux configuration import.

Exit gate:

- the baseline and candidate use the same disposable fixture topology and
  sampling procedure, keep each build's Runtime pair unchanged for the entire
  run, and report p50 and p95 for each named phase; the candidate's warm local
  activation-to-attached p95 is at most 150 ms and at least three times faster
  than its immediately preceding baseline, while the synchronous outer phase
  is at most 100 ms p95;
- deterministic bounded-process coverage proves event-driven successful-child
  completion, timeout cleanup, oversized-output draining, successful-parent
  descendant cleanup, cleanup-error precedence, and the unsupported-pidfd
  fallback without leaking a child, process group, pipe reader, or unbounded
  diagnostic;
- disposable switching coverage proves that both Workstreams retain their
  exact Runtime and provider identities, the invisible provider fixture keeps
  making progress, only the two initial provider starts plus the expected
  attachment helpers are observed, and switching back exposes the same
  still-running provider without capturing its terminal;
- exact attachment tests retain the current ambiguity failures, geometry and
  scroll reconciliation, utility-shell retirement, completed-output retention,
  and local/SSH argument boundaries;
- one uninterrupted `scripts/check` run plus staged and unstaged
  `git diff --check` pass; and
- installed local acceptance meets the same aggregate latency target closely
  enough to feel immediate to the operator, while sanitized process metadata
  proves that no sampled live Runtime was restarted, resumed, parked, or
  rotated. Remote switching remains outside the performance claim.

Implementation evidence (2026-08-18):

- the controlled study now follows the local production sequence: focus the
  Navigator, replace the outer provider helper, focus the provider pane,
  observe the exact helper attempt, and prove the destination private-runtime
  client through bounded tmux metadata. It records no pane or provider content;
- the final harness was compiled once against the pre-D15 `403490a` runner in
  a detached worktree and once against the candidate. Each 40-sample A-to-B-to-A
  run used the same topology and retained its own exact two-Runtime pair. This
  avoids adding a benchmark-only wait-strategy switch to production code while
  keeping the compared workload identical;
- the fixed-wait baseline measured 587.496/617.035 ms p50/p95 from activation
  to attached and 292.875/294.688 ms for outer replacement. The candidate
  measured 57.565/64.651 ms and 17.002/18.262 ms respectively: 9.5 times faster
  end to end at p95 and well inside both absolute gates;
- Linux finite-child completion now uses pidfd readiness with `Child::try_wait`
  as authority and an adaptive 1-20 ms fallback for every pidfd open or poll
  failure. After reaping, a non-mutating signal-zero probe skips the process
  table scan only when the captured process group is absent; any live or
  uncertain group still requires the existing captured PGID-plus-session proof
  before `SIGKILL`, and probe uncertainty remains a fail-closed cleanup error;
- all sampled switches retained both exact Runtime IDs, tmux generations,
  sessions, provider PIDs, process-birth tokens, and fixture native-session
  bindings. The invisible provider kept advancing, the fixture saw exactly two
  provider starts and the expected 41 attachment helpers, and no terminal
  capture was performed; and
- one uninterrupted `scripts/check` run passed all 437 library tests, the D15
  percentile test, 14 presentation-recovery tests, 5 transport tests, package
  verification, dependency policy, formatting, lint, and disposable acceptance
  suites. Staged and unstaged `git diff --check` also pass; and
- after the operator closed only the outer presentation, the candidate was
  installed at the canonical local path without parking a Runtime. All three
  pre-install provider PID/process-birth pairs, Runtime IDs, tmux generations,
  and private sessions remained exact after installation, after the switching
  sample, and after the acceptance presentation closed; and
- an output-discarding installed presentation completed 20 warm local A-to-B
  and B-to-A switches in 62.862/69.075 ms p50/p95 (54.630 ms minimum, 71.396 ms
  maximum). The in-process observer timed navigator key dispatch through exact
  destination-client metadata, rejected an unexpected Workstream, verified the
  source had no client, and never captured or sent input to a provider pane.
  The final destination was the same recorded provider process, the third
  provider remained live and untouched, and the headless presentation exited
  cleanly without a start, Resume, Park, or Runtime rotation.

## D16 - Host-local simplification

Status: Complete on 2026-08-26. The implementation, disposable repository gate,
and explicitly authorized live local and ordinary-SSH-entered-host acceptance
passed. D0-D15 remain complete historical checkpoints, and V1 remains a
source-installed operator beta.

Fresh current-tree evidence includes strict formatting and Clippy, 426 passing
tests with one controlled D15 timing study ignored, package verification,
Cargo Deny policy, shell/Python/fixture checks, the disposable D12 presentation
harness, D16 retired-source/CLI acceptance, and staged plus unstaged diff
checks. The five focused D16 integration suites contribute 96 of those passing
tests. This current total includes the post-acceptance source correction that
removed the redundant page banner, restored the established semantic colors
and activity ages, then the card refinement that made Project headers
name-only, removed repeated host labels, moved relative age beside provider,
and gave the second line to lifecycle plus native thread name. It also includes
the follow-up regression correction for wrapped guidance/status prose, parked
marker precedence, Running-attachment replacement, and exact dead-helper-pane
respawn after Park, plus the compact Projects rows, width-packed footer, and
structured colored help reference. The latest controller correction attaches
an exact owned Runtime while its durable status is still `Starting`, avoiding
a resume
bootstrap cycle in which native SessionStart observation waited on the terminal
attachment that the controller withheld. It also permits exactly one passive
revision refresh when that lifecycle observation races attachment preflight,
but only while the same Workstream ID and Runtime ID remain attachable; a
second change or identity rotation still refuses. These changes affect the
passive activity-time projection, navigator rendering/controller, and exact
presentation-helper replacement; no schema, durable state model, provider
Runtime ownership, or native provider interaction contract changed.
An additional Park convergence correction atomically resolves an exact
recovery-required, already-absent Runtime to `Workstream=parked` plus
`Runtime=stopped`, preventing a deliberate second Park from creating the
non-recoverable `recovery_required + stopped` pair while retaining native
session binding and sticky attention.

The provider-choice correction keeps onboarding-capable Codex states visible
for New instead of filtering them out before the contextual guide can run.
Exact `setup_required`, `update_required`, and `trust_review_required` states
may be selected, then reuse the existing consent, native review, readiness, and
revision boundary before any creation or provider launch. Hard-unavailable
Codex states remain excluded. When `n` starts from one provider and only a
different provider is selectable, the one-entry chooser now requires explicit
confirmation instead of silently substituting that provider. This changes no
schema, provider adapter, Runtime authority, or native interaction contract.
The corrected binary was then atomically installed and the operator confirmed
the bounded clean-bootstrap path: provider choice remained visible, Codex
selection entered contextual setup and native review, and the resulting fresh
state reported one Codex Workstream and Runtime with observer readiness
`Ready`. No provider content was inspected. This operational observation is
not a repeat of the formal live gate because the operator retained the active
Runtime.

The explicitly authorized live gate predates these source corrections and passed
on the local machine and an ordinary-SSH-entered host, including confirmed
schema-12-to-13 cutover, same-Runtime Codex continuity and reattachment across
SSH disconnect/reconnect, atomic candidate installation, and complete
acceptance cleanup. The first visual correction was later installed locally
for operator inspection, but the later corrected sources were not live-tested
in that gate. The sanitized, content-free record retains the accepted executable
hash and its exact 399-test repository evidence in [D16 host-local
simplification acceptance](evidence/acceptance/d16-host-local.md).

### Motivation and decision

The former product let one Navigator register SSH hosts, exchange bounded
snapshots and actions with them, and attach through SSH. That made host-local
runtime authority look distributed and required client-side cross-host Project
grouping, polling, cache, and unreachable-state behavior. D16 retires that
surface. One wsnav instance controls only the machine on which it executes.

Multi-host use is composition: terminal A runs host A's local `wsnav`; terminal
B is an operator-established ordinary SSH session to host B and runs host B's
`wsnav`. After SSH establishment, all WSNav control work for switching,
contextual observer readiness, provider lifecycle, attention, recovery, and
private-tmux attachment on host B is local to that instance. Terminal rendering
and input still cross SSH and retain ordinary network latency. WSNav does not
register SSH hosts, open or manage SSH, poll remote snapshots, issue remote
mutations, attach through SSH, bridge remote utility shells, or present a
unified multi-host catalog or attention view. Independent hosts require no
cross-host WSNav release or protocol parity.

### Scope

- Make the product thesis and active architecture host-local while preserving
  native provider UI, completed output, provider lifecycle, project-root-only
  behavior, D12 shell semantics, D13 geometry, D14 scrolling, and D15 fast
  local switching.
- Preserve one current-host `HostRegistry`, host-local ProjectLocations and
  Workstreams, Runtime generations, provider bindings, attention, private tmux
  servers, and provider-owned history as the authority for that host.
- Rebuild same-host Project grouping from authoritative ProjectLocations,
  generating fresh Project IDs and a stable label-source location at the D16
  cutover. Preserve that source's repository display name as the primary label
  and the safe credential-free Git origin as separate secondary context and
  grouping evidence. Origin metadata never associates separate hosts or grants
  action authority. Remove Project-level hide/forget; Workstream archive and
  restore remain the one visibility mechanism.
- Reduce the active navigator to direct Workstreams, Projects, and Archived
  pages. Workstreams always shows active Workstreams grouped by Project;
  remove Recent, `ViewMode`, left/right view cycling, and Hosts. Archived is a
  separate Project-grouped restore page. Page selection stays process-local and
  is not persisted. This bounded structural reduction belongs to D16 so the
  retired host/catalog UI is not carried into a later redesign.
- Keep Projects as the host-local ProjectLocation surface. `n` on an exact
  selected Location starts an independent Workstream, so a Project whose every
  Workstream is archived remains usable. Empty active Workstreams routes to
  registered Location selection before registration. `n` from a selected
  active Workstream retains the exact same-Location fast path. Projects also
  owns the typed registration-browser root action and explicit metadata
  refresh; it has no Project hide/forget/remove action.
- Detect Codex observer readiness read-only. Only an observer-dependent request
  invokes a contextual guide, which captures the intent and revisions, asks
  explicit consent before exact owned-profile creation or update, opens native
  trust review without granting trust, and continues only after exact readiness
  and revision revalidation. Decline cancels without mutation. Foreign,
  modified, disabled, ambiguous, or live-Runtime-blocked integration changes
  fail closed while existing Runtime attachment remains available. There is no
  setup/settings page or public normal-workflow setup/update command; exact
  removal remains an exceptional documented cleanup flow. Non-interactive CLI
  actions never prepare or review a profile; they return typed readiness
  guidance to interactive `wsnav`.
- Derive the bounded current-host display from a valid sanitized operating-
  system hostname, then `host-<HostId8>`, using the first eight lowercase
  hexadecimal UUID digits. There is no configured label, `HostPresentation`
  state, settings action, or label persistence. The derived display remains
  bounded application metadata but is not repeated in ordinary navigator cards
  or pages; the structurally host-local instance and its containing terminal or
  SSH window already supply machine context. The renderer preserves the
  established Project/provider/lifecycle/age color roles, with selection
  changing only the row background; chromeless direct attach is exempt.
- Remove remote-only JSON protocol and SSH transport, hidden remote endpoints,
  remote release/capability handshakes, remote polling/cache/backoff/
  unreachable state, host registration UI/CLI, cross-host grouping, and other
  dead surfaces once implementation reaches this checkpoint. Host schema 13 is
  the explicit consolidated state boundary; the client schema and catalog are
  removed rather than revised.
- Make current-host scope structural rather than a retained one-host variant:
  remove client host/transport types, remote Navigator variants, host aliases,
  host-plus-Workstream selection keys, and host fields from attachment status
  or action DTOs. `HostId` remains once as registry identity and display-label
  fallback evidence, never as an operator-selected action target.
- Replace the generic local client/protocol boundary with one typed in-process
  application facade used by both navigator and public CLI:
  `snapshot`, `apply`, and `attach`. Delete `HostClient`, `LocalEndpoint`,
  framed JSON, hidden local control endpoints, and the generic control ABI.
  Replace cursor-paged snapshot framing with one deterministically ordered,
  hard-bounded in-memory snapshot and a typed over-limit refusal.
  Retain finite subprocess boundaries only for inherently external tmux, Git,
  provider-helper, observer/hook, launch-barrier, and terminal-attachment work.

### Breaking state-transition contract

- D16 removes `ClientCatalog`, its schema, and its database. It reads no legacy
  client row and provides no importer, client-state compatibility reader, dual
  write, or automatic rollback. The exact retired files are `client.sqlite`,
  `client.sqlite-wal`, and `client.sqlite-shm` under the selected state root.
- An existing state root requires one launcher-owned, pre-presentation
  confirmation from an ordinary interactive `wsnav` launch before current host
  state or a retained presentation is opened. The confirmation names the
  discarded remote registrations, aliases, Project IDs/grouping, hidden state,
  cached capabilities, executable paths, preferences, and exact legacy
  presentation, and separately names the preserved authoritative host/runtime
  state.
  Declining performs no mutation.
  Hooks, observer sidecars, hidden helpers, and scripting commands cannot
  confirm or start cutover.
- Cutover enumerates the selected root's exact owned presentation sockets and
  verifies session topology, pane roles, navigator PID/birth/executable,
  attached-client count, and auxiliary-pane state. Ambiguous or foreign state
  fails closed. An attached client, utility shell, or observer-review surface
  blocks mutation and may be entered only through a no-state-open legacy drain
  attachment so the operator can finish it and quit the old presentation.
  After confirmation, cutover first takes an exclusive state-root transition
  lease, repeats the proof, and may then retire one detached ordinary legacy
  presentation. Its exact navigator and presentation artifacts must disappear
  before the proof is repeated again; exact dead owned presentation artifacts
  may be removed under the lease, while malformed or foreign artifacts refuse.
  Runtime tmux servers and provider processes are never targeted. D16
  control/open paths honor the lease; observer-transition bypasses it and
  serializes only through SQLite so provider evidence is not blocked behind
  presentation retirement.
- Each new D16 presentation creates its bounded private `ownership.json`, fixed
  configuration, and mode-`0700` directory before starting tmux, then records
  the exact owned socket identity. Reopen and close revalidate those identities
  plus the bounded artifact allowlist. Close unlinks only exact owned files and
  socket and removes the empty directory; it never recursively deletes a
  presentation tree, and foreign, malformed, symlinked, changed, or unknown
  artifacts remain untouched.
- Live provider Runtimes and their exact observer sidecars may remain active.
  A narrow observer-transition state handle accepts exactly host schema 12 or
  13, never creates or migrates, reads no client file, and exposes only the
  unchanged lifecycle/binding/attention surface. Within the unchanged
  three-second native Codex timeout, D16 bounds payload/provenance/App Server
  work to 1.75 seconds, reserves 750 milliseconds for monotonic
  `BUSY`/`LOCKED` database retry, 250 milliseconds for bounded failure
  recording, and the final 250 milliseconds for outer scheduling and exit. If
  an exact authorized event cannot commit, Codex hooks and OpenCode observers
  atomically retain one exact
  `run/<RuntimeId>/observer-degraded/<sha256-generation>` marker containing only
  typed identity and a closed reason. Snapshot/action paths derive only the
  current generation's filename; snapshots show `unknown` and
  observer-dependent actions remain unavailable until exact reconciliation or
  Runtime retirement. Migration prebuilds and validates its Project plan,
  revalidates under the writer transaction, and rolls back if writer
  acquisition plus work reaches 500 milliseconds. Other D16 entrypoints fail
  with typed `cutover required`.
- A pre-D16 OpenCode observer has no such retry contract. After confirmation
  and before client-file deletion, cutover enumerates all live OpenCode
  Runtimes in opaque RuntimeId order and corroborates each helper's recorded
  PID/birth, executable identity, generation, endpoint, and status. Exact D16
  observers are revalidated in place; ambiguous identity refuses before any
  signal. For each pre-D16 helper, cutover establishes a D16 standby SSE stream
  with only bounded parsed in-memory buffering and durably journals the old and
  standby PID/birth/executable identities, expected handle revision, and phase
  before signaling. It freezes the exact old helper, rechecks its stopped
  identity and handle, compare-and-swaps only the observer handle, activates
  the standby only from that committed assignment, and then terminates the
  frozen old helper. Repeated status is idempotent and settled evidence
  deduplicates by generation, session, and provider message ID. The exact
  private `d16-observer-handover.json` and
  `d16-observer-handover.json.tmp` paths support restore before the swap. After
  exact buffer replay, the standby durably records the bounded private
  `d16-observer-handover.ack` through its sole `.ack.tmp` path; the launcher
  requires that post-activation proof and exact process recheck before old
  cleanup, including after its original readiness pipe is lost. Malformed or
  changed evidence signals nothing. Failure refuses before deletion; provider
  process, Runtime, terminal, session, and completed output remain unchanged.
  After all handovers and journal cleanup, remove only the three exact client
  paths, sync the state directory, and transactionally migrate host schema 12
  to 13 using only `host.sqlite`.
- Preserve HostIdentity, provider/observer integration state,
  ProjectLocations, the typed Project browser root, all Workstream provider and
  activity fields, independent-creation requests, Runtime generations,
  OpenCode Runtime handles, provider bindings, attention, compound operations,
  private tmux servers, and native provider history. Generate fresh Projects
  and label-source locations from current-host ProjectLocations. Create no
  HostPresentation, configured label, page/view preference, or Project hidden
  state.
- Narrow production schema support to exact boundaries: current-only opens
  schema 13, confirmed cutover migrates exact schema 12, observer-transition
  accepts schema 12 or 13, and fresh-create writes schema 13. Schema 0 through
  11, malformed or missing evidence, and every other unsupported version return
  typed state-recovery/reset-required without mutation; a future version fails
  closed. Remove production incremental pre-12 migrations and behavioral
  migration tests, retaining only an exact schema-12 fixture for 12-to-13
  coverage.
- Group exact credential-free origin fingerprints only within this host;
  missing, ambiguous, and local-path identities remain separate. The primary
  Project label source initially comes from the lowest-LocationId group member.
  Joins and merges preserve a surviving Project's source; only departure of the
  exact source selects the lowest remaining member, and display changes update
  the Project label only for that source. Matching Projects survive merges,
  sole-member unmatched changes retain their Project ID, multi-member unmatched
  changes split, and orphan Projects are deleted. Missing evidence never
  dissolves an association. The safe origin label remains separate secondary
  context.
- Gather changed repository evidence only through the explicit Projects-page
  metadata refresh. It reads the selected Project's locations in bounded
  LocationId order outside SQLite, then revalidates all captured Project and
  Location revisions and applies the complete result transactionally. One
  failed or unsafe inspection or stale revision makes the whole action
  non-mutating; a successful no-fingerprint observation preserves association.
  No snapshot, redraw, attachment, or Workstream switch performs Git inspection.
- The reset is restartable. Missing retired client files are success, partial
  removal is retried, and a failed host migration leaves `host.sqlite` at
  schema 12. Ordinary navigation remains blocked until schema 13 is complete.
  No Start, Resume, Park, provider signal, Runtime rotation, or provider-input
  action is part of cutover.
- Fresh-create accepts only an absent state root or an existing private
  directory that is empty or contains exactly the private, current-user-owned,
  unlocked `transition.lock` regular file. It acquires that lease and repeats
  the allowlist check before database creation. Host SQLite main/WAL/SHM, any
  client file, `run/`, `presentation/`, either observer-handover journal path,
  a malformed, foreign, non-regular, or locked lease, or any unknown entry
  yields typed state-recovery-required without adoption or cleanup. A missing
  host database beside any such artifact never creates a new HostIdentity.
- Downgrade is unsupported after cutover. Operators who require rollback must
  park or stop managed Runtimes, exit WSNav, and create a verified offline copy
  of the complete state root before confirmation, then restore that complete
  copy before running a pre-D16 binary. This optional procedure is outside D16;
  D16 creates no backup and reverse-synchronizes no Project, label-source, or
  preference state.
- An outer SSH disconnect may end or detach that host's disposable presentation
  but must not stop, park, rotate, or restart its private Runtime/provider.
  Reconnect to the host, rerun `wsnav`, and reattach.

### Non-goals

D16 includes only the structural UI reduction needed to remove obsolete host
and view concepts. It does not add a broader visual/daily-use redesign, SSH
launcher, terminal/window manager integration, cross-host aggregator, daemon,
state synchronization, session transfer, remote install/update, or provider
behavior change. It does not change native provider workflow, add
compatibility behavior for the retired remote protocol, or authorize arbitrary
existing-session adoption. Broader UI/UX work remains a possible D17 after the
smaller D16 surface is implemented and accepted.

### Implementation slices

Slices 1 through 8 are complete. The list retains the authority-preserving
delivery order and the separation between disposable validation and explicitly
authorized live local and SSH-entered-host acceptance.

1. **Documentation-only design pass.** Reconcile `design.md` and
   `roadmap.md`, mark D0-D15 SSH evidence historical, settle same-host Project
   semantics, reduced navigation and dormant-Project creation, contextual
   observer onboarding, derived host display, direct local facade, and the
   explicit client-state reset and host-schema boundary.
2. **State foundation.** Implement schema 13 Project state,
   deterministic Project reconstruction, complete preservation of the typed
   schema-12 host inventory, the merge/split/stable-label-source state machine,
   explicit current-only, observer-transition, fresh-create, and cutover modes,
   the exact fresh-root classifier, bounded observer database deadlines, the
   privacy-bounded observer-degraded marker, and the restartable OpenCode
   standby-handover journal. Remove production pre-12 migration paths behind
   focused schema fixtures. This slice does not change `HostRegistry::open`,
   activate schema 13, touch a real state root, delete a client file, signal a
   process, or alter current product behavior.
3. **Cutover orchestration.** Implement exact presentation discovery,
   confirmation inputs, transition-lease ownership, repeated controller proof,
   legacy-presentation drain/retirement planning, and OpenCode sidecar handover
   execution behind disposable tests without routing the current product into
   it or mutating an ordinary state root.
4. **Local application surface.** Add the typed in-process
   `snapshot`/`apply`/`attach` facade, the simplified Workstreams/Projects/
   Archived controller and renderer, exact Location-based creation paths,
   derived host display, and contextual Codex readiness guide behind typed
   seams and focused tests. Preserve D12 shell behavior and D13-D15 local
   interaction invariants.
5. **Atomic activation.** In one cohesive cutover slice, add the
   pre-presentation confirmation and safe drain/retirement, acquire and repeat
   the transition proofs, complete any required observer handover, remove the
   three exact client files, activate schema 13/current-only open, and route
   navigator plus public CLI through the direct local facade and reduced page
   model. No intermediate build may mix schema 13 or the new navigator with the
   old client/catalog path.
6. **Deep deletion.** Remove client catalog/schema, host registration, remote
   monitor and picker, unreachable state, cross-host selectors, SSH attachment
   and utility-shell bridging, remote observer barriers, `By host`, Recent,
   `ViewMode`, Hosts, HostPresentation/configurable-label state, generic
   `HostClient`/`LocalEndpoint`, framed JSON, hidden local/remote endpoints,
   snapshot cursor/page/replay machinery, release/capability handshakes,
   generic control ABI, public normal-workflow observer setup/update commands,
   and their tests and fixtures. Keep only typed local domain/application DTOs,
   exceptional exact cleanup/diagnostics, and bounded finite helpers required
   for inherently external operations.
7. **Operator documentation.** Update README, operator guidance, command help,
   and transition notes to explain ordinary SSH composition, the confirmed
   clean break, unsupported downgrade, contextual observer guidance and exact
   cleanup, the three-page navigation model, and the host-local reconnect/
   disconnect boundary.
8. **Full acceptance.** Run the complete repository and content/privacy gates,
   then perform operator-gated local and SSH-entered-host acceptance with
   sanitized, content-free evidence and complete cleanup.

Each implementation slice must be independently reviewable and run its focused
tests plus diff checks. The final slice runs the complete repository gate; a
large retirement diff is not accepted as one undifferentiated change.

### Exit gate

D16 closes only when all of the following are true:

- host-local operation passes on the local machine and on a machine reached by
  ordinary operator SSH, with WSNav running locally on each host;
- on the SSH-entered host, starting wsnav over its existing registry reattaches
  the same recorded live provider process and Runtime generation without a
  Start, Resume, Park, or Runtime rotation;
- every host preserves its HostIdentity, integrations, ProjectLocations,
  Project browser root, Workstream provider/activity/lifecycle fields,
  independent-creation requests, Runtime generations, OpenCode Runtime
  handles, provider identities and bindings, attention, compound operations,
  native history, and private tmux isolation;
  no Workstream or provider session is copied or migrated between hosts;
- no WSNav action spawns, manages, polls, or mutates SSH/network control for
  another host, and remote-only surfaces are removed rather than left as
  compatibility behavior;
- an outer SSH disconnect may end/detach presentation but never stops, parks,
  rotates, or restarts the host Runtime/provider; reconnect and rerun reattach
  successfully;
- an existing state root changes only after the exact pre-presentation
  interactive confirmation;
  declining, hook/sidecar invocation, hidden helpers, and scripting commands
  perform no cutover mutation;
- cutover verifies every owned presentation's exact topology, navigator
  PID/birth/executable, client count, and auxiliary state; ambiguous, foreign,
  attached, utility-shell, and observer-review cases mutate nothing, while the
  bounded drain attachment opens no host state. One confirmed detached ordinary
  legacy presentation is retired without targeting a Runtime only after an
  exclusive transition lease and repeated proof; the proofs repeat again after
  retirement and before any client-file deletion or migration;
- a live provider retains the same process, Runtime generation, native session,
  terminal, and completed output across accepted cutover. Newly spawned Codex
  hooks use the schema-12/13 observer-transition handle, while a pre-D16
  OpenCode sidecar is replaced only through the deterministic, journaled
  standby handover. Every live helper's PID/birth, executable, generation,
  endpoint, and status are corroborated first, and exact D16 observers remain
  in place. The standby proves its stream but cannot mutate before assignment;
  the exact old helper is frozen and reverified before the observer-handle
  compare-and-swap, and only that frozen PID/birth is terminated. Inability to
  establish every handover refuses before client-file deletion. The standby's
  exact durable post-activation acknowledgement proves buffer replay and
  survives loss of the original launcher pipe before old-helper cleanup;
- the unchanged native Codex profile keeps its three-second timeout, D16 hook
  preparation and App Server work finish within 1.75 seconds, the next 750
  milliseconds are reserved for monotonic bounded retry of SQLite `BUSY` and
  `LOCKED`, the next 250 milliseconds are reserved for failure recording, and
  250 milliseconds remain as outer margin. Migration prebuilds its Project plan
  and limits writer acquisition plus transactional work to 500 milliseconds,
  rolling back to schema 12 at the deadline. Contention tests hold a competing
  writer and prove an exact lifecycle or attention event commits wholly before
  or after migration or leaves only the bounded generation-scoped degraded
  marker. That marker exposes no event or error payload, makes snapshots
  `unknown`, blocks observer-dependent actions, and emits no provider-pane
  output. OpenCode handover tests prove bounded parsed buffering, idempotent
  status, exact settled-message deduplication, and exact restore or completion
  after interruption at every journal phase; malformed or changed evidence
  signals nothing;
- `ClientCatalog`, `src/state/client.rs`, client schema/migrations, importer,
  client-state compatibility reader, dual write, and client-catalog behavioral
  tests are absent from compiled source and active tests; only the three exact
  legacy filenames remain in cutover cleanup and historical
  documents/evidence, and those files are removed without being read, imported,
  backed up, or renamed;
- host schema 12 migrates transactionally to 13 using only `host.sqlite`;
  interruption or partial client-file removal is retryable, a failed
  transaction leaves host schema 12 intact, and ordinary navigation stays
  blocked until schema 13 is complete. Current-only also rejects schema 13 when
  any exact legacy client file has reappeared; confirmed cleanup removes only
  those files and performs no redundant migration;
- production contains no incremental schema-0-through-11 migration path:
  current-only accepts exact schema 13, confirmed cutover exact schema 12,
  observer-transition schema 12 or 13, and fresh-create writes 13. Older,
  malformed, missing, future, or otherwise unsupported schema evidence fails
  closed with the typed recovery/reset or future-state result and no mutation;
  only an exact schema-12 migration fixture remains active;
- ordinary state open never upgrades schema 12 implicitly: Navigator,
  actions, helpers, and scripts receive the typed cutover-required result;
  observer-transition accepts only schema 12 or 13 and exposes only unchanged
  lifecycle/binding/attention operations without creating, migrating, or
  reading client state; fresh-create accepts only an absent state root or an
  existing private directory that is empty or contains exactly the private,
  current-user-owned, unlocked `transition.lock` regular file. It acquires that
  lease and repeats the complete allowlist check before creation. Any host
  main/WAL/SHM, legacy client file, Runtime or presentation directory,
  observer-handover journal or activation-ack path,
  locked/malformed/foreign/non-regular lease,
  or unknown artifact returns typed state recovery required without adoption,
  signaling, cleanup, or a new HostIdentity; and only the confirmed interactive
  transition entrypoint may invoke migration and reset;
- fresh Project IDs and stable label-source locations are rebuilt from
  current-host ProjectLocations with no Project hidden field or imported hidden
  state. Exact fingerprints group only within this host; missing, ambiguous,
  and local-path identities remain separate. Joins and merges preserve the
  surviving Project's label source, display changes update its label only when
  they belong to that source, and only departure of the exact source selects
  the lowest remaining LocationId. The safe origin label remains separate
  secondary context; later missing evidence preserves an existing association
  while positive exact evidence follows the bounded same-host
  merge/update/split rules, matching targets survive, orphan Projects disappear,
  and a missing, foreign, or non-member label source fails closed;
- repository reinspection occurs only through the explicit revision-checked,
  network-free Projects action; one stale revision or failed/unsafe inspection
  makes the complete action non-mutating, successful no-fingerprint evidence
  preserves association, and ordinary snapshot, redraw, attachment, and D15
  switching paths perform no Git subprocess or Project mutation;
- no generic preferences, host label, or `HostPresentation` row is imported or
  created, and D16 creates no automatic backup or downgrade path. A
  pre-D16 binary fails closed on host schema 13 unless the operator first
  restores a verified offline complete-state-root backup created outside D16;
- the derived current-host display obeys validated hostname, then
  `host-<HostId8>` precedence; the validator enforces the 64-scalar,
  single-line, no-control-or-format-character bound. It remains bounded
  application metadata, but no ordinary navigator card or page repeats it;
  the containing terminal or SSH window supplies machine context. Direct
  attach remains intentionally chromeless and exempt; no page or CLI mutates
  that display;
- Project-group headers render only the accented Project name, with no
  disclosure glyph or Location/active/archived counts. Each Workstream is a
  minimal two-line tree child: provider plus right-aligned relative age on the
  first line, then lifecycle marker plus the native thread name using the full
  remaining second line. The host label is absent, and a missing native name
  falls back to the stable short Workstream ID without a `Workstream` prefix;
- the Projects page flattens a one-Location Project into one exact selectable
  row instead of repeating the Project and its label source. Multi-Location
  Projects retain a display-only header and minimal child tree; Location rows
  omit the generic prefix, internal label marker, inventory counts, and inline
  action repetition. Footer hints pack whole key/action pairs over the lines
  required by the pane, while page help uses concise colored key/action columns
  instead of generic prose wrapping;
- Parked lifecycle always renders `p` even while sticky attention remains
  unseen. Status and guidance prose wraps by terminal cell width, with status
  height and list/mouse geometry derived from the same wrapped layout;
- a Running provider attachment is replaceable for ordinary A-to-B switching,
  while AwaitRuntime Start remains serialized. An exact provider helper pane
  left dead by detach or Park may be respawned in place only after the owned
  single-window roles, live navigator, and bounded utility cleanup revalidate;
  ambiguous or otherwise dead topology still refuses before provider mutation.
  AwaitRuntime attaches as soon as the exact owned Runtime exists, including
  while its durable lifecycle is `Starting`, only when no D17 onboarding
  operation remains unfinished and `provider_exec_proven` has committed;
  a D17 Runtime in any earlier post-commit phase is not attachable merely from
  record/process identity—the originating presentation may retain its existing
  pane, but that is not a new attachment. Native SessionStart observation
  confirms lifecycle progress but is not a terminal-attachment prerequisite
  after that D17 proof;
- Workstreams, Projects, and Archived are the only ordinary navigator pages.
  Workstreams and Archived are Project-grouped by descending durable
  `last_activity_sequence` with opaque Project/Workstream ID tie-breakers;
  Archived is a direct restore page, restore returns to and selects in
  Workstreams without provider launch, and no page/view choice is persisted.
  Recent, `ViewMode`, left/right cycling, and Hosts are absent;
- `n` from an active Workstream retains its exact ProjectLocation fast path;
  `n` from an exact Projects Location starts there even when the Project is
  dormant; and an empty active page routes to existing Location selection
  before registration. Project headers never supply ambiguous action authority,
  and Projects owns the typed browser-root and revision-checked metadata actions;
- provider choice is derived from current host capability evidence. Codex
  remains selectable when exact observer setup, update, or native trust review
  can make the requested action ready; hard-unavailable states remain absent.
  A sole different provider reached from an existing Workstream requires
  explicit chooser confirmation, while same-provider and Location/registration
  sole-candidate paths remain immediate. Selection never silently substitutes
  a provider and onboarding completes before creation or launch;
- navigator startup detects observer readiness without mutation. An
  observer-dependent Codex request captures its exact intent/revisions and
  offers explicit consent before owned profile creation or update, then opens
  native trust review without granting trust and continues only after exact
  readiness plus revision revalidation. Decline mutates nothing; incomplete
  review remains accurately trust-pending; stale intent, foreign, modified,
  disabled, ambiguous, or live-Runtime-blocked changes refuse without
  retargeting or blocking existing Runtime attachment. No setup/settings page
  or public normal setup/update command remains, and exceptional exact removal
  preserves foreign/modified state and refuses with live Runtimes. A
  non-interactive CLI request returns typed readiness guidance without profile
  mutation or native review;
- `By host`, remote host selectors, remote registration, remote unreachable
  states, and cross-host grouping are absent from active TUI and CLI behavior;
- Project hide, unhide, forget, remove, and `x` are absent from active TUI and
  CLI behavior; Workstream archive/restore is the sole visibility mechanism;
- client host/transport types, remote Navigator variants, host aliases,
  host-plus-Workstream selection keys, and host fields in attachment/action
  DTOs are absent; one registry `HostId` remains only as identity and display
  fallback evidence;
- `src/app/remote.rs`, `src/remote.rs`, SSH command construction, release
  probes, remote protocol/version types, and remote-only tests/fixtures are
  deleted. `HostClient`, `LocalEndpoint`, framed JSON, hidden local control
  endpoints, and the generic control ABI are also absent; navigator and public
  CLI call the typed in-process `snapshot`/`apply`/`attach` facade. Snapshot
  cursor/page/replay state is absent; the facade returns one deterministically
  ordered hard-bounded projection or a typed over-limit refusal. Any
  surviving finite local-child DTOs belong only to inherently external work
  and carry no retired transport compatibility shape;
- provider UI, management-traffic, completed-output, and project-root-only
  boundaries remain intact;
- bounded metadata/privacy rules, fail-closed identity/effects, one private
  tmux server per live Runtime, and D12-D15 local lifecycle/interaction
  invariants remain green;
- `scripts/check`, staged and unstaged `git diff --check`, and focused
  D16 tests pass; and
- any live real-host acceptance is explicitly operator-gated, sanitized, and
  content-free, with complete cleanup. Historical D3-D15 evidence is not
  rewritten as current validation.

## D17 - Shell-first managed-session onboarding

Status: in progress. The product contract is approved; D17.0 studies, the
D17.2 test-only ownership/private-runtime model, dormant presentation-private
marker-backed materialization/evidence storage, marker-to-state prepare/consume
broker, typed no-provider-effect helper fences, and dormant D17.3 grammar,
command-classification, onboarding-phase, capability-journal, reservation, and
ownership-consumption foundations have begun. No D17 user-facing behavior or
provider launch path is
implemented. The explicit schema-13-to-14 migration, stable provisional-lease,
schema-14-only open, lease acquisition, reservation, ownership consumption,
and marker-backed materialization/evidence storage foundations remain dormant,
and no D17 acceptance is complete.

### Goal

Make onboarding feel like opening an ordinary shell while retaining exact
managed-session authority. Remove the separate Projects workflow, let native
provider commands own provider and launch-option choice, and preserve WSNav's
private Runtime, recovery, privacy, and completed-output guarantees.

### Product contract

- Workstreams and Archived are the only ordinary pages.
- Workstreams always contains exactly one pinned provisional shell card outside
  Project groups. At presentation creation, WSNav captures, validates, and
  canonicalizes the invocation cwd as a private seed cwd. The card is
  materialized lazily with
  exactly one opaque candidate `RuntimeId`; its provisional tmux directory,
  socket, configuration, and session use the existing final full-UUID
  `RuntimePaths` fields (directory, socket, configuration, and session).
  Candidate ID, exact `RuntimePaths` fields (directory,
  socket, configuration, and session), seed, and ownership evidence live only
  in the presentation-private marker, not a registry Runtime or Workstream row.
  Before creating those artifacts, materialization proves the candidate ID and
  all four path fields are absent and unused; it never adopts pre-existing
  artifacts. A marker-backed candidate is excluded from ordinary registry
  inventory, probe, park, remove, and recovery discovery/action until durable
  adoption; only the exact presentation marker plus the stable host-private
  `provisional.lock` lease may manage it. Markerless/registryless, foreign, or
  collision artifacts remain untouched, and a clean replacement allocates a
  fresh candidate RuntimeId. The pinned card is a derived singleton with no
  durable card row, and each materialization mints a fresh opaque
  `slot_generation` bound by the marker, capability, and onboarding journal.
  Every clean newly materialized shell starts at the seed; detach/reattach
  preserves a live shell's actual cwd, and a new presentation captures its own
  seed. Missing, deleted, unsafe, or ambiguous seed evidence makes onboarding
  unavailable with guidance and never falls back.
- The provisional account shell supports Bash and Zsh interactive non-login
  shells only. The launcher rejects login mode before it starts either shell:
  interactive login Bash does not load a supplied `--rcfile`, so the wrapper
  cannot be the enforcement point. A later nested login shell is an unmanaged
  bypass. Shell-specific private wrapper startup files inherit the validated
  presentation environment, original `HOME`, and (for Zsh) original `ZDOTDIR`,
  reproduce the ordinary non-login interactive startup graph in system/user
  order exactly once, remove conflicting `codex`/`opencode` aliases/functions,
  and install exact WSNav-owned functions. Observable environment, options,
  aliases, functions, and prompt readiness match an ordinary disposable
  baseline except bounded wrapper state and intentional interception. WSNav
  never parses or persists RC contents; startup abort, wrapper replacement, or
  ambiguity leave onboarding unavailable.
- For a promotable fresh interactive native TUI shape, the exact function
  invokes a bounded prepare broker as a child over presentation-private
  non-terminal control I/O. One exact stable host-private `provisional.lock`
  artifact, distinct from D16's schema-cutover `transition.lock`, is shared by
  materialization, close/loss cleanup, broker preparation/token issuance,
  helper consume, singleton reconciliation, and marker cleanup. It is
  operational state rather than a Runtime/card/Workstream row or
  presentation-private storage. Schema-14 host-operational lease metadata
  stores only a planned `lease_generation`, install phase `pending` or `ready`,
  and expected lock device/inode once ready. The schema/HostId transaction
  commits schema-14 ownership and pending metadata first; schema-13 code/path
  never creates or recognizes `provisional.lock`.
  In `pending`, startup lazily creates an absent mode-`0600` current-owner
  regular file with create-new/no-follow, writes bounded file contents, fsyncs
  the file, then fsyncs the containing state-root directory before finalizing
  `ready` with expected device/inode; an exact file
  left by a crash may be validated/locked and finalized. Pending foreign or
  mismatched evidence fails closed. In `ready`, missing, replaced, or
  device/inode-mismatched evidence fails closed and is never recreated. The
  file contains only bounded format version, HostId, and
  `lease_generation`; it contains no cwd, command, argv, provider/user content,
  or provider payload. A pre-schema-14 artifact is unexpected/ambiguous,
  remains untouched, and is never adopted or deleted; this ordering does not
  claim cross-store atomicity. Normal operation never unlinks/recreates it.
  Every participant opens it no-follow/CLOEXEC, acquires one nonblocking
  exclusive kernel lock, retains the FD, and revalidates canonical root/path
  plus FD device/inode before mutation. Crash releases only the kernel lock;
  restart reacquires the same artifact and reconciles marker/journal. The FD
  never crosses provider exec, and busy/timeout never creates a second lock or
  proceeds unlocked. Marker, capability, and journal bind both
  `lease_generation` and `slot_generation`; each participant revalidates the
marker, onboarding journal, presentation revision, and registry generation while
  holding it.
  The broker transactionally generates/reserves the durable Runtime generation
  and graph for the exact candidate ID and unchanged full-UUID `RuntimePaths`
  fields (directory, socket, configuration, and session), marks the handoff
  issued, and returns only an exact one-shot opaque launch capability, never a
  provider command or argv. Its claims bind the request/operation,
  presentation/slot, candidate ID and exact `RuntimePaths` fields (directory,
  socket, configuration, and session), fixed provider, exact shell
  cwd/root/Location, reserved generation, captured revisions, shell
  PID/birth/process group, grammar-approved argv digest, and short monotonic
  expiry. The function then `exec`s one hidden WSNav launch helper with that
  capability and the original bounded argv; the helper reacquires
  `provisional.lock` and, while holding it, revalidates every bound
  marker/process/cwd/path/revision/
  token claim, including the candidate ID and all four `RuntimePaths` fields.
  Only on successful revalidation does it atomically compare-and-consume the
  capability and commit durable `Runtime-owned` authority for the candidate; a
  mismatch does not advance ownership. It then, still under the lock and before
  releasing it, revokes/removes presentation cleanup authority; durable
  transition precedes marker cleanup, and only afterward prepares provider
  effects, constructs provider argv internally, and
  `exec`s the provider. Persisted state keeps only
  bounded token identifier/verifier/phase and claim references or digests; the
  live token, argv, shell command line, environment, terminal capture, and
  provider payload are never persisted. The shell leader PID, birth token, and
  process group survive into the provider. Promotion adopts the same private
  tmux server/pane/process lineage without rename, rehome, or replacement.
- Each presentation derives one pinned provisional card, but the shared host
  `provisional.lock` and classifier permit at most one unregistered materialized
  candidate server across all presentations. A valid marker/artifact belonging
  to another presentation is busy/owned, not unknown or adoptable; that card
  remains visible but unavailable until its slot promotes or conclusively
  cleans. Under the lock, a bounded classifier cross-checks the exact marker
  and unfinished operations against registered Runtime IDs and bounded
  `run/runtime-*` names only to detect conflicts; it never adopts or deletes
  unknown artifacts. Missing/changed marker evidence with an unregistered
  Runtime-shaped artifact, multiple candidates, or ambiguous journal/path/
  process evidence blocks every fresh materialization and leaves artifacts
  untouched; no new UUID may evade ambiguity. A clean replacement requires
  exact prior absence or conclusive cleanup and gets a fresh slot generation
  and candidate ID.
- The helper's successful claim revalidation and atomic capability consume
  commit durable Runtime ownership but do not yet activate ordinary attachment
  or action authority. The request-keyed operation enters
  `runtime_owned_launching` (no provider effect), provider-specific
  preparation/external-effect phases, and `provider_exec_started` immediately
  before `execve`; terminal outcomes are `provider_exec_proven`, known-absent
  exec failure, or `recovery-required`/`unknown`. Until full proof, attachment
  and action authority for that unproven Runtime remains fenced: its originating
  presentation may retain its existing Runtime pane or detach through ordinary
  card switching, but no new attachment to that Runtime is allowed. Selecting
  or materializing the fresh derived singleton card attaches only its separate
  provisional server under `provisional.lock` and grants no authority over the
  unproven Runtime. Every ordinary Park/Resume/Fork/contextual
  `n`/`new-workstream`, archive, Rename, recovery/start retry, and cleanup
  action for that Runtime refuses or waits with bounded
  `onboarding-in-progress` guidance. Passive snapshot/probe may
  render `starting`/`onboarding` and reconcile, but must not adopt helper or
  preparation processes, mark the Runtime lost, signal it, or expose ordinary
  action authority. Once terminal `recovery-required`, only exact recovery or
  explicit Park rules apply. A host-local reconciler invoked by passive snapshot/action
  preflight or restart recovery performs no provider effect; only after full
  operation/revision, RuntimeId/generation and exact `RuntimePaths` fields
  (directory, socket, configuration, and session), tmux pane/session,
  PID/birth/PGID/session, and expected-executable proof does it atomically
  commit `provider_exec_proven` and activate ordinary authority. An exact
  helper-recorded `execve` error proves only absence of the final provider TUI
  exec; attempt-only graph rollback is allowed only when provider-specific
  journal evidence also conclusively proves no prior external effect or
  binding. A crash after `provider_exec_started` without proof is ambiguous and
  never rollback authority. A known OpenCode blank-session POST or binding
  remains on the same Runtime/Workstream/binding for exact recovery/resume and
  is never rolled back or posted again; a possible POST effect is
  `recovery-required`. A terminal known-absent result is not itself action
  authority: when provider-specific evidence proves no effect or binding,
  guarded rollback atomically ends onboarding and leaves the derived singleton
  card available but unmaterialized; with a known OpenCode binding, atomic
  resolution instead ends onboarding in the exact stopped/recovery state where
  only binding-preserving Resume/recovery or explicit Park is allowed. No
  ordinary action follows directly from exec-error evidence, and terminal
  reconciliation cannot leave the operation fenced indefinitely.
- Durable Runtime ownership consumes the old slot generation and derives one
  fresh unmaterialized provisional card. Any rollback is lease-held,
  revision/slot-generation guarded, and targets only the old operation/Runtime/
  slot; it never resets a newly materialized shell, targets a newer marker, or
  creates a second card. Existing fresh marker/card state remains unchanged.
- Process names, pane content, hooks, provider inventory, and commands that
  bypass the functions are never adoption authority. A bypassed provider is an
  unmanaged shell process.
- At broker invocation, bounded read-only Git discovery from the shell's exact
  current cwd resolves the containing non-bare worktree root. Only this
  broker-time check creates ProjectLocation/launch authority. A linked
  worktree stays its own ProjectLocation. Missing, unsafe, ambiguous, or
  non-Git seed/current cwd evidence never falls back; failure leaves the shell
  interactive and creates no durable record. Arbitrary cwd history is not
  persisted in the host registry.
- Provider kind is the explicit command the user types. The provider adapter's
  closed, version/contract-validated grammar admits only fresh native TUI
  shapes. Broker-owned cwd, profile, resume/session, attach/server,
  host/port/endpoint, and equivalent identity flags fail before reservation;
  they are never stripped or reinterpreted. Explicitly enumerated provider-owned
  non-session commands such as `--help`, `--version`, and `login` may run
  directly as explicitly unmanaged commands; their effects remain provider-
  owned. Other shapes refuse with bounded guidance. Provider authentication,
  model, effort, role,
  permissions, and first input stay native; safe native arguments pass only
  when proven compatible, without an invented live-version flag list. Any
  secret-bearing argument or value is outside the promotable grammar.
- Public `new-workstream` is source-based parity for contextual `n`: it
  inherits the exact source provider and ProjectLocation, rejects provider/path
  overrides, and has no source-less arbitrary creation form.
- `n` on a selected managed Workstream starts an independent blank session with
  the same provider at the same exact registered Location. `n` does nothing
  special on the provisional card. A different provider or directory uses the
  shell; `f` remains same-provider conversation Fork.
- Once promoted, a Workstream remains pinned to its launch Location. WSNav does
  not create, switch, remove, discover, or follow later provider worktrees and
  never retargets from provider cwd changes.
- Normal tmux detach and reattach to the same owned presentation preserves the
  exact provisional shell, actual cwd, and pending state and never creates a
  duplicate. A prepared reservation alone does not revoke provisional cleanup:
  before the helper successfully revalidates every bound
  marker/process/cwd/path/revision/token claim and atomically consumes the
  capability while committing durable `Runtime-owned` authority, confirmed
  close/loss may win only under the shared lease by atomically canceling/
  revoking an unconsumed capability, proving pre-effect absence, rolling back
  attempt-only rows, and then cleaning exact provisional artifacts. After that
  exact helper commit, presentation cleanup never signals that pane, process,
  or server. Durable transition precedes marker cleanup, and ambiguous
  cross-store windows remain in the onboarding journal for recovery. Outer SSH
  detach follows these same rules. Shell exit and conclusive pre-effect
  failure after the exact helper ownership commit are resolved by onboarding
  recovery, not presentation cleanup. Ambiguous ownership leaves evidence untouched, fails closed, and
  blocks duplicate-shell creation. Managed Runtimes are preserved in every
  presentation-loss path.
- Card and server state key off Runtime ownership rather than provider success.
  Before that exact helper commit the selected card remains the exact shell; once
  ownership commits it becomes the managed Workstream and a fresh
  unmaterialized card appears even when binding is not ready. OpenCode
  pre-creates its exact blank root session; any possible `POST /session` effect
  leaves the same server Runtime-owned and the card visibly
  `recovery-required`, even without a native TUI. Presentation cleanup cannot
  touch it and recovery never issues a second POST. A conclusive pre-effect
  failure after the exact helper commit is classified by onboarding recovery;
  it rolls back attempt-only graph state only when provider-specific evidence
  proves no effect or binding, leaving the derived singleton card available but
  unmaterialized. A blank Codex TUI remains a managed `starting` row until its
  first authoritative SessionStart, without session-list or title inference.
- D12's optional utility shell below an attached managed provider remains a
  distinct, short-lived current-Workstream tool. D17 does not make utility
  shells durable or permit multiple provisional shells.

### Durable-state contract

Host schema 13 migrates transactionally to schema 14. The migration removes
only `ProjectBrowserSettings` and preserves Projects, ProjectLocations,
Workstreams, Runtime generations, provider bindings, integrations, attention,
and unfinished operations. No state wipe is required. Fresh state writes
schema 14 directly; unsupported or ambiguous schema evidence fails closed.

The provisional shell has no registry row. Its marker owns exactly one fresh
`slot_generation`, one candidate RuntimeId, the exact final full-UUID
`RuntimePaths` fields (directory, socket, configuration, and session), seed cwd,
and bounded shell/server ownership evidence. Materialization alone references
no durable graph row; broker prepare may reserve the ProjectLocation,
Workstream, Runtime generation, and onboarding operation before the exact helper
ownership commit. The stable host-private `provisional.lock` is operational
state, distinct from D16's `transition.lock`, and contains only bounded format
version, HostId, and `lease_generation`. Schema-14 host-operational metadata
stores a planned `lease_generation`, install phase `pending` or `ready`, and
expected device/inode once ready; it is not a card/Runtime row. The schema/HostId
transaction commits schema-14 ownership and pending metadata first; schema-13
code/path never creates or recognizes the lock. In `pending`, startup creates an
absent mode-`0600` current-owner regular artifact with create-new/no-follow,
writes bounded file contents, fsyncs the file, then fsyncs the containing
state-root directory before finalizing `ready` with expected device/inode; an
exact crash-window file may be validated/locked and
finalized. Pending foreign/mismatched evidence fails closed. In `ready`, missing,
replaced, or device/inode-mismatched evidence fails closed and is never
recreated. A pre-schema-14 artifact is unexpected/ambiguous, remains untouched,
and is never adopted or deleted; no cross-store atomicity is claimed. All actors
use one retained no-follow/CLOEXEC nonblocking exclusive lock, and malformed,
unlinked/recreated, or busy evidence fails closed. Crash releases the kernel lock
without changing the file; restart retries `pending` or reacquires only the
expected `ready` artifact.
Marker, capability, and journal bind both `lease_generation` and
`slot_generation`.

Promotion acquires `provisional.lock` and creates one request-keyed
`CompoundOperation(kind=onboard)`, transactionally adopting that candidate ID
and unchanged `RuntimePaths` fields (directory, socket, configuration, and
session) while reserving the Project/Location, fixed-provider Workstream, and
Runtime generation. The `provisional.lock` lease and marker/revision checks are
held through the handoff-issued transition. A prepared reservation does not
revoke provisional cleanup: close/loss can cancel/revoke an unconsumed
capability, prove
pre-effect absence, roll back attempt-only rows, and then clean the
marker-backed artifacts. The helper instead reacquires the lock and, while
holding it, revalidates every bound marker/process/cwd/path/revision/token
claim. Only on successful revalidation does it atomically compare-and-consume
the capability and commit durable `Runtime-owned` authority for the candidate;
a mismatch does not advance ownership. It then, still under `provisional.lock`
and before releasing it, revokes/removes presentation cleanup authority, leaves
marker cleanup after that durable transition, and only afterward prepares provider
effects or executes the provider.

The same operation then records `runtime_owned_launching` (no provider effect),
provider-specific preparation/external-effect phases, and
`provider_exec_started` immediately before `execve`; terminal outcomes are
`provider_exec_proven`, known-absent exec failure, or
`recovery-required`/`unknown`. Runtime ownership alone does not activate
ordinary attachment or action authority for that unproven Runtime: until full
proof, its originating presentation may retain its existing pane or detach
through ordinary card switching, but no new attachment to that Runtime is
allowed. Selecting/materializing the fresh derived singleton card attaches only
its separate provisional server under `provisional.lock` and grants no authority
over the unproven Runtime. Park/Resume/Fork, contextual `n`/`new-workstream`,
archive, Rename, recovery/start retry, and cleanup for that Runtime refuse or
wait with bounded onboarding guidance. Passive snapshot/probe and restart recovery
may reconcile but perform no provider effect. A host-local
reconciler atomically commits `provider_exec_proven` only after full
operation/revision, RuntimeId/generation and exact `RuntimePaths` fields
(directory, socket, configuration, and session), tmux pane/session,
  PID/birth/PGID/session, and expected-executable proof. An exact helper-recorded
  `execve` error proves only absence of the final provider TUI exec; attempt-only
  graph rollback is allowed only when provider-specific journal evidence also
  proves no prior external effect or binding. An authoritative Codex hook may
  contribute only through that same identity/revision proof, and an OpenCode
  sidecar or server identity is never native-TUI exec proof. A known OpenCode
  blank-session POST or binding remains on the same Runtime/Workstream/binding
  for exact recovery/resume and is never rolled back or posted again; a possible
  POST effect is `recovery-required`. A terminal known-absent result is not
  itself action authority: when provider-specific evidence proves no effect or
  binding, guarded rollback atomically ends onboarding and leaves the derived
  singleton card available but unmaterialized; with a known OpenCode binding,
  atomic resolution instead ends onboarding in the exact stopped/recovery state
  where only binding-preserving Resume/recovery or explicit Park is allowed. No
  ordinary action follows directly from exec-error evidence, and terminal
  reconciliation cannot leave the operation fenced indefinitely. A crash after
  exec-start without proof is ambiguous and never rollback authority.

Each presentation derives one pinned provisional card with no durable card row,
but the shared host lease/classifier permits at most one unregistered materialized
candidate server across all presentations. A valid marker/artifact belonging to
another presentation is busy/owned, not unknown or adoptable; its card remains
visible but unavailable until that slot promotes or conclusively cleans. Under
the stable lease, the classifier may cross-check the exact marker and unfinished
operations against registered Runtime IDs and bounded `run/runtime-*` artifacts
only to detect conflicts; it never passively adopts or deletes unknown artifacts.
Missing/changed marker evidence with any unregistered
Runtime-shaped artifact, multiple candidates, or ambiguous journal/path/process
evidence blocks all fresh materialization and leaves artifacts untouched. A
fresh UUID cannot evade that ambiguity; replacement is permitted only after
exact prior absence or conclusive cleanup and receives a new slot generation
and candidate ID. Runtime ownership consumes the old slot generation and derives
one fresh unmaterialized card. A lease-held, revision/slot-generation-guarded
rollback targets only the old operation/Runtime/slot, leaves any newer marker or
card unchanged, and is idempotent across restart; it never creates a second
card or resets a newly materialized shell.

The one-shot capability binds the request/operation, presentation and
provisional slot, candidate RuntimeId and exact `RuntimePaths` fields
(directory, socket, configuration, and session), fixed provider, exact shell
cwd/root/Location, reserved Runtime generation, captured revisions, shell
PID/birth/process group, grammar-approved argv digest, and short monotonic
expiry. Its bounded phase records prepare, token issuance,
hidden-helper handoff/atomic consume, `runtime_owned_launching`,
provider-specific preparation/external-effect phases,
`provider_exec_started`, `provider_exec_proven`, known-absent exec failure,
and `recovery-required`/`unknown`. Persisted state keeps only a bounded token
identifier/verifier, claim references or digests, expiry, and phase; it never
stores the live token, original argv, shell command line, environment,
terminal bytes, or provider payload. Secret-bearing arguments are outside the
promotable grammar. Ambiguous cross-store crash windows remain in the journal
for onboarding recovery. A conclusive pre-effect failure after the exact helper
commit is rolled back and classified by onboarding recovery only when the
provider-specific journal proves no prior external effect or binding; an exact
`execve` error alone proves only final TUI exec absence. A possible post-effect
failure is recovery-required and cannot be retried as a clean launch; a known
OpenCode binding remains on the same Runtime/Workstream for exact recovery or
resume and never receives a second POST.

### Evidence basis and remaining falsification gates

- Spike 0019 proves only a single-phase controlled-function-plus-`exec`
  candidate. Spike 0021 validates the narrow synthetic
  prepare-token-helper-provider chain across Bash/Zsh and both provider routes:
direct prepare child, verifier-backed one-shot consume, every bound-claim
mutation and expiry/replay refusal, shell PID/birth/PGID/session preservation,
and lease-FD noninheritance. Spike 0022 separately validates account-shell
non-login baseline parity and its Bash login preflight, Spike 0023 separately
validates the isolated schema-14 stable-lock lifecycle, and Spike 0024 pins the
versioned fresh-TUI grammar. The probes do not prove their cross-actor
integration, cancellation/crash recovery, or native provider effects.
  The separate Codex `0.150.0` run revalidates observer ancestry only; real
  native Codex promotion, terminal behavior, and output retention remain exit
  gates.
- D17.0 is the first falsification gate: disposable Bash/Zsh evidence must
  validate or reject the two-phase handshake, one marker-backed candidate
  RuntimeId with final full-UUID `RuntimePaths` fields (directory, socket,
  configuration, and session), exact non-login wrapper startup and function
  precedence, bounded argv handling, provider grammar, signals, cancellation,
  crash gaps, and shell-leader PID/birth/process-group preservation before
  production onboarding work begins. It must prove the schema/HostId transaction
  commits schema-14 ownership and `pending` lease metadata before lock
  creation/recognition, that schema-13 code/path does neither, and that
  pending-before-file, file-before-ready, ready-steady-state, and crash/restart
  windows behave deterministically. A pre-schema-14 lock artifact is
  unexpected/ambiguous and remains untouched rather than adopted or deleted; no
  cross-store atomicity is assumed. It must prove candidate
  collision/foreign-artifact refusal and exclusion from ordinary registry
  inventory, probe, park, remove, and recovery paths until durable adoption;
  markerless/registryless artifacts remain untouched and a clean replacement
  uses a fresh candidate ID. It must exercise the exact stable host-private
  `provisional.lock` distinct from D16's `transition.lock`: schema-14
  fresh-root recognition, create-new/no-follow, mode-`0600` current-owner
  regular-file validation, pending absent-file creation and exact crash-window
  file finalization, ready missing/replacement/device-inode mismatch refusal,
  valid unlocked-leftover reuse, HostId/format/path/inode checks, one
  nonblocking exclusive lock held by every actor, holder crash/restart, busy
  timeout, symlink/replacement/unlink-recreate attempts, inode mismatch, and FD
  noninheritance across provider exec. It must cover
  `HOME`/Zsh `ZDOTDIR`, system/user startup ordering, environment/options/
  aliases/functions/prompt readiness, startup abort, wrapper replacement, and
  double-source baseline parity. It must race materialization, close/loss,
  prepare and token issuance, helper consume, OpenCode preparation/POST, and
  provider exec under that lock, plus passive snapshot, new attachment,
  Park/Resume/Fork/contextual `n`/`new-workstream`, archive/Rename,
  recovery/start retry, helper exit, exact exec error, exec proof, immediate
  provider exit, and restart. It must prove one deterministic winner, no
  helper adoption, managed kill, premature signal/action, stuck operation,
  blind rollback, duplicate ownership/shell, or second POST. It must exercise
  `runtime_owned_launching`, every provider preparation/external-effect phase,
  `provider_exec_started`, terminal known-absent failure,
  `provider_exec_proven`, and recovery-required/unknown; only a full
  operation/revision, RuntimeId/generation and exact `RuntimePaths` fields
  (directory, socket, configuration, and session), tmux pane/session,
  PID/birth/PGID/session, and expected-executable proof may activate ordinary
  attachment/action authority. It must also prove terminal known-absent plus
  no-effect evidence performs guarded rollback and ends onboarding, while
  terminal known-absent plus a known OpenCode binding ends onboarding in the
  exact stopped/recovery state with only binding-preserving Resume/recovery or
  explicit Park allowed; exec-error evidence alone never grants ordinary
  action, possible effects remain recovery-required, and no operation stays
  fenced indefinitely. It must also cover issuance-to-helper
  cancellation/crash, replay, expiry, duplicate helper, and every bound-claim
  mismatch before provider effect. Marker deletion with live/dead candidates,
  multiple/unknown `run/runtime-*` artifacts, bounded namespace overflow,
  restart, and stale rollback racing fresh-card selection/materialization. It
  must prove outcome-specific singleton counts: ambiguous or unknown evidence
  leaves every artifact untouched, blocks new materialization, and creates no
  new provisional server or marker (the derived singleton card may remain
  unavailable); conclusive clean/pre-effect rollback creates no duplicate and
  leaves one derived unmaterialized card; successful ownership leaves the
  adopted Runtime server plus one unmaterialized card; and clean
  pre-materialization has zero provisional servers. A two-presentation
  materialization race is serialized by the shared host lease: one valid
  candidate may materialize, while the other presentation recognizes that
  marker/artifact as busy/owned, keeps its derived card visible but unavailable,
  and creates no second server. It never normalizes unknown artifacts to a
  count of one or resets a newer marker. The post-commit action fence applies
  only to the unproven Runtime; selecting the fresh card may attach its separate
  provisional server but grants no authority over that Runtime.
- Spike 0020 passes the bounded OpenCode `1.18.23` fresh-session/provider
  lifecycle revalidation. It supports the adapter contract but is not evidence
  that OpenCode promotion through the D17 broker is implemented.
- Before implementation can rely on account-shell interception, disposable
  tests must prove the exact function, quoting, argument, signal, and `exec`
  behavior for both Bash and Zsh. Login-shell mode and unsupported or ambiguous
  startup contexts must show bounded unavailable guidance and must not launch a
  managed provider. A missing/deleted/unsafe/ambiguous seed cwd must likewise
  fail closed without fallback or Project authority.
- The broker must prove how Codex contextual observer readiness completes
  before final `exec`, and how OpenCode's non-idempotent blank-session
  precreation remains journaled under the same reserved Runtime authority.

### Implementation slices

1. **D17.0 handshake and grammar falsification.** Build disposable synthetic
   Bash/Zsh evidence for the two-phase prepare-token-helper handshake, exact
   non-login wrapper startup/baseline behavior, closed fresh-TUI grammar,
   bounded argument transfer, signals, cancellation, crash gaps, and
   shell-leader PID/birth/process-group preservation. Exercise one
   marker-backed candidate RuntimeId with final full-UUID `RuntimePaths` fields
   (directory, socket, configuration, and session), including candidate
   collision/foreign-artifact refusal and inventory/probe/park/remove/recovery
   exclusion until durable adoption. Exercise the exact stable host-private
   `provisional.lock` (schema-14 creation/reuse, no-follow, mode-`0600`,
   HostId/format/path/inode checks, holder crash/restart, busy timeout,
   symlink/replacement/unlink-recreate refusal, and FD noninheritance) and its
   race across close/loss, prepare/token issuance, helper consume, OpenCode
   preparation/POST, and provider exec. Race passive snapshot, new attachment,
   Park/Resume/Fork/contextual `n`/`new-workstream`, archive/Rename,
   recovery/start retry, helper exit, exec error/proof, immediate provider
   exit, and restart across every post-commit phase. Prove the full
   operation/revision, RuntimeId/generation and exact `RuntimePaths` fields
   (directory, socket, configuration, and session), tmux,
   PID/birth/PGID/session, and executable proof fence, no managed kill,
   premature action, stuck operation, blind rollback, duplicate ownership/shell,
   or second POST. Exercise issuance-to-helper cancellation/crash, replay,
   expiry, duplicate helper, and every bound-claim mismatch before provider
  effect. Spikes 0021-0024 close only the narrow two-phase topology,
  account-wrapper, isolated stable-lock, and versioned grammar risks; their
  listed limits and every unresolved falsification above still stop the
  production slices.
2. **Dormant/test-only provisional ownership.** Add the presentation-scoped
   ownership/card model and private provisional server behind internal test
   seams, including deterministic presentation seed cwd, exact final-form
   candidate `RuntimePaths` fields (directory, socket, configuration, and
   session), fresh `slot_generation`, singleton derivation, detach/reattach,
   and close/loss lifecycle, but do not render
   or activate the provisional card in the ordinary D16 UI. D16 onboarding
   remains exactly usable through this slice and the next slices; do not remove
   or repurpose its Projects, picker, browser, refresh, or provider-choice
   paths yet.
3. **Broker, journal, and migration support.** Add the prepare broker, hidden
   launch-helper boundary, request authentication, bounded Git-root and
   provider-grammar adapters, bounded `runtime_owned_launching` through
   `provider_exec_proven`/known-absent/recovery-required phases, the host-local
   no-provider-effect reconciler, and crash/recovery journal support while D16
   ordinary Projects, picker, browser, and refresh workflows remain usable.
   Migration groundwork may understand schema 14, but does not remove the D16
   settings or action surface before the replacement is ready. D16 onboarding
   remains exactly usable through slice 4.
4. **Provider promotion.** Complete Codex observer-readiness and OpenCode
   blank-session preparation inside the onboarding journal, preserve exact
   Runtime/process authority, provider-exec proof, and conclusive versus
   ambiguous effects through the validated helper handoff for both Bash and Zsh.
   This work stays dormant behind internal seams until the atomic cutover.
5. **Atomic schema-14 and Navigator cutover.** In one coherent product
   cutover, migrate schema 13 transactionally to 14, remove
   `ProjectBrowserSettings` and Project-browser action DTOs, remove the
   Projects/provider-picker/browser/refresh UI, render the pinned shell card
   and in-place promotion, keep Archived, and make `n` the same-provider/
   same-Location selected-Workstream fast path. Remove the current public
   arbitrary-location `register <checkout> [--provider]` command and any
   equivalent `host register-checkout` form in this same cutover. D16 ordinary
   onboarding stays available until this replacement is complete; no hidden
   D16 compatibility behavior remains after cutover.
6. **Recovery and dead-code cleanup.** Reconcile interrupted onboarding,
   remove dead D16 chooser/registration code and the public arbitrary-location
   registration command after the atomic cutover rather than preserving hidden
   compatibility behavior, and keep bypassed provider launches unmanaged.
7. **Acceptance and operator docs.** Run focused tests, complete repository
   gates, and explicitly operator-gated live Codex/OpenCode shell promotion
   with sanitized evidence and complete disposable cleanup.

Each slice is independently reviewable and runs its focused tests plus diff
checks. `scripts/check` is mandatory before every checkpoint commit.

### Exit gate

D17 closes only when all of the following are true:

- exactly one derived provisional shell card is visible, lazily materialized
  with one fresh `slot_generation`, one marker-backed candidate RuntimeId, and
  final full-UUID `RuntimePaths` fields (directory, socket, configuration, and
  session). It remains usable across managed-card switching in the same
  presentation, starts clean shells at the presentation seed cwd, preserves a
  live shell's actual cwd across normal detach/reattach, proves candidate
  collision/foreign-artifact refusal and exclusion from ordinary registry
  inventory, probe, park, remove, and recovery paths until durable adoption,
  and leaves no durable row or residue on shell exit or conclusive pre-handoff
  loss. Marker deletion with live/dead candidates, multiple/unknown
  `run/runtime-*` artifacts, bounded namespace overflow, restart, and stale
  rollback versus fresh-card selection/materialization have outcome-specific
  invariants: ambiguous or unknown evidence leaves every artifact untouched,
  blocks new materialization, and creates no new provisional server or marker
  (the derived singleton card may remain unavailable); conclusive clean/
  pre-effect rollback creates no duplicate and leaves one derived unmaterialized
  card; successful ownership leaves the adopted Runtime server plus one
  unmaterialized card; and clean pre-materialization has zero provisional
  servers. Unknown artifacts are never normalized to a count of one, and a
  newer marker is never reset;
- Bash and Zsh interactive non-login wrapper/functions inherit the validated
  environment and original `HOME`/Zsh `ZDOTDIR`, match ordinary non-login
  baseline matrices (system/user startup ordering, environment, options,
  aliases, functions, and prompt readiness), and reproduce the ordinary
  non-login interactive startup graph exactly once without parsing RC contents.
  The launcher rejects login mode before startup because Bash login mode ignores
  a supplied `--rcfile`; a later nested login shell is unmanaged. The wrappers
  fail closed on abort, replacement, or ambiguity. They pass
  bounded grammar/quoting/argument/signal tests, invoke a child prepare broker
  that returns only an exact one-shot capability, and hand it to the hidden
  helper. The helper opens `provisional.lock`, retains its lease, and
  revalidates every bound marker/process/cwd/path/revision/token claim. Only
  after successful revalidation does the helper atomically compare-and-consume
  the capability and commit durable `Runtime-owned` authority. A mismatch does not advance
  ownership. The helper then revokes presentation cleanup before releasing the
  lease, with durable transition preceding marker cleanup, and only afterward
  prepares provider effects/execs. The final provider
  preserves the shell leader PID, birth token, and process group.
  Issuance-to-helper cancellation/crash, replay, expiry, duplicate helper, and
  each request/operation/presentation/provider/candidate/path/cwd/root/Location/
  Runtime/revision/process/argv-digest mismatch fail before effect. A terminal
  known-absent result is resolved atomically: provider-specific no-effect proof
  performs guarded rollback and ends onboarding, while a known OpenCode binding
  ends onboarding in the exact stopped/recovery state where only
  binding-preserving Resume/recovery or explicit Park is allowed. Exec-error
  evidence alone never grants ordinary action, possible effects remain
  recovery-required, and no operation stays fenced indefinitely;
- the stable host-private `provisional.lock` (distinct from D16's
  `transition.lock`) serializes materialization, close/loss, prepare/token
  issuance, helper consume, singleton reconciliation, and marker cleanup. Its
  schema-14 host-operational metadata stores planned `lease_generation`, phase
  `pending`/`ready`, and expected device/inode once ready; it is not a card or
  Runtime row. Schema-14 ownership and pending metadata commit before any lock
  create/recognition; schema-13 code/path never does so. In pending, an absent
  mode-`0600` current-owner regular file is created with create-new/no-follow,
  bounded file contents are written, the file is fsynced, then the containing
  state-root directory is fsynced before metadata finalizes ready; an exact
  crash-window file may be validated/locked and finalized. Ready missing,
  replaced, or device/inode-mismatched evidence fails closed and is never
  recreated. A pre-schema-14 artifact remains untouched and is never adopted or
  deleted; no cross-store atomicity is claimed. The file contains only bounded
  format version, HostId, and `lease_generation` (no cwd, command, argv,
  provider/user content, or provider payload), and is never unlinked/recreated
  in normal operation. Every actor holds one nonblocking exclusive
  no-follow/CLOEXEC FD lock; crash/restart, busy timeout, symlink, replacement,
  unlink/recreate, or inode mismatch never permits a second lock or unlocked
  mutation. A prepared reservation alone does not revoke cleanup:
  before the helper successfully revalidates every bound marker/process/cwd/
  path/revision/token claim and atomically consumes the capability while
  committing durable `Runtime-owned` authority, close/loss may win only under
  the lease by atomically revoking the unconsumed capability and proving
  pre-effect absence. After that exact helper commit, presentation cleanup
  never signals the server, pane, or process. Ambiguous ownership leaves
  evidence untouched and blocks duplicate creation, and every
  presentation/outer-SSH path preserves managed Runtimes;
- live Codex and OpenCode promotions retain the exact candidate private tmux
  server/path/pane, provider PID, birth token, process group, native terminal,
  and fixed provider kind without rename/rehome/replacement, then keep the
  promoted card selected and derive one unmaterialized singleton card even when
  binding is absent. Runtime-owned is not ordinary attach/action authority for
  the unproven Runtime until the operation reaches `provider_exec_proven` or
  terminal reconciliation: while `runtime_owned_launching`, provider
  preparation/external-effect, or `provider_exec_started`, its originating
  presentation may retain or detach its existing pane, but no new attachment
  to that Runtime is allowed. Selecting the fresh card attaches only its
  separate provisional server under `provisional.lock` and grants no authority
  over the unproven Runtime; ordinary actions for that Runtime refuse/wait with
  onboarding guidance. A passive snapshot,
  action preflight, or restart reconciler performs no provider effect and may
  activate authority only after proving the exact operation/revisions,
  RuntimeId/generation and exact `RuntimePaths` fields (directory, socket,
  configuration, and session), tmux pane/session, PID/birth/PGID/
  session, and expected executable. A possible OpenCode `POST /session` effect
  leaves that same server Runtime-owned and its card visibly recovery-required,
  with no second POST. Helper-recorded exact `execve` errors prove only absence
  of the final provider TUI exec; attempt-only rollback requires provider-specific
  journal proof of no prior external effect or binding. A known OpenCode
  blank-session POST or binding remains on the same Runtime/Workstream/binding
  for exact recovery/resume and is never rolled back or posted again; a possible
  POST is recovery-required. A terminal known-absent result is not itself action
  authority: provider-specific no-effect proof performs guarded rollback and
  ends onboarding, while a known OpenCode binding ends onboarding in the exact
  stopped/recovery state where only binding-preserving Resume/recovery or
  explicit Park is allowed. Exec-error evidence alone never grants ordinary
  action, and no operation stays fenced indefinitely. A crash after
  `provider_exec_started` without proof is ambiguous/recovery-required and is
  never blindly rolled back;
- `git rev-parse --show-toplevel`-equivalent discovery from a child directory,
  main worktree, and linked worktree records the exact containing worktree root;
  non-Git, bare, changed, unsafe, timed-out, and ambiguous seed/current cwd
  evidence launches nothing, leaves the shell intact, and never falls back;
- conflicting cwd/profile/resume/session/attach/server/host/port/endpoint or
  equivalent identity arguments fail before provider effects; only
  version/contract-proven safe native arguments preserve native behavior,
  explicitly enumerated provider-owned non-session commands such as
  `--help`, `--version`, and `login` remain explicitly unmanaged with their
  effects provider-owned, other non-fresh-TUI shapes refuse with bounded
  guidance, secret-bearing argv remains outside the promotable grammar, and
  bypassed launches are never adopted from process, pane, hook, or inventory
  evidence;
- conclusive pre-effect failures, including those discovered after the exact
  helper commit, are classified by onboarding recovery and leave no Project,
  Location, Workstream, Runtime, binding, or operation residue after their
  attempt-only rollback, while every possible post-effect result is visible as
  one recovery-required managed Workstream and cannot be blindly retried;
- schema 13 migrates transactionally to 14 without a state wipe, removes only
  obsolete browser settings, and preserves all enumerated authoritative state;
- Workstreams and Archived are the only pages; Project grouping is derived
  from retained Workstreams/Locations, no browser/root/refresh action survives,
  and `n` uses the selected managed Workstream's exact provider and Location;
- provider exit preserves the managed stopped/recoverable card and completed
  output, provider cwd/worktree changes never retarget it, and D12 utility-shell
  behavior remains distinct and bounded;
- all automated tests use disposable state roots, repositories, provider homes,
  account-shell startup files, and private tmux sockets; and
- `scripts/check`, staged and unstaged `git diff --check`, focused D17 tests,
  and explicitly authorized sanitized live acceptance pass with complete
  cleanup and no ordinary tmux/provider-state interference.

## Deferred beyond V1

The roadmap does not include arbitrary existing-session adoption, hard
Workstream/provider-session deletion, worktree or branch removal, checkout
synchronization, task/context transfer, transcript or memory features,
automatic plan rollover, provider/model/role launch presets, provider filters
or grouping, passive provider-session adoption, unproven OpenCode navigator
Rename, profile composition, Claude parity, multiple-controller catalog
synchronization, a public daemon, or a replacement provider UI.

WSNav-managed cross-host/SSH operation is retired by D16, not deferred for a
later aggregator. Ordinary SSH composition and one host-local wsnav instance
per execution host are the supported multi-host workflow.

The one D17 provisional onboarding shell does not approve durable or multiple
per-Workstream shells. A future presentation could preserve each utility shell
across Workstream switches, but that remains deferred until its multi-shell
resource bound, background-shell visibility, transition rollback, and cleanup
contract are explicitly approved.
