# Workstream Navigator documentation

## Status and authority

D16 host-local implementation, disposable repository gate, and separately
authorized live local and ordinary-SSH-entered-host acceptance are complete.
The current source also includes documented post-acceptance rendering,
attachment, Park convergence, and compact-navigator corrections; those
follow-ups do not rewrite the earlier candidate's live-acceptance evidence.
D17 shell-first managed-session onboarding is now the approved target design
and next planned checkpoint; it is not implemented or accepted yet.

- [Product and architecture design](design.md) is the V1 contract.
- [Delivery roadmap](roadmap.md) owns delivery order, checkpoint status, and
  exit gates.
- [Product captures](media/README.md) are frozen, privacy-safe pre-D16 design
  history and do not show the current reduced navigator.
- [Historical evidence](evidence/README.md) preserves the exact candidate,
  environment, procedure, and limitations of earlier checkpoints.

## Current operator contract

This section describes the shipped D16 binary. It intentionally retains the
Projects page and provider chooser until D17 implementation replaces them.

WSNav is host-local. Run it on the machine where the provider Runtime lives.
For another machine, establish ordinary SSH in a separate terminal, tab, or
window and run that host's own `wsnav`; separate hosts therefore have separate
WSNav windows. WSNav itself has no SSH registration, remote transport,
cross-host polling, remote attachment, or combined catalog. If an outer SSH
connection ends, the disposable presentation may be lost while the host-local
Runtime and provider remain untouched; reconnect and rerun `wsnav` on that host
to reattach.

The reduced navigator has three direct pages: Workstreams (active
Workstreams, Project-grouped), Projects (host-local Projects and Locations),
and Archived (Project-grouped restore). `,` opens Projects and `.` opens
Archived; `Esc` returns to Workstreams; `Left` and `Right` no longer cycle
views. Workstreams keeps `Enter`, `n`, `f`, `p`, `x`, `a`, and `?`; Projects
provides Location registration (`a`), browser-root configuration (`b`),
Location-based New (`n`), and explicit metadata refresh (`r`); Archived uses
`u` to restore without starting or attaching a provider. The installed
`wsnav --help` output is the CLI reference for that exact binary.

Projects registers exact host-local Git Locations through a bounded browser,
rooted at `~` by default. It uses relative cursors and does not expose raw
paths in snapshots or provider panes. Repository metadata is refreshed only by
the explicit Projects action; ordinary redraw, attachment, and fast
Workstream switching do not inspect Git. A selected Location can start a new
Workstream even when its Project has no active Workstreams.

New Workstream provider choice is explicit. A ready provider is selectable,
and Codex also remains selectable when its exact observer setup, update, or
native trust review can be completed by the contextual guide. From an existing
Workstream, switching to a sole different provider still requires confirmation;
WSNav never silently substitutes it. New provider conversations are independent
and never migrate or copy context from the source Workstream.

Observer readiness is contextual rather than a page or manual setup mode.
Startup detects it read-only. A Codex action that needs readiness opens a guide
that captures the exact intent, asks explicit consent before one exact owned
profile is created or updated, opens native trust review without granting
trust, and continues only after readiness and revisions are revalidated.
Decline, foreign/modified/disabled/ambiguous state, or live-Runtime conflicts
leave state unchanged. Exact removal is an exceptional cleanup path; normal
CLI use returns guidance rather than installing or reviewing a profile.

D16 removes the client catalog in a clean break. The exact
`client.sqlite`, `client.sqlite-wal`, and `client.sqlite-shm` paths are
discarded without reading or importing their contents. There is no importer,
dual write, automatic backup, downgrade, or rollback adapter. An operator who
wants downgrade insurance may, before interactive confirmation, park or stop
managed Runtimes, exit WSNav, and create a verified offline copy of the
complete state root; restoring that external copy is outside D16 and is the
only downgrade procedure. Schema 12 migrates transactionally to schema 13
using `host.sqlite` only, preserving host/runtime/provider identity and history
while rebuilding host-local Projects.

Legacy presentation retirement is also fail-closed. Attached clients,
utility shells, and native observer-review surfaces block mutation. A bounded
drain-only attachment may be offered without opening host state so the user
can finish and quit the old presentation. Only an exact detached owned
presentation may be retired under the transition lease; Runtime tmux servers,
provider processes, sessions, and completed output are never targeted.

## D17 target contract (not yet implemented)

D17 will reduce the ordinary navigator to Workstreams and Archived. Workstreams
will always show one pinned `New session · shell` card. At presentation
creation, WSNav captures, validates, and canonicalizes that presentation's
invocation cwd as a private seed. Selecting the card lazily materializes one
opaque candidate `RuntimeId` and creates the provisional tmux directory, socket,
configuration, and session with the existing final full-UUID `RuntimePaths`
form. The candidate ID, exact `RuntimePaths` fields (directory, socket,
configuration, and session), seed, and ownership evidence live only in the
presentation-private marker; they do not create a registry Runtime or
Workstream row. Before creating those artifacts, materialization proves the
candidate ID and all four path fields are absent and unused; it never adopts
pre-existing artifacts. A marker-backed candidate is excluded from ordinary
registry inventory, probe, park, remove, and recovery discovery/action until
durable adoption; only the exact presentation marker plus the stable host-private
`provisional.lock` lease may manage it. Markerless/registryless, foreign, or
collision artifacts remain untouched, and a clean replacement allocates a fresh
candidate RuntimeId.
Every newly materialized clean shell starts at that seed, while
detach/reattach preserves a live shell's actual cwd. Missing, deleted, unsafe,
or ambiguous seed cwd makes onboarding unavailable with guidance and never
falls back or becomes Project authority. A new presentation captures its own
seed. The pinned provisional card is a derived singleton with no durable card
row. Each materialization mints a fresh opaque `slot_generation` in the marker;
the capability and onboarding journal bind that generation to the candidate.

The selected card opens a presentation-scoped account shell; the user changes
directory normally and types `codex` or `opencode`. D17 supports Bash and Zsh
interactive non-login shells only. Shell-specific private wrappers inherit the
validated presentation environment, original `HOME`, and (for Zsh) original
`ZDOTDIR`, reproduce the ordinary non-login interactive startup graph in its
system/user order exactly once, then remove conflicting `codex`/`opencode`
aliases/functions and install exact WSNav functions. Observable environment,
options, aliases, functions, and prompt readiness match an ordinary disposable
baseline except bounded wrapper state and intentional interception. WSNav
never parses or persists RC contents; login-shell mode, startup-abort, wrapper
replacement, and ambiguous startup contexts fail closed with guidance.

For a promotable fresh interactive native TUI shape, the controlled function
invokes a bounded prepare broker as a child over private non-terminal control
I/O. One stable host-private `provisional.lock` serializes materialization,
close/loss cleanup, prepare/token issuance, helper consume, singleton
reconciliation, and marker cleanup; it is distinct from D16's schema-cutover
`transition.lock` and is operational state rather than presentation-private
storage or a Runtime/card/Workstream row. Schema-14 host-operational lease
metadata stores only a planned `lease_generation`, install phase `pending` or
`ready`, and expected lock device/inode once ready; it is not a card, Runtime,
Workstream, or presentation-private row. The schema/HostId transaction commits
schema-14 ownership and pending metadata first; schema-13 code and path never
create or recognize `provisional.lock`.
Only after that database commit is durable may schema-14 startup reconcile the
artifact. In `pending`, an absent mode-`0600` current-owner regular file is
created lazily with create-new/no-follow, bounded file contents are written, the
file is fsynced, then the containing state-root directory is fsynced before
metadata finalizes `ready` with expected device/inode;
an exact file left by a crash may be validated/locked and finalized. Pending
foreign or mismatched evidence fails closed. In `ready`, missing, replaced, or
device/inode-mismatched evidence fails closed and is never recreated. The file
contains only bounded format version, HostId, and `lease_generation`; it carries
no cwd, command, argv, provider/user content, or provider payload. A pre-schema-14
artifact is unexpected/ambiguous, remains untouched, and is never adopted or
deleted. This ordering does not claim cross-store atomicity. Every actor opens it
no-follow/CLOEXEC, retains one
nonblocking exclusive kernel-lock FD, revalidates root/path/FD device-inode
identity before mutation, and never leaks the FD across provider exec; crash
releases only the kernel lock, and restart reacquires the same artifact. Busy,
timeout, malformed, symlinked, foreign, replaced, or locked evidence fails
closed with guidance; state-root reset/removal is outside this flow. The marker,
capability, and journal bind and check both the lock's `lease_generation` and the
presentation/slot `slot_generation` on each transition.
Each participant rechecks the marker, onboarding journal, and
presentation/registry revisions while holding it. The broker transactionally
reserves the durable graph and Runtime generation for the exact candidate ID
and unchanged `RuntimePaths` fields (directory, socket, configuration, and
session), without renaming, rehoming, or replacing the live server, marks the
handoff issued while holding the lease,
then returns only an exact
one-shot opaque launch capability, never a command or argv. Its claims bind
the request/operation, presentation and provisional slot, candidate ID and
exact `RuntimePaths` fields (directory, socket, configuration, and session),
fixed provider, exact shell cwd/root/Location, reserved generation, captured
revisions, shell PID/birth/process group, grammar-approved argv digest, and
short monotonic expiry. A prepared reservation alone does not revoke
provisional cleanup: close may win by canceling an unconsumed capability after
proving pre-effect absence. If the helper wins, it reacquires the lease,
while holding it revalidates every bound marker/process/cwd/path/revision/token
claim. Only on successful revalidation does it atomically compare-and-consume
the capability and commit durable `Runtime-owned` authority for the candidate;
a mismatch does not advance ownership. It then, still under the lease and
before releasing it, revokes/removes presentation cleanup authority; durable
transition precedes marker cleanup, and only afterward prepares provider effects, builds provider
argv internally, and `exec`s the provider, preserving the shell leader PID,
birth token, and process group. Persisted state keeps
only a bounded token identifier/verifier/phase and claim references or digests;
the live token and argv are never persisted. After ownership commits, the same
selected card becomes the managed Workstream and the UI derives a fresh
unmaterialized singleton card even when native binding is not ready. A provider launched by
bypassing the function remains unmanaged and is never passively adopted.

Each presentation derives one pinned provisional card, but the shared host
`provisional.lock` and classifier permit at most one unregistered materialized
candidate server across all presentations. A valid marker/artifact belonging to
another presentation is busy/owned, not unknown or adoptable; that card remains
visible but unavailable until its slot promotes or conclusively cleans. Under
the same lock, a bounded classifier cross-checks the exact marker and unfinished
onboarding operations against registered Runtime IDs and the bounded
`run/runtime-*` namespace only to detect conflicts. It never passively adopts or
deletes unknown artifacts. Missing or changed marker evidence with any
unregistered Runtime-shaped artifact, multiple candidates, or ambiguous
journal/path/process evidence blocks every fresh materialization and leaves
artifacts untouched; it cannot evade ambiguity by choosing a new UUID. A clean
replacement is allowed only after exact prior absence or conclusive cleanup,
with a fresh `slot_generation` and candidate RuntimeId. Ownership consumes the
old slot generation; rollback is revision/slot-generation guarded and targets
only the old operation/Runtime/slot, leaving any newer marker/card unchanged and
never creating a second card or resetting a new shell.

Helper ownership commit does not yet make the Runtime ordinarily attachable or
actionable. The operation enters `runtime_owned_launching`, then provider-
specific preparation/external-effect phases and `provider_exec_started`
immediately before `execve`; terminal phases are `provider_exec_proven`, a
known-absent exec failure, or `recovery-required`/`unknown`. Until
`provider_exec_proven` or terminal reconciliation, attachment and action
authority for that unproven Runtime remains fenced. Its originating
presentation may retain its existing tmux
Runtime attachment/pane or detach through ordinary card switching, but no new
attachment to that Runtime is allowed. Selecting/materializing the fresh derived
singleton card attaches only its separate provisional server under
`provisional.lock` and grants no authority over the unproven Runtime. Park,
Resume, Fork, contextual `n`/`new-workstream`, archive, Rename, recovery/start
retry, and cleanup actions for that Runtime refuse or wait with bounded
`onboarding-in-progress` guidance. Passive snapshot/probe may show
`starting`/`onboarding` and reconcile, but never adopts the helper/preparation
process as provider identity, marks the Runtime lost, or signals it. Once
terminal `recovery-required`, only the existing exact recovery or explicit Park
rules apply. A terminal known-absent exec result is not itself action authority:
the reconciler must atomically resolve it. When provider-specific journal
evidence proves no prior external effect or binding, guarded rollback ends
onboarding and leaves the derived singleton card available but unmaterialized.
When OpenCode has a known blank-session POST or binding, the same atomic
resolution ends onboarding in the exact stopped/recovery state; only
binding-preserving Resume/recovery or explicit Park is then allowed. A possible
effect remains `recovery-required`. No ordinary action is enabled directly by
exec-error evidence, and no operation remains fenced after terminal
reconciliation. A host-local reconciler proves the same
operation/revisions, RuntimeId/generation and exact `RuntimePaths` fields
(directory, socket, configuration, and session), tmux pane/session,
PID/birth/PGID/session,
and expected executable before atomically committing `provider_exec_proven` and
activating ordinary authority. A possible exec/provider effect is
`recovery-required`; it is never blindly rolled back. Codex may remain managed
`starting`/unbound until `SessionStart`; a known OpenCode blank-session POST or
binding remains on the same Runtime/Workstream/binding for exact recovery/resume
after a final TUI failure and is never rolled back or posted again. A possible
POST remains `recovery-required`.

Only fresh native TUI command shapes are promotable. Broker-owned cwd,
profile, resume/session, attach/server, host/port/endpoint, and equivalent
identity arguments refuse before reservation. Explicitly enumerated provider-
owned non-session commands such as `--help`, `--version`, and `login` may run
directly as explicitly unmanaged commands; their effects remain provider-owned.
Other shapes receive bounded guidance to use an ordinary terminal or explicit
bypass. Any secret-bearing argument or value is outside the promotable grammar.
Safe native options are admitted only after provider adapter/version-contract
validation.

The broker detects the exact containing non-bare Git worktree root from the
shell's current cwd and registers it with the new Workstream; only this
broker-time check creates ProjectLocation/launch authority. Linked worktrees
remain distinct Locations. WSNav will not create, switch, remove, or follow
worktrees, and the Workstream stays pinned to its launch root even if the
provider later works elsewhere. `n` on a selected managed Workstream remains
the direct same-provider, same-Location path for another blank session; a
different provider or location begins through the shell card, while `f` remains
a conversation Fork. WSNav does not persist arbitrary cwd history.

Card and server state key off Runtime ownership, not provider success. A known
helper `execve` error proves only absence of the final provider TUI exec;
attempt-only graph rollback is allowed only when the provider-specific journal
also conclusively proves no prior external effect or binding. For OpenCode, any
known blank-session `POST /session` or binding remains on that same Runtime,
Workstream, and binding for exact recovery/resume if final TUI exec fails; it is
never rolled back and recovery never issues a second POST. Any possible POST
effect leaves the card visibly `recovery-required`, even if no native TUI
remains, and presentation cleanup cannot touch that server. A conclusive
pre-effect failure after the exact helper commit is classified by onboarding
recovery; when provider-specific evidence proves no effect or binding, it rolls
back attempt-only graph state and leaves the derived singleton card available
but unmaterialized. An ambiguous-effect slot is never reusable.

Projects remain internal host-local grouping metadata for Workstreams and
Archived. D17 exposes no Projects page, provider picker, Location picker,
browser-root setting, repository-registration form, or manual metadata-refresh
action. The current arbitrary-location `register <checkout> [--provider]`
command (and equivalent public registration form) is removed at the atomic
cutover; only the brokered shell can create a new Location/provider pair.

The schema-13-to-14 migration will remove the obsolete Project-browser setting
while preserving authoritative Projects, Locations, Workstreams, Runtimes,
bindings, attention, and unfinished operations. D17 does not require a state
wipe. Normal detach and reattach to the same owned presentation preserves the
exact provisional shell. The stable host-private `provisional.lock` lease makes
confirmed close or conclusive loss recheck the marker, onboarding journal, and
revisions, then clean only exact pre-handoff provisional ownership by canceling
an unconsumed capability
after proving pre-effect absence; before the helper successfully revalidates
every bound marker/process/cwd/path/revision/token claim and atomically
consumes the capability while committing durable `Runtime-owned` authority,
cleanup may win only under that lease with atomic revocation and proven
pre-effect absence. After that exact helper commit, presentation cleanup never
signals that server. Ambiguous ownership leaves evidence untouched and blocks
duplicate creation. Shell exit and conclusive pre-effect launch failure are
resolved by onboarding recovery; ambiguity after a provider effect remains visible as a
recovery-required managed Workstream. Public `new-workstream` remains
source-based parity for contextual `n`: it inherits the exact source provider
and Location and rejects provider/path overrides. Source-less arbitrary
provider/path creation is broker-only.

D17.0 remains unproven until disposable Bash/Zsh baseline matrices and a race
of close/loss against materialization, prepare/token issuance, helper consume,
OpenCode preparation/`POST /session`, and provider `exec` proves one lease
winner with no managed kill, duplicate ownership, duplicate shell, or second
POST. It proves the schema/HostId transaction commits schema-14 ownership before
lock creation/recognition, that schema-13 code/path does neither, that a crash
after the database commit but before file creation retries safely, and that a
pre-schema-14 lock artifact is unexpected/ambiguous and remains untouched rather
than adopted or deleted; no cross-store atomicity is assumed. It exercises the
exact stable host-private `provisional.lock` across schema-14 pending-before-file,
file-before-ready, ready steady-state, and ready missing/replacement crash and
restart cases, plus creation/reuse, no-follow/root/path/inode validation, holder
crash/restart, busy timeout, symlink/replacement/unlink-recreate refusal, and FD
noninheritance. It also races
passive snapshot, new attachment,
Park/Resume/Fork/contextual `n`/`new-workstream`, archive/Rename,
recovery/start retry, helper exit, exact exec error/proof, immediate provider
exit, and restart across `runtime_owned_launching`, provider preparation/
external-effect, and `provider_exec_started`, proving only full identity proof
activates ordinary authority. It proves terminal known-absent plus no-effect
evidence performs guarded rollback and ends onboarding, while terminal
known-absent plus a known OpenCode binding ends onboarding in the exact
stopped/recovery state with only binding-preserving Resume/recovery or explicit
Park allowed; exec-error evidence alone never grants ordinary action, possible
effects remain recovery-required, and no operation stays fenced indefinitely. It
covers candidate collision/foreign-artifact
refusal, marker deletion, multiple/unknown `run/runtime-*` artifacts, bounded
namespace overflow, and stale rollback versus fresh-card materialization while
excluding marker-backed candidates from ordinary registry inventory/probe/park/
remove/recovery paths until durable adoption. The revision/slot-generation
reconciler is idempotent across restart with outcome-specific counts: ambiguous
or unknown evidence leaves every artifact untouched, blocks new materialization,
and creates no new provisional server or marker (the derived singleton card may
remain unavailable); conclusive clean/pre-effect rollback creates no duplicate
and leaves one derived unmaterialized card; successful ownership leaves the
adopted Runtime server plus one unmaterialized card; and clean pre-materialization
has zero provisional servers. A two-presentation materialization race is
serialized by the shared host lease: one valid candidate may materialize, while
the other presentation recognizes that marker/artifact as busy/owned, keeps its
derived card visible but unavailable, and creates no second server. It never
normalizes unknown artifacts to a count of one or resets a newer marker. The
post-commit action fence applies only to the unproven Runtime; selecting the
fresh card may attach its separate provisional server but grants no authority
over that Runtime. The [D17 roadmap
checkpoint](roadmap.md#d17---shell-first-managed-session-onboarding) lists the
complete exit gates and keeps the D16 binary as the current usable product.

## Build and command references

The project is a source-installed operator beta. From a reviewed checkout,
build and install at a high level with `cargo build --locked --release` and
`install -m 755 target/release/wsnav ~/.local/bin/wsnav`; run `scripts/check`
for the repository gate. `wsnav --help` is the installed CLI reference. D16
cutover is available only through the ordinary interactive `wsnav` startup
flow; there is no public transition command. The normal workflow is the
Navigator beside the native provider TUI.
