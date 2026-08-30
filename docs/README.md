# Workstream Navigator documentation

## Status and authority

D0-D18 are complete. D18 preserves the shell-first managed-session product on
one direct schema-15 epoch while deleting the old transition and compatibility
plane. The corrected release artifact is
installed byte-for-byte, the authorized destructive reset and ordinary
schema-15 bootstrap are complete, and a real 80-column shell-first launch
passes. Rust 1.88 and Ubuntu 24.04 clean-environment matrices, explicit native
observer trust, disposable Codex/OpenCode lifecycle acceptance, complete
cleanup, and the full repository gate pass. The checkpoint commit containing
this record binds the accepted source to the installed release artifact.

- [Product and architecture design](design.md) is the V1 contract.
- [Delivery roadmap](roadmap.md) owns delivery order, checkpoint status, and
  exit gates.
- [Product captures](media/README.md) are frozen, privacy-safe pre-D16 design
  history and do not show the current reduced navigator.
- [Historical evidence](evidence/README.md) preserves the exact candidate,
  environment, procedure, and limitations of earlier checkpoints.

## D18 accepted current-only release

[D18 current-only consolidation](design.md#current-schema-15-consolidation-boundary)
is implemented in the installed release. It preserves the current
Shell-first product while replacing the accumulated D16/D17 transition plane
with one direct schema-15 bootstrap and semantic current modules. It is an
intentional reset epoch: D18 refuses rather than migrates or adopts schema-12
through schema-14 roots. The
[roadmap](roadmap.md#completed-checkpoint-d18-current-only-consolidation) records
the completed disposable-provider, native-trust, cleanup, and commit gates. The
rejected coherent-backup design remains historical
evidence in [spike
0027](evidence/spikes/0027-d18-root-move-falsification.md), but its
arbitrary-holder proof is not required for discarded state.

The authorized reset parked the exact D17.1 Runtimes, removed the owned observer
declaration, and quarantined the whole schema-14 root as discarded state before
D18 installed and directly created schema 15. No old state is migrated,
adopted, or restored; the exact quarantine was deleted after acceptance.

## Current operator contract

This section describes the installed D18 release.

WSNav is host-local. Run it on the machine where the provider Runtime lives.
For another machine, establish ordinary SSH in a separate terminal, tab, or
window and run that host's own `wsnav`; separate hosts therefore have separate
WSNav windows. WSNav itself has no SSH registration, remote transport,
cross-host polling, remote attachment, or combined catalog. If an outer SSH
connection ends, the disposable presentation may be lost while the host-local
Runtime and provider remain untouched; reconnect and rerun `wsnav` on that host
to reattach.

The shell-first navigator has two direct pages: Workstreams and Archived. `.`
opens Archived; `Esc` returns to Workstreams; `Left` and `Right` do not cycle
views. Workstreams keeps `Enter`, `n`, `f`, `p`, `x`, `a`, and `?`; Archived
uses `u` to restore without starting or attaching a provider. The installed
`wsnav --help` output is the CLI reference for that exact binary.

Retained public management commands use the same schema-15 snapshot and
revision-fenced action boundaries as the Navigator. Passive status and
operation queries do not launch a provider or inspect tmux. Direct scripting
commands never install observer state or open native review; a Codex action
that needs setup returns a typed readiness-required refusal and points to the
interactive `wsnav` flow.

Workstreams starts with one pinned `Shell` card outside Project groups. A fresh
presentation starts with that card selected and its account shell visible on
the right; reconnect preserves the detached presentation's current surface.
The stable two-line card shows `Shell`, then a cwd line that abbreviates every
parent component and keeps the leaf folder whole, for example
`~/c/workstream-navigator`. This display is neither persisted nor used as
launch authority. Change directory with ordinary shell commands and run
`codex` or `opencode`. The native command owns provider and launch-option
choice. Successful brokered launch registers the detected worktree root,
promotes that same selected card to a managed Workstream, and derives a fresh
Shell card. There is no Projects page, provider picker, path picker, or
below-provider split shell.

Observer readiness is contextual rather than a page or manual setup mode.
Startup detects it read-only. A provisional shell checks readiness before any
Codex broker reservation; when setup is needed, its wrapper retains the
bounded argv only in process memory, asks explicit consent, opens native trust
review, and retries only after exact readiness. A managed Codex action instead
captures its exact intent and revisions in the Navigator before using the same
review surface. Decline, foreign/modified/disabled/ambiguous state, or
live Codex Runtime conflicts leave state unchanged. Exact removal is an exceptional
cleanup path; direct CLI use returns typed guidance rather than installing or
reviewing a profile.

The native review cwd is an empty, presentation-owned directory with bounded
owner/process and filesystem-identity evidence. Normal exit removes it
non-recursively through its process-local owner; presentation teardown finishes
an interrupted exact cleanup only after stopping the possible native users.
Changed, non-empty, foreign, or ambiguous paths are preserved.

D18 directly creates only schema 15 through its stable `bootstrap.lock` and
refuses old, future, foreign, malformed, mixed, or transition-shaped roots
before SQLite recovery or mutation. There is no importer, migration, adoption,
automatic backup, downgrade, or state-rollback adapter. The discarded D17.1
quarantine was deleted after acceptance and is never an input to the installed
binary.

## Managed-onboarding contract

The ordinary navigator has only Workstreams and Archived. Workstreams
always shows one pinned `Shell` card. At presentation
creation, WSNav captures, validates, and canonicalizes that presentation's
invocation cwd as a private seed. After both presentation panes are proven,
fresh startup selects the card and materializes one opaque candidate
`RuntimeId`, creating the provisional tmux directory, socket, configuration,
and session with the existing final full-UUID `RuntimePaths` form. The
candidate ID, exact `RuntimePaths` fields (directory, socket,
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

The card is headed exactly `Shell` with no path-selection hint. It shows the
bounded live cwd described above, then opens a presentation-scoped account
shell; the user changes
directory normally and types `codex` or `opencode`. WSNav supports Bash and Zsh
interactive non-login shells only. Shell-specific private wrappers inherit the
validated presentation environment, original `HOME`, and (for Zsh) original
`ZDOTDIR`, reproduce the ordinary non-login interactive startup graph in its
system/user order exactly once, then remove conflicting `codex`/`opencode`
aliases/functions and install exact WSNav functions. Observable environment,
options, aliases, functions, and prompt readiness match an ordinary disposable
baseline except bounded wrapper state and intentional interception. The launcher
rejects login mode before startup because Bash login mode does not load a
supplied `--rcfile`; a later nested login shell is unmanaged. WSNav never parses
or persists RC contents; startup-abort, wrapper replacement, and ambiguous
startup contexts fail closed with guidance.

For a promotable fresh interactive native TUI shape, the controlled function
invokes a bounded prepare broker as a child over private non-terminal control
I/O. One stable host-private `provisional.lock` serializes materialization,
close/loss cleanup, prepare/token issuance, helper consume, singleton
reconciliation, and marker cleanup. It is bootstrap-bound operational state,
not presentation-private storage or a Runtime/card/Workstream row. Direct
schema-15 bootstrap precommits its generation, installs the exact mode-`0600`
current-owner create-new/no-follow inode, and records its device/inode before
the bootstrap becomes `ready`. `bootstrap.lock`, host operational metadata,
and the file must agree whenever current state opens. A missing, replaced,
foreign, or mismatched ready artifact fails closed and is never recreated or
adopted. The file contains only bounded format version, HostId, and
`lease_generation`; it carries no cwd, command, argv, provider/user content,
or provider payload. Every actor opens it no-follow/CLOEXEC, retains one
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
Resume, Fork, contextual `n`, archive, Rename, recovery/start retry, and
cleanup actions for that Runtime refuse or wait with bounded
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
Archived. WSNav exposes no Projects page, provider picker, Location picker,
browser-root setting, repository-registration form, or manual metadata-refresh
action. The arbitrary-location `register <checkout> [--provider]` command and
equivalent public registration form are absent; only the brokered shell can
create a new Location/provider pair.

Direct schema 15 omits the obsolete Project-browser setting and retains only
internal Project/Location grouping required by Workstreams. Normal detach and
reattach to the same owned presentation preserves the exact provisional shell.
The stable host-private `provisional.lock` lease makes
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
recovery-required managed Workstream. The public `new-workstream` command is
removed. Contextual `n` inherits the selected Workstream's exact provider and
Location and has no provider/path override; every new provider/Location pair is
broker-only through the provisional shell.

Disposable automated evidence covers the schema/lease boundaries, Bash/Zsh
wrappers, marker-backed Runtime handoff, provider grammar, crash/recovery
fences, exact attachment, card promotion/selection, live cwd display, and
retired split-shell bindings. The
[D17 acceptance record](evidence/acceptance/d17-shell-first.md) records the
historical onboarding exit gates. The
[D18 acceptance record](evidence/acceptance/d18-current-source-candidate.md)
records the current sanitized Codex/OpenCode pass with explicit operator intent
and complete disposable cleanup.

## Build and command references

The project is a source-installed operator beta. The installed D18 release
opens only schema 15 and its `wsnav --help` is the installed CLI reference. Run
`cargo build --locked --release` and `scripts/check` before any replacement.
The destructive reset, exact installation, native observer trust, and
live-provider acceptance are complete; there is no state rollback. The normal
workflow remains the Navigator beside either the provisional account shell or
the native provider TUI.
