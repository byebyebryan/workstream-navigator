# Workstream Navigator V1 Design

Date: 2026-08-29

Status: D18 implements the shell-first product on a direct, current-only
schema-15 epoch with semantic modules and no old-root migration or adoption.
Accepted checkpoint `c961c7e` and installed artifact
`f732e2b16344b038cd05996501ce77be42302f7403de9720d156dbf24777d124`
passed the authorized destructive reset, direct ordinary bootstrap, explicit
native observer trust, disposable Codex/OpenCode lifecycle acceptance, and
complete cleanup. Post-acceptance source correction `ed0d883` shares a bounded
fail-closed topology retry across presentation startup and attach restoration
and directly tests current Fork/recovery transaction ordering. Its local full
gate and repeated Rust 1.88/tmux 3.3a matrix pass; no remote-CI or
accepted-release/live-provider evidence is transferred to the correction.
Per-host development installation is a separate operational fact. The product
and state contract is unchanged, and D18 remains complete.

The design is the current product and architecture contract. Dated acceptance,
spike, and study records preserve the evidence and limitations of the candidate
they tested; their historical version numbers, test counts, and presentation
details do not supersede this contract.

## Product thesis

Workstream Navigator is a thin terminal navigator for persistent coding-agent
workstreams on the machine where it is running. It adds organization,
attachment, status, and a few compound workstream actions around the provider's
native terminal UI.

It is not a replacement terminal, provider frontend, task manager, transcript
store, project-memory system, or autonomous agent orchestrator.

The central design rule is:

> Workstream Navigator owns where work runs and how the user reaches it on the
> current host. The provider owns the conversation and how the user works
> inside it.

Historical tmux/SSH and native Codex spikes established that this split was
technically viable for the former cross-host surface. That evidence remains
truthful for the candidate it tested, but D16 retires WSNav-managed SSH and
cross-host operation from the current product. To use another machine, the
operator opens an ordinary SSH terminal/tab/window, runs `wsnav` there, and
uses that host-local instance. WSNav itself does not establish or manage that
connection.

The retained two-server presentation can show minor cursor artifacts in Ghostty
during high-churn native TUI activity such as typing or streaming. After
removing WSNav's continuous runtime and presentation control probes, that
residual is accepted as non-blocking V1 visual polish: it does not alter input,
provider output, result retention, or provider ownership. The artifact is
caused by upstream tmux behavior ([tmux issue
5419](https://github.com/tmux/tmux/issues/5419)): every full client redraw
emits `civis`/`cnorm` cursor-visibility toggles, which repeatedly restart the
cursor blink phase in the nested path. That version-bound diagnosis was made
on tmux `3.7b`. D16 acceptance later ran on tmux `3.7c`, but did not rerun the
cursor-fidelity study or claim that the upstream issue was fixed. WSNav
therefore keeps its best-available private-server configuration and defers a
change until a candidate upstream fix passes the recorded A/B instrument. The
instrument, ruled-out workarounds, and decision gates are recorded in
[Spike 0014](evidence/spikes/0014-terminal-fidelity-a-b.md) and the
[archived roadmap](roadmap-through-d18-design.md#2026-08-04-terminal-fidelity-root-cause-is-upstream-tmux).

A provider launched manually in one ordinary tmux pane does not use the same
rendering path. Its output crosses one tmux renderer. WSNav keeps each provider
inside its own private Runtime tmux server so the process and completed output
survive presentation detach, then uses a separate private presentation tmux
server to place the Navigator beside it. The presentation's provider pane runs
a nested tmux client attached to the Runtime; it is not a transparent byte
pipe. Provider terminal output is therefore parsed and rendered by the Runtime
server, then parsed and rendered again by the presentation server. Launching
WSNav from an ordinary tmux pane adds a third renderer. This intentional
topology explains why the artifact can be absent in a manual pane while the
same native provider shows it through WSNav.

Provider cursor mode is a separate concern from redraw amplification. WSNav's
private tmux servers leave `cursor-style` at `default`, and the local Ghostty
configuration has no cursor-blink override. A metadata-only check on 2026-08-13
found the live private OpenCode Runtime in blinking mode and both a live private
Codex Runtime and ordinary Codex panes in steady mode. The
[OpenCode TUI configuration](https://opencode.ai/docs/tui/) documents a block
cursor with blinking enabled as its default and provides the native
`cursor.blinking = false` control in `tui.json` or `tui.jsonc`. Cursor policy
therefore remains provider-owned. WSNav intentionally leaves OpenCode's native
blinking default unchanged and must not impose a tmux override or manage an
OpenCode cursor configuration to normalize providers. The distracting irregular
flicker remains the separate nested tmux redraw path repeatedly hiding, showing,
and disturbing the cursor. An operator also reports a steady cursor in Claude,
but Claude is outside the V1 provider surface and that observation is not
independent WSNav evidence.

WSNav asks tmux for multi-field control metadata using printable `|`
separators. tmux 3.4 normalizes literal control separators in format strings;
the printable delimiter preserves exact field boundaries across the supported
matrix. All current fields are bounded identifiers or enums, and legacy
free-form evidence containing the delimiter fails closed. This traffic never
contains or captures provider-pane content.

## V1 tenets

1. **Preserve the native provider workflow.** Codex owns its composer, models,
   permissions, Plan choices, `/new`, `/clear`, `/fork`, `/rename`, resume,
   history, and transcripts.
2. **Augment instead of intercepting.** Normal work does not pass through a
   manager-owned prompt box, plan router, session wizard, or model picker.
3. **Keep the completed result visible.** Workstream Navigator never writes
   status, routing, synthesis, or completion traffic into the provider pane.
4. **Make workstreams explicit.** A workstream is an independent provider and
   runtime lane pinned to the Git worktree root detected when it is launched,
   not a task record, filesystem owner, or synonym for a provider chat.
5. **Treat the execution host as the authority.** One wsnav instance controls
   only the machine on which it is executing. Multi-host use is composition of
   separate ordinary SSH sessions and separate host-local wsnav instances; no
   repository, chat, or task context crosses that boundary.
6. **Fail visibly and conservatively.** Unknown provider identity, runtime
   ownership, provider identity, or host-local observation becomes `unknown`
   or `recovery required`; it is never guessed.
7. **Keep provider history canonical.** Workstream Navigator stores provider
   identifiers needed for exact resume, but no prompts, responses, tool output,
   transcript copies, or rendered-history substitute.
8. **No legacy constraints.** The Python prototype is behavioral evidence only.
   V1 has no schema, command, state, or compatibility obligation to it.
9. **Keep ordinary operation inside the TUI.** After WSNav and its declared
   external prerequisites are installed, a user can perform every ordinary
   WSNav-owned catalog, lifecycle, recovery, and observer action from the
   default Navigator/provider presentation. Direct CLI commands remain optional
   scripting, diagnostics, and break-glass parity, never a required normal
   workflow.

## V1 scope

### Included

- Codex and contract-compatible OpenCode host-local operation through the
  bounded provider-aware launch, exact resume, same-provider Fork, and
  lost-Runtime recovery contract. Historical production acceptance covers
  OpenCode `1.18.11`; the provider contract was revalidated on `1.18.23`.
  Release numbers are diagnostic evidence, not compatibility authority.
- A minimal terminal experience that defaults to the Navigator beside exactly
  one directly interactive surface: the native provider TUI or the provisional
  account shell.
- One always-visible provisional shell card on Workstreams. The selected card
  is headed `Shell`, reports live presentation-local cwd on a stable second
  line, opens a presentation-scoped account shell, and recognizes only an
  explicit brokered `codex` or `opencode` launch as authority to create a
  managed Workstream.
- One current-host registry with read-only capability and observer-readiness
  checks plus contextual readiness guidance.
- Projects represented by one or more Git worktree roots detected and
  registered atomically during successful brokered launch on that host only.
  Project grouping is presentation state and never grants host authority.
- Workstream creation, switching, parking, exact resume, and display through
  the current tip's provider-owned native name when that metadata surface is
  supported.
- Navigator-local Workstreams and Archived pages. Workstreams is the default
  operational home, always groups active Workstreams by Project, and keeps the
  provisional shell card outside those groups. Archived is a separate restore
  page rather than another Workstreams view.
- Reversible Workstream archive and restore for removing inactive work from the
  ordinary navigator without deleting provider history or Git state.
- Independent workstreams started at a registered project root.
- Conversation-forked workstreams that retain the same registered project root.
- Read-only Git-root detection and credential-free origin metadata at brokered
  registration time for host-local Project grouping; no Git lifecycle
  ownership, passive retargeting, or association between separate execution
  hosts.
- Activity and durable result attention for Workstream Navigator-started
  provider sessions.
- Automatic read-only observer readiness detection and contextual Codex
  onboarding when a requested action actually requires an unready observer.
  The guide requires explicit consent before installing or updating one exact
  Navigator-owned profile, opens native trust review without granting trust,
  and resumes the captured intent only after exact readiness and revision
  revalidation. Exact removal remains an exceptional documented cleanup path;
  an accepted provider-owned model prefix survives removal. OpenCode uses the
  separate read-only per-Runtime sidecar contract in D8.1 and has no generic
  onboarding flow.
- Reconnection after local presentation loss. If wsnav is running inside an
  outer operator-established SSH session, a normal detach and reattach to the
  same owned presentation preserves its provisional shell and actual cwd; a
  conclusive loss cleans only exact pre-handoff provisional ownership under the
  shared stable host-private `provisional.lock` lease. Before the helper has
  successfully revalidated every
  bound marker/process/cwd/path/revision/token claim and atomically
  consumed the capability while committing durable `Runtime-owned`
  authority, presentation loss may win only under that lease by atomically
  revoking an unconsumed capability and proving pre-effect absence. After that
  exact helper commit, presentation loss never signals that server; onboarding
  recovery handles any remaining conclusive cleanup. Reconnecting and rerunning
  wsnav on the host reattaches to the same private Runtime/provider in every
  case.
- Recovery after the host tmux runtime disappears, using the provider's native
  session identity.
- TUI access to every ordinary WSNav-owned action, with optional direct CLI
  equivalents for scripting, diagnostics, and recovery.
- Multiple same-user attachment points to one provider runtime, using tmux's
  native shared-screen behavior without a separate input-lease system.

### Explicitly outside V1

- Importing or controlling arbitrary existing provider sessions.
- Passive adoption of a provider process started outside the exact provisional
  shell broker, including process-name, pane-text, hook-only, or session-list
  inference.
- A persisted `Task` entity, assignments, priorities, plans, schedules, queues,
  dependencies, or task-context transfer.
- Automatic plan detection, plan acceptance inference, prompt interception, or
  automatic thread rollover.
- A replacement implementation or altered semantics for Codex `/new`,
  `/clear`, `/fork`, `/rename`, Plan mode, history, settings, permissions, or
  model selection. Navigator Rename is a thin call to the same Codex-owned name
  field, not a separate naming system.
- Composing the WSNav observer with another user-selected Codex `--profile`.
  V1 managed launches preserve the normal base and trusted project
  configuration layers but reserve the one selected profile slot.
- A catch-all global WSNav hook or plugin observer that runs for ordinary Codex
  sessions.
- Transcript storage, transcript rendering, history search, or project memory.
- A custom PTY server, terminal emulator, browser UI, desktop UI, or mobile UI.
- A public network service, always-running remote daemon, or WSNav-owned SSH
  control plane.
- WSNav-managed cross-host operation: registering SSH hosts, opening or
  managing SSH, polling remote snapshots, issuing remote mutations, attaching
  through SSH, bridging remote utility shells, or presenting a unified
  multi-host catalog/attention view. Ordinary SSH composition remains an
  operator workflow outside WSNav.
- Cloning repositories, managing worktrees, synchronizing repositories, moving
  a live workstream between hosts, or transferring chats between hosts or
  providers.
- Launching a managed Workstream outside a valid non-bare Git worktree, or
  changing a Workstream's registered ProjectLocation because the provider
  later changes directories or creates, enters, or removes a worktree.
- Automatic Git fetch, pull, commit, merge, rebase, reset, stash, push,
  cherry-pick, or conflict resolution.
- Copying files, commits, branches, or worktrees between Workstreams.
- Hard deletion of Workstream records, native provider sessions, or project
  files. Archive is visibility and retention, not cleanup authority.
- Automatic installation, upgrade, repository cloning, or host-wide teardown.
- Claude or provider parity beyond the explicitly bounded OpenCode D8 scope.
- Cross-host logical Project grouping or use of repository-origin metadata to
  associate locations owned by separate execution hosts.

The former cross-host behavior is retired by D16. Each host's own
`HostRegistry`, `ProjectLocations`, Workstreams, Runtime generations, provider
bindings, attention, private tmux servers, and provider-owned history remain
authoritative on that host. No workstream, session, project, or provider state
migrates or is copied between hosts.

## Concepts and ownership

| Concept | Meaning | Canonical owner |
| --- | --- | --- |
| `Host` | The machine on which this wsnav instance executes, with tmux, Git, and dynamic provider capabilities | That host's Workstream Navigator registry |
| `Project` | A persisted host-local presentation group of one or more registered `ProjectLocation` roots on the current execution host; it is never action authority | That host's Workstream Navigator registry |
| `ProjectLocation` | One exact non-bare Git worktree root detected from the provisional shell cwd at brokered launch; a linked worktree is its own Location | That host's Workstream Navigator registry |
| `ProvisionalShell` | The one presentation-scoped, non-durable onboarding slot and private shell process, with one preallocated opaque candidate `RuntimeId` and exact final-form `RuntimePaths` fields (directory, socket, configuration, and session), that may be promoted in place by an exact brokered launch | The current presentation controller until the helper successfully revalidates every bound claim, atomically consumes the capability, and commits durable `Runtime-owned` authority |
| Current-host display label | A bounded display-only derivation from a valid operating-system hostname, or `host-<HostId8>` as fallback; never persisted, editable, identity, or action authority | Derived at presentation time from the execution host and its registry identity |
| `Workstream` | One runtime lane and current provider-session binding at its ProjectLocation root | That host's Workstream Navigator registry |
| `Runtime` | One provider process in one private tmux server, session, window, and pane | tmux and live process evidence |
| `ProviderSession` | A provider chat/session referenced by its namespaced native identifier | Native provider |
| `ConversationTip` | The current native session plus its latest accepted settled turn | Workstream Navigator binding plus native provider identities |
| `ThreadName` | The current tip's provider-owned user-facing name; navigator Rename exists only when the adapter exposes that capability | Native provider |
| `AttentionState` | One durable, sticky indication per Workstream that a result or recovery state remains unseen | Workstream Navigator |

V1 deliberately has no `Task` record. Tasks remain what the user asks a
provider to do inside a provider session. A workstream may carry many
successive tasks and many native chats over time without becoming a task
manager.

The Workstream ID is stable; its ConversationTip moves. A verified native
`/clear` or managed cutover may replace thread A with thread B without
replacing the Workstream. Although Codex native `/new` creates a distinct
thread, V1 cannot exact-bind that thread to a running Runtime; it is therefore
unsupported in a managed WSNav provider pane and does not replace the tip. A
Workstream fork creates a new Workstream ID, Runtime, and ConversationTip while
retaining explicit ancestry at the same ProjectLocation root.

There is no separate Workstream label in V1. The current tip's provider-owned
native name is the canonical display name and exact resume still relies on the
namespaced native session ID. The navigator may cache the last observed name
for availability, but it never creates a second naming authority or claims
Rename support when the provider adapter lacks it.

## Architecture

```text
operator terminal on the execution host
└── dedicated host-local tmux presentation session (disposable)
    ├── navigator pane
    │   └── wsnav TUI with one pinned provisional-shell card
    └── active pane
        └── either
            ├── wsnav attach helper -> exact Runtime tmux server
            │                                      └── native provider TUI
            └── one private provisional shell server
                └── account shell with broker-owned provider functions

wsnav TUI
├── current-host registry projection
├── contextual provider-readiness guidance
└── host-local action and attachment boundary

the execution host
├── private SQLite state
├── one private tmux server per live workstream runtime
│   └── exactly one session, window, and provider pane
├── at most one presentation-scoped provisional shell server
│   └── promotable in place to one managed Runtime
├── in-process local application facade for navigator and public CLI
├── short-lived per-operation provider metadata helpers
└── observation scoped to managed Runtimes
    ├── Codex hooks active only in wsnav-started sessions
    └── D8.1: one host-owned OpenCode sidecar per Runtime generation
```

To use another machine, the operator establishes ordinary SSH in a separate
terminal/tab/window and runs `wsnav` on that machine. That composition is
outside this diagram and outside WSNav control; there is no unified
multi-host catalog or attention view.

### Presentation layer

The host-local presentation session is a dedicated tmux server with its own socket
and configuration. It never modifies or depends on the user's ordinary tmux
server.

Before starting that server, WSNav creates a mode-`0700` presentation directory,
a mode-`0600` fixed configuration, and a bounded private `ownership.json` that
binds their exact identities to the generated session and socket paths. Once
tmux creates the socket, WSNav records its exact identity in that same owned
marker. Reopen and close revalidate the marker, configuration, socket, directory,
and a bounded filename allowlist. Close unlinks only those exact owned artifacts
and removes the then-empty directory; it never recursively deletes a presentation
tree. A missing, changed, symlinked, foreign, malformed, or newly added artifact
fails closed and remains untouched.

That allowlist admits one presentation-private provisional marker. It
records only the candidate RuntimeId, exact final-form `RuntimePaths` fields
(directory, socket, configuration, and session), seed cwd, presentation/slot
identity, fresh `slot_generation`, and bounded shell/server/process ownership
evidence. It is
revalidated with the onboarding journal, presentation revision, and registry
generation under the shared
stable host-private `provisional.lock` lease. The marker is the provisional
cleanup authority until the helper successfully revalidates every bound claim,
atomically consumes the capability, and commits durable `Runtime-owned`
authority; it is never a durable
Runtime or Workstream record.

The initial presentation sets the navigator to its normal 32-cell width and
gives every remaining terminal column to the provider pane. A detached tmux
server begins at a default size and proportionally redistributes panes when its
first real client supplies the terminal dimensions. The private presentation
therefore installs exact `client-attached` and `window-resized` hooks that
resize only its Navigator pane; the Rust TUI also retains its resize correction
as a defensive path.

Those outer hooks do not by themselves establish the native provider's initial
geometry. Each provider starts inside a detached private Runtime tmux window,
so its first render otherwise uses tmux's default dimensions and may race the
first nested client resize. Immediately before attaching a real terminal, the
presentation owner pre-sizes its exact owned window from that terminal and the
Runtime owner pre-sizes its exact owned window from the provider attachment
PTY. Each window then returns to tmux's `window-size latest` policy so later
native resize propagation remains unchanged. This handshake stores no geometry,
touches no ordinary tmux server, and neither captures nor injects provider
terminal bytes. Individual renderers retain their compact fallbacks for
explicitly narrowed panes.

The presentation has exactly the Navigator and one active pane. The active pane
is either the selected managed Runtime attachment or the presentation's
provisional account shell. Selecting another card replaces that exact surface;
WSNav exposes no third pane or split-shell action. Unknown or duplicate
pane-role evidence is ambiguity and must leave the layout unchanged.

The private presentation does not inherit tmux's general-purpose prefix or root
management tables. Its prefix table is rebuilt as an explicit allowlist:
`Ctrl+b d` detaches; `Ctrl+b o` and directional keys move among the two owned
panes; `Ctrl+b Ctrl+b` delivers a literal `Ctrl+b` to the focused application
without exposing the nested Runtime's tmux prefix table; and `Ctrl+b ?` shows
only this curated help. The retired D12 `Ctrl+b "`, `Ctrl+b %`, and `Ctrl+b x`
bindings are explicitly absent, including when an older live presentation is
reattached. The root table retains only the primary mouse
selection/forwarding and bounded scrolling/copy interactions required by the
Navigator and active native surface. Default right-click management menus,
mouse split/swap/kill/respawn actions, arbitrary tmux command prompts,
additional splits, windows, sessions, and layout mutation bindings are absent.
These restrictions belong only to WSNav's private presentation server and never
modify the user's ordinary tmux server or configuration.

Both private tmux layers also own their copy-mode wheel behavior. They bind
`WheelUpPane` and `WheelDownPane` in the `copy-mode` and `copy-mode-vi` tables
to one line per event instead of tmux's five-line default. This changes only
tmux-owned history navigation: the presentation root table still forwards
wheel events through nested alternate-screen clients, and a native provider
that owns its alternate-screen scrolling retains its own behavior.

WSNav does not source, parse, or execute the user's ordinary tmux
configuration. A tmux configuration is an executable command stream that may
install hooks, plugins, shell commands, or topology-changing bindings, so it
cannot be treated as a safe preference document and then repaired by later
overrides. Newly created private servers receive the fixed copy-mode profile
from one shared source of truth. Immediately before attachment, the Runtime
owner idempotently reapplies only those four fixed bindings through the exact
owned socket so a Runtime created by an older WSNav build converges without a
provider restart. This reconciliation reads no key table or pane content,
touches no ordinary tmux server, and adds no user configuration, durable state,
protocol, or provider-input surface.

A possible extended feature, outside the current V1 contract and not yet an
approved roadmap checkpoint, is selective user tmux preference import. It
would not source or execute the user's configuration. A future study may use
tmux's parse-only verbose mode in a disposable private parser server, convert
only explicitly supported command shapes into bounded typed values, and then
generate the same WSNav-owned private profiles. An initial allowlist could be
limited to `mode-keys` and one consistent wheel repeat count across all four
copy-mode bindings. Unknown, executable, included, conditional, conflicting,
or malformed input would have no effect and would fall back to WSNav defaults;
raw configuration and parser output would not be persisted. Before this can
enter the roadmap, disposable evidence must settle supported tmux versions,
user-config path resolution, include and conditional behavior, bounded output,
host-local preference ownership, change detection for live Runtimes,
and fail-closed preservation of every private topology and input boundary.

The navigator is a small Rust TUI in one pane. The provider pane is not a
terminal widget rendered by Rust; it is a real tmux attachment to the host-owned
provider runtime. This retains direct keyboard, mouse, resize, color, and native
TUI behavior without building a PTY server or terminal emulator.

The provisional account shell is the sole account-shell surface. The retired
below-provider utility shell remains historical evidence only; its
split/focus/close controls and compatibility cleanup are not part of the
current product.

When this presentation is itself running inside an ordinary operator SSH
session, an outer disconnect may end or detach the disposable presentation and
its shell. It must not stop, park, rotate, or restart the host's private
Runtime or provider. Reconnect to that host, rerun `wsnav`, and attach again.

Exactly one provisional card is always visible on Workstreams, pinned outside
Project groups. At presentation creation, WSNav captures, validates, and
canonicalizes the invocation cwd as that presentation's private seed cwd.
After the fresh presentation has created and proven both owned panes, startup
selects the card, materializes exactly one opaque candidate `RuntimeId` and
fresh opaque `slot_generation`, and shows that account shell in the active pane.
It creates the
provisional tmux directory, socket, configuration, and session using the
existing final full-UUID `RuntimePaths` fields (directory, socket,
configuration, and session) for that candidate. The candidate
ID and exact final-form `RuntimePaths` fields (directory, socket,
configuration, and session), together with the shell and server ownership
evidence, live only in the presentation-private marker. They do not create a
registry `Runtime` or `Workstream` row. Before creating those artifacts,
materialization proves the candidate ID and all four path fields are absent and
unused; it never adopts pre-existing artifacts. A marker-backed candidate is
outside ordinary registry inventory, probe, park, remove, and recovery
discovery/action until durable adoption; only the exact presentation marker
plus the stable host-private `provisional.lock` lease may manage it.
Markerless/registryless, foreign, or collision artifacts remain untouched, and a
clean replacement allocates a fresh candidate RuntimeId. Every newly materialized clean provisional shell in that
presentation starts at the seed cwd; an existing shell keeps its actual cwd
across detach and reattach, and a new presentation captures its own seed. A
missing, deleted, unsafe, or ambiguous seed cwd makes onboarding unavailable
with bounded guidance; it never falls back or becomes Project authority.

The provisional slot has one serialized ownership handoff shared by lazy
materialization, confirmed close/loss cleanup, the prepare broker, and the
launch helper. Each participant acquires the stable host-private
`provisional.lock` lease, revalidates the marker, presentation revision, and
registry generation, and releases the lease only after its state transition is
complete.
The marker, capability, and onboarding journal bind both the lock's
`lease_generation` and the presentation/slot `slot_generation`.
Materialization owns only the marker-backed provisional artifacts. The prepare
broker, while holding the `provisional.lock` lease, validates the live shell and
broker cwd,
detects the exact non-bare Git worktree root, transactionally generates and
reserves the durable Runtime generation, adopts that exact candidate
`RuntimeId` and unchanged final-form `RuntimePaths` fields (directory, socket,
configuration, and session), records the durable graph and request journal, and
marks the handoff issued. It binds every claim to that candidate; it does not
rename, rehome, or replace a live tmux server.

The prepared reservation does not by itself revoke provisional cleanup. Before
the helper's successful revalidation and atomic capability consume plus durable
`Runtime-owned` commit, a confirmed close or conclusive loss may win only when
it acquires the same `provisional.lock` lease, rechecks the marker and journal,
and atomically cancels/revokes an issued but unconsumed capability, then proves
the provider effect is absent. It then rolls back attempt-only graph rows and cleans the
exact provisional process group, pane, server, and marker. The helper instead
reacquires `provisional.lock` and, while holding it, revalidates every bound
marker/process/cwd/path/revision/token claim. Only on successful revalidation
does it atomically compare-and-consume the capability and commit durable
`Runtime-owned` authority for the candidate; a mismatch does not advance
ownership. It then, still under `provisional.lock` and before releasing it,
revokes/removes presentation cleanup authority, with durable transition
preceding marker cleanup, and presentation close/loss never signals
the pane, process, or server after that exact commit, regardless of provider
binding success. Ambiguous cross-store crash windows remain in the onboarding
journal for reconciliation; conclusive pre-effect rollback/cleanup after
transfer belongs to onboarding recovery, not presentation cleanup.

Before that exact helper commit, the selected card remains the exact shell even when
the broker has prepared a reservation. Once Runtime ownership commits, that
same selected card becomes the managed Workstream and the UI derives one fresh,
unmaterialized provisional singleton card immediately, even when native binding is still
unavailable. OpenCode provider success does not decide card or server
ownership: a possible `POST /session` effect leaves the same server Runtime-
owned and the card visibly `recovery-required`, even if no native TUI remains.
A conclusive pre-effect failure after that exact helper commit is classified by
onboarding recovery; it rolls back attempt-only graph state only when
provider-specific evidence proves no effect or binding, leaving the derived
singleton card available but unmaterialized. An ambiguous-effect slot is never
reusable and never issues a second POST.

The lifecycle therefore has these observable rules:

- A normal tmux client detach, followed by reattachment to the same owned
  presentation, preserves the exact provisional shell server, pane, process,
  actual cwd, and pending request state. It does not create a second shell. The
  same rule applies when an outer operator SSH connection detaches while the
  private presentation remains alive.
- A confirmed presentation close or conclusive presentation loss uses the
  shared `provisional.lock` lease and marker/journal checks above. It cleans only exact
  pre-handoff provisional ownership when the lease atomically revokes any
  unconsumed capability and proves absence; after the helper successfully
  revalidates every bound marker/process/cwd/path/revision/token claim and
  atomically consumes the capability while committing durable `Runtime-owned`
  authority, it never targets that server. A possible provider effect remains
  visible for recovery rather than being hidden by close.
- A shell exit or conclusive pre-effect launch failure follows onboarding
  recovery's clean-replacement path. It does not make a provider process
  unmanaged or silently recreate a replacement server.
- Missing, changed, symlinked, foreign, malformed, or otherwise ambiguous
  marker, lease, path, process, or revision evidence is left untouched. WSNav
  fails closed, marks onboarding unavailable with bounded guidance, and blocks
  duplicate provisional creation until that exact evidence is resolved. It
  never stops, parks, rotates, or cleans a managed Runtime.

Disposable acceptance must race close and presentation loss against lazy materialization,
prepare and token issuance, helper consumption, OpenCode preparation and
`POST /session`, and provider `exec`; it must also race passive snapshot,
new attachment, Park/Resume/Fork/contextual `n`/archive/
rename/recovery/start retry, helper exit, exec error, exec success proof,
immediate provider exit, and restart across every post-commit phase. The
evidence must show one deterministic lease winner, no managed kill, no helper
adoption, no premature signal or action, no stuck operation, no blind rollback,
no duplicate ownership, no duplicate shell, and no second OpenCode POST.

### Provisional lock and singleton reconciliation

The serialized presentation/slot handoff uses one stable host-private
`provisional.lock`. Direct schema-15 bootstrap precommits its generation and
installs the exact mode-`0600`, current-owner, create-new/no-follow inode
before the bootstrap becomes `ready`. Host operational metadata records the
generation and expected device/inode; `bootstrap.lock`, the database, and the
provisional file must agree before current state opens.

The file contains only a bounded format version, HostId, and lease generation.
It contains no cwd, command, argv, provider/user content, or provider payload.
A missing, malformed, symlinked, foreign, replaced, mismatched, or busy ready
lock fails closed and is never recreated or adopted. Current operation never
unlinks it, and old roots cannot introduce it through migration.


Every materializer, prepare broker, launch helper, confirmed close/loss cleanup,
and singleton reconciler opens `provisional.lock` with no-follow/CLOEXEC,
acquires one nonblocking exclusive kernel lock, and retains that FD through its
mutation. Before mutation it binds and revalidates the canonical root identity,
pathname, and open-FD device/inode identity. A process crash releases the kernel
lock without changing the file; restart reacquires the same artifact and
reconciles the marker and journal. The FD cannot leak across provider `exec`.
A bounded busy or timeout returns onboarding guidance, never creates a second
lock or proceeds unlocked. The marker, capability, and journal bind the
`lease_generation` plus `slot_generation`; this lock is host operational state,
not presentation-private storage.

Each presentation derives one pinned provisional shell card with no durable
card row. Across the host, the shared `provisional.lock` and classifier permit
at most one unregistered materialized provisional candidate server. Under that
lock, each lazy materialization mints a fresh opaque `slot_generation` and
candidate `RuntimeId` in the exact marker, and the capability and journal bind
both. A valid marker/artifact owned by another presentation is recognized as
busy/owned, not unknown or adoptable; that presentation's card remains visible
but unavailable until the slot promotes or conclusively cleans. A bounded
provisional classifier cross-checks
the marker and unfinished onboarding operations against registered Runtime IDs
and the bounded `run/runtime-*` namespace. It may identify names and ownership
only to detect conflicts; it never passively adopts or deletes unknown artifacts.
Missing or changed marker evidence combined with any unregistered
Runtime-shaped artifact, multiple candidates, or ambiguous journal/path/process
evidence blocks all fresh materialization and leaves artifacts untouched. It
cannot evade ambiguity by choosing a new UUID. A fresh candidate is permitted
only after exact prior artifacts are proven absent or conclusively cleaned;
collision/foreign artifacts block, and clean replacement always gets a new
slot generation and candidate RuntimeId.

At Runtime ownership commit, the old slot generation is consumed and the UI
derives one fresh unmaterialized card. If onboarding later rolls back, the
lease-held reconciler targets only the old operation, Runtime, and slot
generation; it never creates a second card, resets or closes a newly materialized
shell, or targets a newer marker. If a fresh card/marker already exists it is
left unchanged; if none exists, the ordinary derived singleton card is enough
and remains unmaterialized until selected. Recovery is revision- and
slot-generation-guarded and idempotent across restart.

### Post-commit launch fence and reconciliation

The helper's successful claim revalidation and atomic capability
compare-and-consume commit durable Runtime ownership and revoke presentation
cleanup, but do not yet make the Runtime an ordinary attachable or actionable
provider Runtime. The same request-keyed `CompoundOperation` therefore advances
through explicit bounded phases: `runtime_owned_launching` (no provider effect),
provider-specific preparation and external-effect phases, `provider_exec_started`
immediately before the final `execve`, and terminal `provider_exec_proven`, a
known-absent exec failure, or `recovery-required`/`unknown`. These phases are
durable distinctions, not display hints.

While the operation is Runtime-owned and its launch remains unresolved,
attachment and action authority for that unproven Runtime remains fenced. Its
originating
presentation may retain its already-existing tmux Runtime attachment/pane or
detach through ordinary card switching; neither creates a new attachment to
that Runtime. Selecting/materializing the fresh derived singleton card attaches
only its separate provisional server under `provisional.lock` and grants no
authority over the unproven Runtime. Every new attachment to that Runtime and
ordinary Runtime action or mutation—Park, Resume, Fork, contextual
`n` from this source, archive, Rename, recovery/start retry,
and cleanup—refuses or waits with bounded `onboarding-in-progress` guidance.
Passive snapshot/probe may render the managed Runtime as `starting`/`onboarding`
and run exact reconciliation, but it must not treat the hidden helper or
OpenCode preparation process as provider identity, mark the Runtime lost from
that mismatch, signal it, or expose normal action authority. Once an operation
is terminal `recovery-required`, only the existing exact recovery or explicit
Park rules apply. A terminal known-absent exec result is not itself action
authority: the reconciler must atomically resolve it. When the provider-specific
journal proves no prior external effect or binding, guarded rollback ends
onboarding and leaves the derived singleton card available but unmaterialized.
When OpenCode has a known blank-session POST or binding, the same atomic
resolution ends onboarding in the exact stopped/recovery state; only
binding-preserving Resume/recovery or explicit Park is then allowed. A possible
effect remains `recovery-required`. No ordinary action is enabled directly by
exec-error evidence, and no operation remains fenced after terminal
reconciliation.

The hidden helper durably advances the operation to `provider_exec_started`
immediately before `execve`. If `execve` returns an exact error, it records a
terminal known-absent exec failure before exiting when possible. A crash after
`provider_exec_started` without proof is ambiguous and is never rollback
authority. Because successful `execve` never returns, a bounded host-local
onboarding reconciler, invoked during passive snapshot, action preflight, and
restart recovery, owns success proof without performing provider effects. It
revalidates the exact operation/revisions, RuntimeId/generation and exact
`RuntimePaths` fields (directory, socket, configuration, and session), tmux
pane/session, the same PID/birth/PGID/session, and
the expected provider executable; only full proof atomically commits
`provider_exec_proven` and activates ordinary Runtime attachment/action
authority. An authoritative Codex hook may contribute evidence only through
that same identity/revision proof; an OpenCode sidecar or server identity is
never native-TUI exec proof. If the expected provider disappears before proof,
an exact helper-recorded `execve` error classifies only the final provider TUI
exec as known-absent. Attempt-only graph rollback is allowed only when the
provider-specific journal also conclusively proves no prior external effect or
binding; a possible effect is `recovery-required`, and a possibly live provider
is never rolled back.

Codex may reach `provider_exec_proven` while its Workstream remains managed
`starting` and unbound until the first `SessionStart`. For OpenCode, a known
blank-session POST or binding is retained on the same Runtime/Workstream and
enters the exact recovery/resume state if final TUI exec fails; it is never
rolled back and never issues a second POST. A possible POST effect remains
`recovery-required`.

For OpenCode, executable proof alone is not final activation proof.
Before the final native exec, the helper persists the exact loopback
endpoint/version/session handle that the already-fenced blank-session creation
returned; that handle is not observer authority while the pane still contains
the account shell. After the passive reconciler proves the final native
executable and records the unchanged pane PID/birth, it retains
`provider_exec_started` and starts one detached, presentation-independent
observer for that exact Runtime generation. The observer may perform its first
health, root-session, and SSE reads only after those durable PID/birth, handle,
and generation checks all agree. It records `Ready` under the same exact
identity, and only then does the reconciler commit `provider_exec_proven` and
make the Runtime attachable/actionable. An observer that is missing, changed,
unknown, exits before Ready, or cannot establish the exact endpoint/session
leaves the Workstream action-fenced for explicit recovery; it never permits a
blind attach, a second session POST, or an inferred observer replacement. The
helper does not try to start this long-lived observer after `execve` from the
provider pane: the presentation/controller owns the staged post-exec
handoff and must clean an observer-start failure without signalling the native
provider.

### Account-shell bootstrap and broker handshake

WSNav supports Bash and Zsh interactive non-login shells only. The launcher
rejects login-shell mode before it starts either shell: interactive login Bash
does not load a supplied `--rcfile`, so a Bash wrapper cannot be the enforcement
point. A later nested login shell bypasses the controlled function and remains
unmanaged. WSNav starts the provisional slot with a shell-specific private
wrapper startup file while preserving the validated presentation environment,
original `HOME`, and, for Zsh, original `ZDOTDIR`. The wrapper reproduces that
shell's ordinary non-login interactive startup graph, including system/user
ordering, exactly once; it does not select a vaguely named RC file or promise
arbitrary login-shell mode. Observable environment, options, aliases,
functions, and prompt readiness must match an ordinary disposable baseline
except bounded wrapper state and the intentional provider interception. The
wrapper then removes any `codex` and `opencode` aliases or functions before
installing the exact WSNav-owned functions. WSNav never parses, stores, or
modifies ordinary RC contents. Startup abort, an `exec` that replaces the
wrapper, or any ambiguous startup context leaves the card visible but
unavailable with bounded guidance; it does not expose a partially intercepted
shell.

When a controlled function receives a provider invocation, it first applies
the provider adapter's closed command grammar. Only a proven fresh
interactive native TUI shape can be promoted. For that shape, the function
invokes a bounded prepare broker as a child over presentation-private,
non-terminal control I/O. A pre-effect refusal or user cancellation therefore
returns to this exact interactive shell. The prepare broker validates the
request, detects and validates the Git root, and journals/reserves the durable
operation before returning only an exact one-shot opaque launch capability over
that private channel. It never returns a provider command string or argument
vector.

The returned value is an exact one-shot capability, not a reusable bounded
token. Its claims bind the request/operation key, presentation identity,
provisional-slot identity, candidate `RuntimeId`, exact final-form `RuntimePaths`
fields (directory, socket, configuration, and session), fixed provider, shell
cwd, detected worktree root and ProjectLocation, reserved Runtime generation,
captured presentation revision and registry generation, shell leader PID/birth/process-
group identity, a digest of the already grammar-approved bounded argv, and a
short monotonic expiry. Expiry uses one host monotonic-clock provenance; after
a restart or clock-provenance ambiguity the capability is expired rather than
reused. The journal persists only a bounded token identifier/verifier, those
claim references or digests, expiry, and phase; it never persists the live token
or original argv. Secret-bearing argv is outside the promotable grammar and
never enters this capability.

The function then `exec`s one hidden WSNav launch helper with the one-shot
capability and the original bounded argument vector. Before any provider
effect, the helper reacquires the exact stable host-private `provisional.lock`
lease and,
while holding it, revalidates every bound marker/process/cwd/path/revision/token
claim: marker and presentation/provisional-slot identity, candidate `RuntimeId`,
each `RuntimePaths` field (directory, socket, configuration, and session), token
verifier and request/operation, provider, cwd/root/Location, Runtime generation,
captured revisions, shell PID/birth/process group, argv digest, and monotonic
expiry. Only when every claim revalidates does it atomically compare-and-consume
the capability and commit durable `Runtime-owned` authority for that candidate.
A replay, expiry, duplicate helper, or any mismatch fails before provider effect
and does not advance Runtime ownership. After that successful commit, the
helper, still under `provisional.lock` and before releasing it, revokes/removes
presentation cleanup authority; durable transition precedes marker cleanup.
Only afterward does it construct the provider argument vector internally
(including any WSNav-owned provider flags), prepare provider
effects, and `exec` the provider. No provider command text crosses the shell
boundary. The two `exec` steps preserve the shell leader's PID, birth token,
and process group as the final provider identity. Broker control traffic remains
outside the provider pane. Issuance-to-helper cancellation or crash, helper
crash after consume, and all rollback/recovery gaps are journaled: a conclusive
pre-effect absence plus provider-specific proof of no external effect or
binding may roll back the graph. An exact `execve` error alone proves only
absence of the final provider TUI exec; any possible post-effect result remains
a visible recovery-required operation and cannot become a blind clean retry.

This two-phase prepare-token-helper variant is routed by the current binary.
[Spike 0021](evidence/spikes/0021-d17-two-phase-handshake.md) validates its
narrow synthetic mechanical boundary across Bash/Zsh and both provider routes:
direct prepare child, one-shot verifier-backed capability, exact claim
comparison, shell identity preservation, and lease-FD noninheritance. Those
spikes remain historical model evidence rather than proof of the production
composition. The routed implementation and disposable tests now cover
conclusive pre-provider shell/handoff recovery and the required cross-actor
race matrix. Historical live provider acceptance is recorded in the D17 evidence.
[Spike
0022](evidence/spikes/0022-d17-account-shell-wrapper.md) validates the
non-login account wrapper and Bash login preflight, while [Spike
0023](evidence/spikes/0023-d17-provisional-lock.md) validates the isolated
historical provisional-lock lifecycle.

The command grammar is closed and provider-specific. Broker-owned or
identity-changing cwd, profile, resume, session, attach, server, host, port,
endpoint, or equivalent flags are rejected before reservation; they are never
silently stripped or reinterpreted. Explicitly enumerated provider-owned
non-session commands such as `--help`, `--version`, and `login` may be passed
directly to the real provider as explicitly unmanaged commands and return to
the shell; their effects remain provider-owned. Other execution or subcommand
shapes refuse with bounded guidance to use an ordinary terminal or an explicit
bypass. Any secret-bearing argument or value is outside the promotable
grammar. Safe native model, effort, permission, and similar
options are admitted only when the adapter's version/contract validation and
tests prove them compatible; WSNav does not invent a fixed live-version flag
list.

User redefinition of a controlled function, `command`, an absolute provider
path, a differently named binary, a nested shell, or a script is an explicit
unmanaged bypass. Process-name observation, terminal text, hooks, session
inventory, and a provider launched after such a bypass are never promotion
authority. WSNav does not kill or adopt that process. Provider exit never
converts a managed card back into a shell: the card remains stopped or
recovery-required, and completed provider output stays visible until the user
acts.

Only account shells whose exact wrapper, function, token handoff, signal, and
`exec` behavior passes the current Bash, Zsh, and implementation tests are
eligible for managed onboarding. Unsupported or ambiguous shells leave the
card visible but unavailable with bounded guidance. This does not authorize
ordinary RC parsing, provider-command aliases, passive provider detection, or
a general-purpose tmux surface.

The dedicated tmux status line stays disabled because it consumes a row from
the provider surface. Navigation and status live in the navigator pane.

The navigator footer reserves separate space for status and controls. When
there is a warning, progress update, or action outcome, it appears in a
bordered `Status` box with at most three wrapped content lines directly above
the persistent contextual key strip. Ordinary grouping state is not repeated
there. The box never replaces the controls below it. The key strip keeps
single-key terminal actions first-class and wraps only at complete action
boundaries into at most two compact lines. It never lets terminal wrapping mix
two bindings. On a terminal too short to preserve useful content, the strip
collapses to `? keys`. It lists only distinctive actions: ordinary
Enter/Esc behavior remains native terminal convention rather than consuming
permanent hint space. Related actions remain adjacent in each page's strip. A
one-cell inset keeps the hints visually separate from the Workstream list.

`?` toggles an expanded shortcut reference at the bottom of the Ratatui
navigator pane. The reference is page-specific and single-column, with one
keyboard action per line. It omits mouse and self-closing reminders so ordinary
pages fit without scrolling at the standard navigator height. It still scrolls
if a terminal is unusually short rather than pairing or wrapping entries into
each other. `?`, `Esc`, or `q` collapses it. This is not a tmux popup, window,
centered overlay, or provider overlay.
Shortcut descriptions begin at one display-cell-aware column regardless of key
label width and are bounded to the remaining 19 cells of the normal 30-cell
inner pane. The reference advertises `↑/↓` as the canonical selection keys;
`j/k` remain accepted compatibility aliases but do not consume help width.
While expanded, all other navigator keyboard and mouse actions are inert, so
help cannot accidentally activate or mutate a Workstream.

The navigator pane has one Workstreams home page and one infrequent direct page
rather than a generic management landing page:

```text
Workstreams
├── Shell
├── Project-grouped active Workstreams
└── Recovery

Archived
└── Project-grouped archived Workstreams and Restore
```

Workstreams is the default page and retains the product's ordinary switching
workflow. `.` opens Archived; pressing `.` again, or `Esc`, returns to
Workstreams. These are direct pages, not members of a cyclable view mode, and
there is no persistent tab bar. Workstreams has no Recent, Projects, or By-host
projection and `Left`/`Right` do not change pages or grouping. Provider
readiness is not a page or manual setup mode: the navigator detects it
read-only and offers contextual guidance only when the brokered provider launch
needs missing readiness.

The bounded current-host display label uses deterministic precedence: a valid,
trimmed operating-system hostname, then `host-<HostId8>`, where `HostId8` is
the first eight lowercase hexadecimal digits of the UUID with separators
removed. The hostname is accepted only when its UTF-8 value is single-line,
contains no Unicode control or format characters, and is at most 64 Unicode
scalar values; it is never silently rewritten or truncated. There is no
configured label, persistence field, settings page, or label mutation action.
The derived label is application metadata only: it never selects a registry,
authorizes an action, enters a shell command, or appears inside provider
content. The reduced navigator does not repeat it in ordinary cards or pages;
each instance is structurally host-local and the containing terminal or SSH
window supplies machine context. Workstream titles remain neutral white,
provider and Project identity use distinct stable accents,
lifecycle/attention/recovery indicators retain their reserved state colors,
and activity ages use a neutral brightness ramp. Selection changes only the
row background. Chromeless direct attach likewise relies on operator terminal
context. No page creates a tmux popup, overlays the provider pane, or replaces
the native TUI.

Direct page-local keys are the canonical control path. The compact footer
shows the most relevant bindings for the current page and state; `?` reveals
the complete list. The management lists provide bounded status and context
inline, but D7 does not require a menu-driven action system. A later clickable action menu may
augment the same operations without replacing or delaying the direct keys.
Each stateful action introduces its own bounded text entry, confirmation, and
progress state with the authority that consumes it; the navigator does not keep
an unconnected generic modal that could imply an action is available before its
host contract exists. There is no Project browser, browser-root setting,
repository-registration form, or manual metadata-refresh action.
ProjectLocation registration is a bounded host operation inside successful
brokered promotion, and the shell remains the user's familiar path-selection
surface.

The Workstreams page retains its accepted muscle memory: `Enter` attaches a
live Runtime, starts/resumes a parked or stopped one, or enters exact native
recovery for an ordinary lost Runtime. `n` starts a sibling at the selected
managed Workstream's exact ProjectLocation with the same provider; `f` forks;
`p` parks; `x` opens the reversible archive confirmation; `a` acknowledges
only result attention; `r` opens bounded Codex Rename or recovers the selected
unresolved Fork; and `?` toggles the full reference. `u` on Archived restores
without starting. On the provisional shell card, `Enter` opens or focuses the
shell and `n` has no separate meaning. `Left` and `Right` remain inert. There
is no hard delete: archive/restore is the sole Workstream visibility control.

Workstreams always groups active Workstreams by Project. Groups sort by their
newest included member's durable `last_activity_sequence`, descending, with
opaque ProjectId as tie-breaker; children sort by that sequence descending,
then opaque WorkstreamId.
Headers are non-actionable display rows; selection, mouse activation, and
provider attachment remain exact Workstream operations. Archived uses the
same deterministic grouping and ordering over archived members. `u` restores the
selected Workstream, returns to Workstreams, and selects it without starting,
resuming, or attaching a provider. Page selection is process-local
presentation state and is never persisted.

The navigator assumes horizontal space is scarce and spends vertical space to
keep rows scannable. Each Project-grouped Workstream is a compact two-line tree
child. Its first line places the provider at the left and relative activity age
at the right edge. Its second line reserves only the minimal continuation and
lifecycle marker, giving the remaining width to the native thread name.

Provider names use stable provider-specific accents rather than the white used
for Workstream titles. Project labels retain their own identity palette; the
host name is not repeated. Green, yellow, and red remain lifecycle colors. Age
is right-aligned when space permits; the provider and age truncate safely at
pathologically narrow widths without allowing a card to overflow. Every
display line in a card is one selectable and mouse-actionable Workstream row.
Archived uses the same compact row shape as Workstreams.

A Parked Workstream always renders the muted `p` lifecycle marker; sticky
result or recovery attention remains durable but does not mask the parked
lifecycle. Bounded prose in status and guidance panels word-wraps by terminal
cell width. The status area reserves the wrapped line count, and the renderer
and mouse hit-testing use the same resulting list geometry.

Project group headers use the accented Project name alone: no disclosure
marker, Location count, active count, or archived count consumes that line.
The provisional shell card is not rendered under a Project header and uses no
persisted Location label. Its first line is exactly `Shell`. Its stable second
line renders the cwd with home as `~`, abbreviated parent components, and the
leaf folder whole. The value refreshes when the exact live cwd changes and is
bounded, cell-aware, presentation-local display evidence; it is never persisted
and never becomes registration, launch, or retargeting authority.
WSNav persists the exact containing worktree root only at promotion and does
not follow or manage later branch/worktree changes. The footer owns action
discovery.

Footer hints are laid out as indivisible key/action pairs and packed across the
number of physical lines required by the current pane width; they are never
passed to prose wrapping or silently clipped at the ordinary 32-cell navigator
width. Page help likewise uses a compact key column and concise action column,
with colored keys plus semantic action colors. Its copy is designed to fit the
bounded navigator width, and only pathological widths use cell-aware
truncation rather than breaking words or alignment.

The grouped view renders an explicit minimal tree instead of communicating
hierarchy through indentation alone:

```text
workstream-navigator
├ Codex                                     3 min ago
│ ✓ lifecycle repair
└ OpenCode                                   1 day ago
  p later follow-up
```

Tree branch and continuation glyphs are structural, neutral-colored chrome.
They do not become lifecycle indicators, selection targets, or identity. A
group header remains non-actionable; either line of a child resolves to the
same exact host-local Workstream identity. When no native thread name is
available, the title line shows only the stable short Workstream ID; it does not
spend width on a synthetic `Workstream` prefix.

The navigator uses deliberately quiet provider and Project identity accents. A
deterministic collision-resolved muted 256-color Project label distinguishes up
to twelve concurrently visible Projects without coloring the Workstream title
or whole row. Selection changes only the row background. Green, yellow, and red
remain reserved for completed, working, and recovery/error state, so color
never becomes action authority or pulls focus from the native provider pane.

Switching workstreams replaces only the provider pane's attachment helper. It
does not stop, restart, type into, or resize an inactive provider process beyond
the normal detach/attach terminal negotiation. A currently Running attachment
is therefore replaceable rather than treated as an in-progress Start; only an
actual AwaitRuntime transition remains serialized. When detach or Park leaves
the exact owned provider helper pane dead under tmux `remain-on-exit`, the
replacement path may respawn that pane in place only after revalidating the
single owned window, live navigator, exact roles, and bounded utility cleanup.
Other dead or ambiguous presentation topology remains a refusal.

AwaitRuntime serializes the Start operation; for an already-managed Runtime it
does not require the durable lifecycle to leave `Starting` before native
attachment. Once the exact owned private Runtime record and live process
identity exist, the navigator may attach while its status is `Starting`, but
only when no onboarding operation remains unfinished and
`provider_exec_proven` has committed. A Runtime still in
`runtime_owned_launching`, provider preparation/external effect, or
`provider_exec_started` is not attachable merely from its record/process
identity: only the originating presentation may retain its existing pane, which
is not a new attachment. The provider's later native SessionStart observation
confirms lifecycle progress; it is not a prerequisite after that proof for
giving the provider its terminal client. `Stopped`, `Unknown`, missing, or
identity-changed Runtime evidence still refuses or waits without retargeting.
Because that native lifecycle observation can advance the Runtime and
Workstream revisions between the navigator snapshot and attachment preflight,
one stale-revision result may take one fresh passive snapshot and retry once
only when both opaque IDs are unchanged and the Runtime remains attachable. A
second revision change, Runtime rotation, archival, or unavailable snapshot
still refuses without provider-pane mutation.

Each replacement gets one presentation-private attempt ID and a mode-`0600`
pending/running/completed/failed status file. The attachment helper updates
that file, never the provider pane. The navigator clears its non-durable
attachment marker when the helper completes or fails and permits an exact
same-row retry; a helper pane that dies before reporting a terminal phase is
also classified as failed. These files disappear with the disposable
presentation and contain only the Workstream ID, attempt ID, and phase. The
current host is implicit in the presentation's selected state root.

Focus is local presentation state, not durable Workstream state. A primary
mouse click on any line of a Workstream card selects that exact Workstream and
switches the provider pane to its normal open/start/recover attachment while
retaining keyboard focus in the Navigator. The user explicitly enters the
native provider with `Enter`, `Tab`, or a click in the provider pane. This lets
successive card clicks browse live Workstreams without transferring keyboard
control on every selection; it does not create a passive preview mode or alter
the selected Workstream's lifecycle action. Two navigator clients may look at
different workstreams without racing over a global `current` record. Durable
state records activity and attention, never an authoritative focused pane.

A provider Runtime may have more than one same-user tmux attachment, including
another Workstream Navigator client or a deliberate direct attachment to its
private socket. tmux mirrors the same screen and terminal state across those
clients. Workstream Navigator does not add leases, heartbeats, takeover, or
input fencing. Simultaneous typing can interleave and is explicitly a
user-coordination concern in V1.

### Host runtime layer

Every managed host owns:

- one private state root;
- one stable host identity;
- zero or more runtime-private tmux sockets and server generations, one for
  each live Runtime;
- the Workstream, ProjectLocation, onboarding/Fork recovery, binding, and
  attention records for work physically running on that host.

Every newly created Runtime derives its private tmux directory and session name
from the complete opaque Runtime UUID. Lazy provisional materialization
preallocates one candidate RuntimeId and uses these same final full-UUID
`RuntimePaths` fields (directory, socket, configuration, and session) before a
durable Runtime row exists; promotion adopts that candidate and never renames,
re-homes, or replaces its live server. The persisted session value must match
that exact current form before WSNav probes, attaches, parks, or removes a
private server. A narrowly defined former
short-ID form is read only for a Runtime record created by an older build; any
other value is ambiguous and no tmux action is attempted.

The private tmux server owns the terminal container, while SQLite owns bounded
Runtime metadata, exact provider-process authority, and recoverable
onboarding/Start/Fork state. The native provider owns session history. A closed
tmux server is not proof that its former pane process exited: a provider may
survive terminal hangup or spin on a deleted PTY.

Each live Runtime is a bounded tmux unit:

```text
Runtime -> one private socket and server -> one session -> one window -> one pane
```

No private runtime server contains a sibling Workstream. Parking or stopping a
Runtime removes its server rather than leaving an empty session.

Before releasing the launch barrier, WSNav proves that the sole pane process is
the leader of its private process group and persists the exact leader PID plus
process birth. Park first stops any provider-owned observer, then sends bounded
TERM to that exact proven provider group while its leader identity is still
corroborated. Surviving group members receive bounded KILL only after the same
group/session ownership is revalidated. The private tmux server and artifacts
are removed only after the complete group is gone. Observer sidecars remain a
separate exact-PID ownership boundary and are never treated as provider-group
leaders. Missing, changed, inaccessible, or malformed ownership evidence is
never signaled; cleanup failure refuses the parked transition.
Once the exact provider group is proven gone and the private Runtime artifacts
are removed, Park commits `Runtime=stopped` and `Workstream=parked` atomically.
That convergence also applies when the Workstream was already
`recovery_required`: an explicit Park after a failed cleanup resolves the
retained Runtime to a safely resumable parked state instead of persisting the
invalid `recovery_required + stopped` pair. Provider binding and sticky
attention remain retained; no provider session is deleted or replaced.
The host registry, not tmux's own session list, is the host-local Workstream
catalog. This contains server failure, terminal sizing, attachment, and
`tmux ls` visibility to one Workstream at a time.

### tmux namespace boundary

Workstream Navigator never creates a session on the user's default tmux
socket. An ordinary `tmux ls` therefore contains no Workstream Navigator
sessions. A naming prefix is useful only when an operator deliberately inspects
a private socket; it is not an isolation mechanism or a fallback for sharing
the default server.

A Workstream is not a tmux window. Window-per-Workstream would couple
attachments through session-level current-window selection and size policy.
Independent private servers keep an explicit provider attachment and tmux
failure scoped to one Workstream.

The private runtime socket belongs under the host's private state/run root at a
short, bounded path. The host registry records it; no socket-discovery scan of
the default tmux directory is permitted.

There is no shared or always-running remote daemon in V1. Each wsnav instance
launches only bounded host-local actions and provider metadata helpers against
its current registry. A host-local navigator may refresh bounded snapshots for
its own state, but it never polls another host, caches another host's state,
backs off a remote endpoint, or models a remote host as unreachable. D8.1 adds
only one host-local observer sidecar scoped to each live OpenCode Runtime
generation; it is neither a shared service nor a network control plane.

The outer SSH session used to reach a machine is user-owned terminal
composition, not a WSNav transport. If it detaches and the exact private
presentation remains alive, the presentation and its provisional shell are
preserved for reattachment. If the presentation is conclusively lost, WSNav
cleans only exact provisional artifacts; ambiguous ownership is left untouched
and blocks a duplicate shell. In every case the host's private managed Runtime
and provider remain untouched. A later host-local `wsnav` invocation reopens
the presentation and attaches to that exact Runtime.

All mutation commands use host-local SQLite transactions and optimistic
revisions. Independent Start commits its Workstream and private Runtime
reservation before launching the selected native provider, so reopening that
Workstream automatically continues the normal start/resume path after a client
loss. Fork additionally uses a durable request key and recovery phases because
native provider session creation is non-idempotent. Concurrent observations
and clients may race, but only one transaction can commit a particular record
revision.

Focus, attach, snapshot, and passive observation refresh are not durable
operations. Rename is a repeatable provider setting. Park and Resume reconcile
through the authoritative Runtime record plus live tmux/process probes.
Brokered onboarding and Fork use the `CompoundOperation` journal, as does
OpenCode's non-idempotent blank-session creation boundary.

Resume transactionally reserves one new Runtime generation before launching
tmux or the selected native provider. The launcher must match that exact
prepared record, and another Resume is refused while the generation is
`starting` or live. If the response is lost, a snapshot reconciles the prepared
record with the exact private tmux socket and process evidence instead of
starting a second Runtime.

The private pane initially runs a silent one-shot WSNav launch barrier. Its PID
and process birth are recorded against the prepared Runtime before the owning
action releases the barrier. The barrier then `exec`s the selected provider TUI
in place, preserving the same PID and birth token. For Codex, this prevents an
immediate `SessionStart` from racing ahead of its recorded hook authority;
OpenCode additionally must satisfy the D8.1 endpoint and sidecar readiness
barrier before attachment.

### Host-local control boundary

The navigator and public CLI call one typed in-process local application
facade:

```text
snapshot() -> derived host display, projects, locations, workstreams, dynamic provider capabilities, runtime probes, attention
apply(action, expected revisions) -> deterministic outcome
attach(runtime_id) -> native terminal attachment
```

`HostId` appears once as registry identity and display-label fallback evidence;
it is not repeated as an action selector. Host aliases, host transports, and
host-plus-Workstream compound selection keys do not exist in the target local
boundary. Workstream and Location IDs are resolved only inside the already
selected current registry.

The facade is a Rust call boundary, not a generic `HostClient`, `LocalEndpoint`,
framed JSON protocol, hidden local control endpoint, or public control ABI.
The current source omits those abstractions together with remote-only JSON, SSH
transport, release/capability handshakes, polling, and cache machinery.
Subprocesses remain only where the owned
operation is inherently external: tmux, Git, provider helpers, hooks and
observer processes, launch barriers, and direct terminal attachment. Their
bounded DTOs exclude paths from public projections, prompts, responses,
terminal captures, credentials, and raw provider payloads.

External probes and finite helper calls retain bounded process deadlines.
Runtime-creating and recovery mutations retain the longer bounded deadline
needed for provider readiness barriers; no generic control process exists.
Host-local observer hooks commit status and AttentionState before a snapshot or
action result exposes them, and optimistic revisions still reject stale
mutations.

### Codex adapter

Production sessions use the user's normal Codex home, authentication,
configuration, plugins, skills, models, permissions, and native history.
Temporary Codex homes remain test-only.

Every live Workstream runs one dedicated native `codex -C <project-root>` or
`codex -C <project-root> resume <thread-id>` process in its own host tmux
session.
The TUI owns that process's runtime for its entire lifetime. Workstream
Navigator never launches a managed TUI with `codex --remote`.

Workstream Navigator also never starts a persistent App Server listener. A
Unix, WebSocket, or other shared listener plus one or more `codex --remote`
clients changes the runtime into a client/shared-server topology. That
contradicts the Workstream isolation boundary even if the provider surface
still looks native.

Workstream Navigator installs one narrowly scoped profile named
`wsnav-observer` at `$CODEX_HOME/wsnav-observer.config.toml`. Managed native
launches add `--profile wsnav-observer`; ordinary Codex launches do not. This
uses the user's existing `CODEX_HOME`, not an isolated or copied home.

The generated hook command carries the canonical private host state root as a
static, quoted command argument. It never relies on a launch environment value
to locate Runtime authority: Codex 0.146.0 sanitizes arbitrary launch values
before it runs ordinary command hooks. The argument is part of the exact owned
profile declaration and trust hash, not user or provider input.

Codex loads the normal system and user configuration, overlays the selected
profile, then applies trusted project configuration and explicit CLI
overrides. The WSNav-generated declaration contains only the hook feature
setting and the four observation-only lifecycle hook definitions below. The
dedicated profile may additionally carry the bounded provider-owned model
prefix described below when Codex's native `/model` UI writes it. WSNav never
selects or changes a model, provider, reasoning effort, permissions, sandbox,
approval policy, MCP server, skill, plugin, memory, UI preference, or native
history setting.

V1 does not compose two named Codex profiles. If a user later needs another
selected profile for managed launches, WSNav reports that capability as
unsupported rather than copying, parsing, or synthesizing the user's profile.
Session-scoped hook injection or explicit profile composition may be studied
later. This does not affect ordinary Codex use of any profile.

Opening `wsnav` performs only read-only observer readiness detection. It does
not install, update, remove, or force review of a profile, and an unready Codex
adapter does not block Workstreams, Archived, the provisional shell, or
attachment to an existing live Runtime. Before a provisional shell reserves a
Codex launch, the shell gate performs the same read-only check. If setup is
required, the wrapper retains the original bounded argv only in process memory,
asks for explicit consent in that shell, opens the native review in place, and
retries the exact argv only after readiness succeeds. No broker reservation,
capability, Runtime, Workstream, or ProjectLocation exists before that success.

When the user requests a Codex Start, Resume, Fork, or other managed operation
that requires an unready observer, the Navigator instead captures that exact
intent and its expected Workstream, Location, integration, and registry
revisions in a typed process-local pending action, then offers contextual review
in the right pane. A stopped or deliberately parked attachment may surrender
only its disposable outer helper after exact proof that its private Runtime is
absent; a live, ambiguous, changed, or foreign attachment is never replaced.

A non-interactive public CLI action never installs, updates, or opens native
review. It returns a typed `observer readiness required` result with bounded
guidance to use interactive `wsnav`; hidden internal preparation entrypoints
remain inaccessible as normal public workflow.

For an absent profile or an exact owned declaration requiring update, the
review path explains the bounded mutation and asks for explicit consent before
any write. Declining cancels the pending action or shell retry without
mutation. A missing
ownership record, foreign or modified file, disabled configuration, ambiguous
path, or other ownership mismatch is never adopted or overwritten; the guide
reports the exact refusal and leaves the requested action available for an
explicit retry after external correction. Installation or declaration update
also refuses while any WSNav-managed Codex Runtime is live and guides the user
to park or stop it first; existing Runtime attachment remains available.

An accepted creation or update writes only the exact owned profile through a
mode-`0600` temporary file and atomic rename. Its human-readable managed marker
does not grant authority: write and removal authority comes from the private
host record containing the owner ID, schema version, canonical profile path,
absolute WSNav hook executable path, and exact generated-declaration hash.

The hook definition is reviewed and trusted through Codex's native `/hooks`
UI. WSNav never writes Codex's trust database and never passes
`--dangerously-bypass-hook-trust`. Once the exact owned declaration is
`trust_pending`, the shell or Navigator review path may replace only the
presentation's right pane with a temporary native, profile-selected Codex
review process in an empty disposable cwd beneath the exact owned presentation.
The navigator remains visible; the
operator uses the normal `/hooks` UI, trusts the exact generated command if
desired, and exits without submitting a prompt. That temporary process is not
a managed Runtime or Workstream and deliberately has no observer authority: an
invoked hook drains and does nothing. A bounded sibling marker binds the cwd to
the presentation ID/revision, owner PID/birth, and presentation/directory
device/inode. The process-local owner quarantines and revalidates the exact
empty directory and marker before non-recursive removal. An interrupted owner
is never adopted by a later review; after the presentation lifecycle has
stopped its pane and any provisional Runtime, it completes the same exact
cleanup. Missing, replaced, non-empty, foreign, malformed, or ambiguous
evidence is preserved and fails closed.

On review exit, WSNav silently re-detects the complete native trust record and
revalidates every captured revision. It continues the pending action or exact
shell retry only when the profile is exactly ready and the original intent is
still current. An incomplete or declined native review leaves the owned
profile accurately `trust_pending` and cancels the pending action or shell
retry with guidance; changed revisions likewise cancel rather than retargeting
the operation. The review path
neither inspects the current cwd nor creates a ProjectLocation or Workstream.
A blank Codex landing screen emits no `SessionStart`, so no stronger passive
activation signal is fabricated. The first managed `SessionStart` must instead
pass the normal provider-side corroboration gate. Whether an unprompted review
process leaves any native history residue is a validation gate and must be
disclosed if it cannot be avoided.

Native Codex hook review appends trust records to the selected profile itself:
`[hooks.state]` records keyed to the exact generated hook entries and trusted
`[projects]` entries. Native `/model` instead prepends the selected `model` and
`model_reasoning_effort` to that same profile. WSNav therefore verifies the
document as three independently owned regions: an optional provider prefix,
the byte-exact generated declaration beginning at the managed marker, and a
narrow schema-checked native trust suffix.

The provider prefix is at most 4096 bytes and must contain one or both of the
top-level `model` and `model_reasoning_effort` keys as non-empty TOML strings
of at most 256 decoded bytes each. WSNav preserves the prefix byte for byte but
never interprets, hashes, displays, or records either value in host state. An
unowned profile is never adopted, even when it contains only those keys. Any
other key or table, duplicate or malformed key, wrong or oversized value,
ambiguous managed marker, or model setting outside the prefix is `modified`
and fails closed.

The native suffix still permits only the four generated lifecycle hook keys,
`sha256:` trusted hashes, and project records whose sole value is
`trust_level = "trusted"`. A malformed record, unknown event, different hook
path, changed declaration, or other suffix setting is `modified` and fails
closed. This narrow mixed ownership preserves native model control without
making model state WSNav metadata or giving WSNav authority over arbitrary
profile configuration.

Existing user-configured Codex hooks remain the user's integrations. Workstream
Navigator neither disables nor rewrites them, and cannot guarantee that an
unrelated failing hook will preserve the native UI. `doctor` reports detected
overlap or failures when Codex exposes enough information, without silently
mutating the user's configuration.

Profile update or removal requires no live WSNav-managed Codex Runtime. A
contextually accepted update validates an exact legacy declaration, atomically
replaces it, and discards its co-located native trust suffix before entering
the same native review. A declaration-changing update preserves an accepted
provider prefix byte for byte and returns the integration to `trust_pending`
until native review succeeds again; an exact no-op preserves both prefix and
trust. Setup and update remain internal entrypoints used by the guide, not
public normal-workflow commands or a dedicated page.

Exact removal belongs to an exceptional documented uninstall/cleanup flow, not
ordinary navigator navigation. It refuses while any managed Runtime is live,
validates all three regions, removes the WSNav declaration and native trust
suffix, and then removes the ownership record. With no provider prefix it
deletes the profile; with an accepted prefix it atomically leaves only those
provider-owned model settings at the same path. A foreign, modified, disabled,
or ambiguous profile is preserved with a typed refusal. A model-only file is
foreign to a later setup and is never silently adopted. Base configuration,
other profiles, user and project hooks, plugins, history, credentials, and all
state outside the dedicated profile remain untouched.

The observer consumes these native events:

- `SessionStart` to bind the runtime to the exact Codex session and record
  `startup`, `resume`, `clear`, or `compact`;
- `UserPromptSubmit` to mark the exact session/turn as working;
- `Stop` to atomically mark the turn settled and create background result
  attention; and
- `SessionEnd` to mark the provider runtime stopped when available.

The hook is deliberately passive:

- it drains all stdin before any state lookup or early return;
- it keeps a bounded parse buffer and continues draining oversized input;
- it discards prompts and transcript paths rather than storing or logging them;
- it emits no stdout, model context, provider warning, or management message;
- it exits successfully when observation cannot be recorded, leaving the
  navigator `unknown` instead of disrupting Codex; and
- it finds one private Runtime through the static state-root argument, then
  requires the hook's direct Codex parent PID, process-birth value, and cwd to
  match exactly one current Runtime record; and
- its session, runtime generation, cwd, and binding revision are checked
  before an observation is accepted.

The pre-refactor launch-environment authority mechanism is falsified by
[Spike 0009](evidence/spikes/0009-codex-hook-environment-boundary.md): it must remain
fail-closed and cannot supply lifecycle status. [Spike
0010](evidence/spikes/0010-codex-hook-ancestry-authority.md) proves the static-argument
plus direct-parent candidate. The production observer implements that candidate
with the normal transactional binding and App Server corroboration gates. No
shell-wrapper ancestry fallback is allowed; that would admit an agent
tool-shell forgery.

Hook evidence can update status and bind an observed native session inside an
already managed runtime. It cannot authorize workstream creation, fork,
parking, provider input, Git mutation, or focus.

A ProviderBinding is stronger than an untrusted hook claim. A `SessionStart`
first agrees with a pending launch or the one accepted native transition, then
must agree with the recorded runtime generation, pane, cwd, provider PID,
process birth, and direct ancestry. Before it changes durable binding state,
WSNav performs one bounded,
read-only `thread/read(includeTurns=false)` over a new App Server stdio
connection and requires the returned `thread.id` to equal the hooked ID. Events
that cannot be corroborated may leave status `unknown`, but cannot replace a
known binding. The installed Codex 0.145.0 contract proved exactly one changed
binding rule: a distinct `SessionStart(source=clear)` in the same live TUI may
replace an `idle` or `attention` tip. Its predecessor ID/name metadata and
sticky result attention remain; all other changed, racing, replayed, working,
or unknown-source claims fail closed. Follow-up [Spikes
0011](evidence/spikes/0011-codex-native-new-rebinding.md),
[0012](evidence/spikes/0012-codex-new-prompt-session-rotation.md), and
[0013](evidence/spikes/0013-codex-new-thread-inventory.md) on Codex 0.146.0
show that native `/new` creates a distinct thread but provides neither a
changed `SessionStart` claim nor a changed first-prompt hook identity. It is
unsupported in a managed Runtime; `thread/list` ordering must not be used to
adopt a possible destination. Native `/fork` and `compact` remain provider
workflow whose changed-binding visibility is deferred until separately
validated. If legitimate transitions cannot be distinguished from an
agent-shell invocation, V1 must require explicit native resume/fork selection
and observe the resulting launch; it must not weaken the authority rule.

#### Ephemeral App Server adapter

Persisted thread metadata and bounded thread-store mutations use a separate,
per-operation App Server process on the host that owns the Codex state:

```text
wsnav host action
-> spawn codex app-server --listen stdio://
-> initialize one private stdin/stdout connection
-> issue one or more bounded requests
-> wait for the exact action result
-> close stdin and wait briefly for exit
-> kill and reap on bounded shutdown failure
```

No TUI connects to this process. It does not host interactive work, listen on a
socket, remain alive between operations, or become activity authority for a
dedicated TUI. The concrete Codex adapter filters App Server responses before they
reach any bounded Navigator state or status surface.

V1 allowlists only:

- `thread/read` with `includeTurns: false` for exact managed thread IDs and
  `SessionStart` binding corroboration;
- `thread/list` with `sourceKinds: ["cli"]` for ordinary bounded `doctor`
  checks, or every documented source kind only while reconciling one unresolved
  WSNav-owned Fork operation; both use `useStateDbOnly: true`;
- `thread/name/set` for an explicit Rename action or a provisional fork name
  set before its destination TUI starts; and
- `thread/fork` with an exact accepted `lastTurnId` and destination `cwd` for
  an explicit Fork Workstream operation.

V1 does not call App Server turn start, steer, interrupt, item injection,
runtime configuration, shell, approval, or provider-input methods. App Server
runtime `status` is scoped to that short-lived process and is never treated as
the status of a separately running native TUI. Codex 0.145.0 can expose a
persisted partial turn as interrupted while the native TUI's command is still
running; this is expected evidence that helper status is non-authoritative.

`thread/read` and setting an already-requested name are safely repeatable.
`thread/fork` is not assumed idempotent: if the helper exits after Codex may
have created a destination but before returning its ID, Workstream Navigator
must reconcile exact provider lineage and recorded operation evidence. It must
not retry and risk a duplicate destination while the effect remains ambiguous.

Only an unresolved CompoundOperation with `kind=fork` may use `thread/list` for
this reconciliation. Its recorded evidence includes the exact source session,
accepted last-turn ID, requested destination cwd, and effect timing. Installed
Codex 0.145.0 did not persist the requested fork cwd before native resume and
did not place the fork in the CLI-only source-kind query. Recovery therefore
queries all documented source kinds and matches exact source lineage, settled
prefix, and effect time; requested cwd remains operation intent, not candidate
proof. Recovery accepts only one matching destination; zero or multiple
candidates remain `recovery_required` and are never guessed or automatically
adopted. `doctor` may report the same bounded evidence but cannot turn broad
discovery into ownership.

The concrete Codex adapter extracts only approved fields from responses. It never
returns or persists `preview`, turns, items, transcript paths, or the raw
response.
`thread.preview` is prompt-derived and therefore is not a naming fallback.

Codex's native CLI and ephemeral App Server divide the action boundary:

- fresh work uses `codex`;
- recovery uses `codex -C <project-root> resume <session-id>`;
- a Workstream fork uses App Server `thread/fork`, then starts the resulting
  thread through `codex -C <project-root> resume <destination-id>`;
- chat naming uses native `/rename` or App Server `thread/name/set`, both
  changing the same Codex-owned field.

#### Workstream display names

The current tip's non-empty `thread.name` is canonical. When it is missing or
cannot be refreshed, the navigator computes a context-specific display
fallback without persisting a shadow label.

Name observation and transition context are separate:

```text
NameState
  named | known_empty | unavailable

EffectiveNameSource
  native | cutover_fallback | fork_fallback | cached_stale | synthetic
```

`known_empty` means an exact App Server read returned no name. `unavailable`
means the bounded host-local read did not complete; it does not erase a cached
name.

| Context | Effective display when the current tip has no native name |
| --- | --- |
| New Workstream before thread binding | `starting` |
| New or existing Workstream with a known-empty name | `untitled` |
| Same-Workstream cutover from named A to unnamed B | `<A name> ↻ unnamed` |
| Same-Workstream cutover when A was also unnamed | `untitled ↻` |
| Fork to a new Workstream from a named source | `<source name> · fork` |
| Fork from an unnamed source | `forked workstream` |
| Metadata refresh unavailable with a current-tip cache | Last cached native name with a stale indicator |
| Metadata refresh unavailable without a current-tip cache | The contextual transition display with `name unavailable`; otherwise `name unavailable` |
| Provider thread missing during recovery | Last cached native name with `recovery required`; otherwise `recovery required` |

Resolution prefers a current non-empty native name, then a current-binding
cache when refresh is unavailable, then transition context, and finally a
synthetic lifecycle fallback. An unavailable observation never becomes
`unnamed` or `untitled`; those displays require `known_empty`. The reduced
navigator's last-resort row label is the stable short Workstream ID without a
synthetic prefix; it never substitutes for exact identity or action authority.
Fallbacks never expose a provider identifier or raw provider payload. Git
state, host, and cwd remain secondary context rather than naming authority.

An exact thread ID, not any displayed text, remains identity and action
authority. Names and computed fallbacks need not be unique.

Navigator rows show Project, provider, current tip name, and a relative age
from the last observed native conversation activity. Activity sequence remains
the deterministic ordering key within this host. There is no cross-host
ordering or combined client view. The wall-clock value survives start, resume,
and park. A migrated Workstream or one with no observed turn visibly reports
`activity unknown` until its first prompt submission or settled result.

Native `/rename` and navigator Rename both update the current Codex thread
name. The navigator action calls `thread/name/set`; a later bounded metadata
refresh observes either route. One ephemeral App Server process may read
several exact managed thread IDs before it exits, but Workstream Navigator does
not keep a shared server alive to receive name notifications.

After a native same-Workstream cutover, Workstream Navigator must not
automatically set `B.name = A.name`. App Server `thread/name/set` has no
compare-and-set field, so a read-then-write could overwrite a fast native or
skill-driven rename. The previous title remains a visibly provisional computed
fallback until B obtains its own canonical name.

An explicit Fork Workstream action may set a bounded provisional native name
derived from a non-empty source thread name after `thread/fork` returns and
before the destination TUI starts. That ordering prevents a user rename race.
If the source has no native name, the navigator uses the computed fork fallback
and leaves the destination name empty.

Semantic automatic naming is not a lifecycle-hook responsibility. It may later
be offered as an opt-in Codex skill or managed agent policy, where Codex already
has conversation context. V1 does not read prompts or transcripts, invoke a
second model, or derive a semantic name out of band.

If the session identity hook was missed, a still-live runtime remains
attachable. After that process is lost, exact resume and conversation fork are
blocked until the user selects a session through Codex's native resume picker
and a later `SessionStart(source=resume)` rebinds it.

## Durable state

V1 uses fresh SQLite schemas with no migration from Agent Switchboard. The
worktree-free schema is intentionally a breaking host-state boundary: a host
database from the retired worktree-managed design fails closed and requires an
explicit state reset and project re-registration. WSNav never silently deletes
or mutates that state.

### Historical state transitions

The schema-12 through schema-14 client-catalog, host-local cutover, and
onboarding migrations are retired evidence. Current source never opens,
migrates, adopts, drains, or cleans those roots. Their complete contracts and
release results remain in the [D16 acceptance](evidence/acceptance/d16-host-local.md),
[D17 acceptance](evidence/acceptance/d17-shell-first.md), and
[D17.1 closure](evidence/acceptance/d17.1-correctness-closure.md).


### Current onboarding state boundary

Schema 15 stores the current onboarding journal directly. It contains no
browser setting or migration-only metadata, and it never opens or transforms a
schema-12 through schema-14 root. Unsupported, malformed, future, foreign, or
mixed evidence fails closed before mutation.

The provisional shell itself has no durable registry row. At lazy materialization
the presentation marker owns one fresh opaque `slot_generation`, one opaque
candidate `RuntimeId`, the exact full-UUID `RuntimePaths` fields (directory,
socket, configuration, and session), the seed cwd, and bounded shell/server
ownership evidence. That marker is the only authority for the provisional
process and never becomes a `Runtime` or `Workstream` row by itself. Before
provider execution, the prepare broker acquires the stable host-private
`provisional.lock`, revalidates the marker, its `lease_generation` and
`slot_generation`, captured presentation revision, and registry generation, creates or
reuses one exact request-keyed `CompoundOperation`, and transactionally
generates/reserves the durable Runtime generation while adopting that exact
candidate ID and unchanged `RuntimePaths` fields (directory, socket,
configuration, and session). It records the detected Project/ProjectLocation,
fixed provider, Workstream, and generation, then marks the handoff issued while
the lock is held. Its bounded phase records prepare, token issuance, helper
handoff, `runtime_owned_launching`, provider-specific preparation/external-effect
phases, `provider_exec_started`, `provider_exec_proven`, known-absent exec
failure, and `recovery-required`/`unknown`.

The issued capability binds the request/operation, presentation and
provisional-slot identities, `lease_generation`, `slot_generation`, exact
candidate Runtime ID and `RuntimePaths` fields (directory, socket,
configuration, and session), fixed provider, exact live shell cwd and detected
root/Location, reserved Runtime generation, captured registry/presentation
revisions, shell PID/birth/process group, grammar-approved argv digest, and a
short monotonic expiry. The operation
persists only a bounded token identifier/verifier, claim
references or digests, expiry, and phase; the live token and original argv are
never persisted. Secret-bearing arguments are outside the promotable grammar.
The operation carries only the identities and phase needed to distinguish a
conclusively absent external effect from an ambiguous one; it never stores cwd
history, shell commands, arguments containing secrets, environment, terminal
bytes, or provider payloads.

The same `provisional.lock` lease serializes confirmed close/loss cleanup and
helper consumption. Its lock generation and the slot generation are checked on
every transition. A prepared reservation does not revoke provisional cleanup.
Before the helper's
successful revalidation and atomic capability consume plus durable `Runtime-owned`
commit, close may win only by acquiring this lease, atomically canceling/
revoking the still-unconsumed capability, proving pre-effect absence, rolling
back attempt-only rows, and then cleaning the marker-backed artifacts. The
helper wins only after it reacquires the lock and successfully revalidates every
bound marker/process/cwd/path/revision/token claim; it then performs an atomic
compare-and-consume of the capability and commits durable `Runtime-owned`
authority. A mismatch does not advance ownership. It next, still under the
lock and before releasing it, revokes presentation cleanup authority, with
durable transition preceding marker cleanup; only after that does the operation
enter `runtime_owned_launching` and continue through the post-commit launch
fence. After that exact helper commit,
presentation cleanup never signals the pane, process, or server. Ambiguous
cross-store crashes remain in this journal for onboarding recovery. A normal
cancel, shell exit, unsupported provider argument, failed Git-root check, or
conclusive pre-effect launch failure after transfer is therefore resolved by
onboarding recovery, which may remove only attempt-created graph state after
the provider-specific journal proves no external effect or binding; the derived
singleton card then remains available but unmaterialized. Once the provider
boundary may have been crossed, cleanup cannot manufacture absence: the
Workstream and any OpenCode binding remain visible in the exact
`recovery-required`/resume state, and OpenCode never issues a second
non-idempotent POST.

### Host registry

The host registry contains:

```text
HostIdentity
  host_id, registry_generation, schema_version

CodexIntegration
  integration_id, profile_name, canonical_profile_path, owner_id,
  profile_schema_version,
  hook_executable_path, generated_content_hash, lifecycle, revision

Project
  project_id, label_location_id, display_name, repository_fingerprint?, revision
  (non-empty repository_fingerprint is unique within this host)

ProjectLocation
  location_id, project_id, repository_path, repository_display_name,
  remote_identity_fingerprint?, remote_identity_display?, revision
  (credential-free Git-origin metadata for same-host presentation only;
  historical field names do not grant remote authority)

Workstream
  workstream_id, location_id, provider, origin,
  source_workstream_id?, lifecycle, archived_at?,
  last_activity_sequence, last_activity_at_millis, revision

IndependentCreationRequest
  request_key, source_workstream_id, source_revision, workstream_id

Runtime
  runtime_id, workstream_id, provider, tmux_generation,
  tmux_session, cwd, provider_pid, process_birth, lifecycle, revision

OpenCodeRuntimeHandle
  runtime_id, runtime_generation, endpoint_host, endpoint_port, version,
  native_session_id, observer_pid?, observer_birth?, observer_status, revision

ProviderBinding
  binding_id, runtime_id, provider, native_session_id, start_source,
  last_settled_turn_id?, observed_thread_name?, name_state,
  name_observed_at?,
  predecessor_native_session_id?, predecessor_effective_name?,
  runtime_generation, revision

AttentionState
  workstream_id, result_unseen_since_revision?,
  recovery_unseen_since_revision?, latest_native_session_id?,
  latest_native_session_provider?, latest_turn_id?, revision

CompoundOperation
  operation_id, request_key, kind=onboard|start|fork, phase,
  phase includes runtime_owned_launching, provider-specific preparation and
  external-effect phases, provider_exec_started, provider_exec_proven,
  exec_failed_known_absent, recovery_required, or unknown,
  expected_revisions_json, launch_token_id?, launch_token_verifier?,
  launch_token_expiry_monotonic?, launch_claims_digest?,
  effect_watermark?, outcome_json?, revision
```

Paths and provider identifiers are private host fields. Public snapshots return
bounded host-local Project and thread names, name provenance, statuses,
capabilities, and opaque Workstream Navigator IDs. Credential-free origin
fingerprints and safe display labels may be produced by local Git inspection
and consumed by the same host's presentation layer; they never cross a WSNav
network boundary or associate records owned by another host. No raw remote URL,
prompt, preview, response, transcript, tool payload, terminal capture,
credential, or environment dump is persisted.

### State relationships

- The provisional shell and card have presentation-private identities only; the
  pinned card is a derived singleton with no durable card row. Lazy
  materialization additionally records one fresh opaque `slot_generation`, one
  candidate RuntimeId, and exact final-form `RuntimePaths` fields (directory,
  socket, configuration, and session) plus ownership evidence in its marker.
  Materialization alone
  references no ProjectLocation, Workstream, Runtime, ProviderBinding, or
  CompoundOperation row. Broker prepare may reserve the ProjectLocation,
  Workstream, Runtime generation, and onboarding operation before the exact
  helper ownership commit; the selected card remains the exact shell until the helper commits
  durable Runtime-owned authority. Promotion adopts that same candidate ID and
  fields rather than creating or relocating a server.
- One Project contains one or more ProjectLocations owned by this host.
- One ProjectLocation references exactly one Project.
- Project identity and label-source state are presentation metadata and never
  authorize a host, Git, provider, Workstream, or Runtime action. The label
  source references exactly one current member location.
- One Workstream references exactly one ProjectLocation root.
- A promoted Workstream remains pinned to that exact launch-time Location even
  if its provider later changes directories or manages Git worktrees.
- Durable `Runtime-owned` authority during `runtime_owned_launching`, provider
  preparation, external effect, or `provider_exec_started` does not yet grant
  ordinary attachment or Runtime actions for that unproven Runtime. Its
  originating presentation may retain its existing pane or detach through
  ordinary card switching, but no new attachment to that Runtime is allowed.
  Selecting/materializing the fresh derived singleton card attaches only its
  separate provisional server under `provisional.lock` and grants no authority
  over the unproven Runtime. Snapshots may show `starting`/`onboarding`, while
  the onboarding reconciler alone may advance the operation. A terminal
  known-absent result is resolved atomically: provider-specific proof of no
  effect/binding permits guarded rollback and ends onboarding, while a known
  OpenCode binding ends onboarding in the exact stopped/recovery state where
  only binding-preserving Resume/recovery or explicit Park is allowed. After
  explicit Park has stopped that exact adopted Runtime and recorded the
  Workstream parked under `provisional.lock`, it commits onboarding recovery
  resolution without asserting that the original native exec was proven,
  without deleting the retained binding, and without retrying a provider
  effect; ordinary Resume then uses that retained binding.
- One host has at most one owned `wsnav-observer` CodexIntegration.
- One Workstream has at most one live Runtime.
- One Runtime has one current ProviderBinding.
- The binding may retain only the immediately replaced native session ID and
  effective name needed for cutover display and bounded recovery. V1 stores no
  browsable or recursively linked binding history.
- The current ProviderBinding plus its accepted `last_settled_turn_id` is the
  Workstream's ConversationTip.
- `observed_thread_name` is a cache of Codex-owned metadata, not a second
  naming authority.
- `name_state=unavailable` retains a prior cached name; an unavailable refresh
  never becomes evidence that the provider name is empty.
- `EffectiveNameSource` is derived presentation state and is not persisted as a
  user-authored name.
- Codex may create native conversations sequentially inside one Workstream as
  the user uses native `/clear` or `/fork`. D1.5 observes only the separately
  proven `/clear` binding replacement; other native actions remain canonical
  Codex workflow without an inferred WSNav transition. Native `/new` is not a
  supported managed action: Codex creates its destination thread, but WSNav
  retains the prior binding because no exact transition claim identifies that
  destination.
- One sticky AttentionState exists per Workstream; it never changes
  presentation focus.
- Runtime status and Workstream lifecycle are separate.
- Archive visibility is separate from Workstream lifecycle. An archived
  Workstream retains `parked` or `recovery_required`, its exact binding,
  AttentionState, ProjectLocation, and lineage; restore never starts a Runtime
  automatically.

Every accepted settled turn marks the Workstream's AttentionState as unseen
independent of focus, updates its latest exact identifiers, and leaves
`result_unseen_since_revision` unchanged if an earlier result was already
unseen. Recovery evidence similarly makes its unseen flag sticky and dominates
ordinary result presentation. Acknowledge uses the exact observed AttentionState
revision and clears only that kind of notification, so a concurrent newer event
cannot be lost. Acknowledging recovery attention does not clear the Workstream's
`recovery_required` lifecycle; only successful recovery does. The row is not an
event history: provider results remain canonical in Codex.

Suggested CodexIntegration lifecycle values:

```text
trust_pending | ready | modified | disabled
```

No record means not installed. `modified` means the generated profile no longer
matches the owned hash. `disabled` means Codex policy or a higher-precedence
configuration prevents the profile hooks from running. Neither state is
silently repaired.

Suggested Workstream lifecycle values:

```text
open | parked | recovery_required
```

Suggested observed Runtime status values:

```text
starting | onboarding | idle | working | attention | stopped | unknown
```

`unknown` is an observation boundary, not proof that a runtime stopped.

### Current schema-15 consolidation boundary

D18 is a behavior-preserving consolidation of the D17.1 product, not a new
user workflow. Its purpose is to make the implemented architecture match the
current contract after the D16 and D17 clean breaks: one current schema, one
current startup path, one current presentation, and semantically named current
modules and internal commands. The Shell-first Workstreams/Archived navigator,
provider-native terminal ownership, brokered Codex/OpenCode launch, managed
session lifecycle actions, and all recovery boundaries remain unchanged.

D18 intentionally does not migrate, import, adopt, drain, or clean a D16 or
D17 root. Schema 15 is the only accepted current host schema. An absent state
root, or an exact current-user-owned private empty root, may be bootstrapped
directly at schema 15; an exact schema-15 root may be opened or recovered
through the current-format bootstrap protocol below.
Schemas 12, 13, and 14, `client.sqlite` artifacts, D16 transition artifacts,
legacy presentations, and any other retired state cause a typed refusal before
mutation. Unsupported future, malformed, foreign-owned, replaced, or
ambiguous evidence continues to fail closed.

The refusal classifier may perform only bounded no-follow metadata reads of the
selected root and its top-level names, plus a bounded raw read of the
`host.sqlite` header needed to classify its SQLite magic, fixed WSNav
application ID, and schema identity. Direct schema-15 bootstrap sets and
checkpoints that exact application-ID/user-version pair into the main database
file before publication, and no later current operation changes it, so classification does
not need to open SQLite, recover a journal/WAL, read old tables, or inspect a
legacy tmux/process topology. Only after the raw identity is exact schema 15 may
the current validator open the database and hand current `run/` and
`presentation/` namespaces to their own exact validators. A schema-12 through
schema-14 root is refused from its schema identity alone; a presentation name
beside it is not parsed as legacy evidence.

The classifier must not read or import the retired client catalog, inspect old
rows in order to adopt Workstreams or Runtimes, signal an old tmux server or
provider process, remove an old file, or rewrite an old database. D18 does not
contain a schema-14-to-15 migration, legacy presentation classifier, or
compatibility launcher. Provider-owned native history remains available in the
provider's ordinary native tooling outside WSNav's managed onboarding path;
the provisional Shell continues to refuse resume/session arguments, and the
old WSNav catalog and Runtime ownership are not carried into the new epoch.

The accepted startup matrix is:

| Root evidence | D18 result |
| --- | --- |
| State root absent, or exact private empty root | Create a direct schema-15 root through the current bootstrap protocol. |
| Exact schema 15, ready current bootstrap metadata, exact current lock artifacts | Open normally. |
| Exact, unambiguous interrupted current-format bootstrap | Resume only that bootstrap to a ready schema-15 root. |
| Schema 12, 13, or 14; retired client catalog; D16 transition or legacy-shaped top-level artifact | Refuse from bounded metadata/schema identity without mutation and show bounded offboard/reset guidance. |
| Future schema, malformed or missing identity, foreign ownership, replacement, or mixed evidence | Refuse without mutation; never guess, repair, adopt, or delete. |

#### Direct schema-15 bootstrap

Fresh creation from an absent or exact private empty root uses one
authoritative schema-15 definition. It does not build schema 12 and apply
schema-13/schema-14 fragments. The direct schema omits retired browser settings
and uses semantic current names, including
`onboarding_exec_targets` rather than a delivery-checkpoint-qualified table.
One current schema constant and validator define the entire accepted durable
shape. An exact fixed SQLite `application_id` plus `user_version = 15` is the
raw main-file identity. The application ID is `0x57534e56` (ASCII `WSNV`);
neither value is inferred from filenames or tables.

A stable host-private `bootstrap.lock` is the sole cross-process authority for
creation and current-format bootstrap recovery. Its bounded checksummed record
contains a format version, exact current schema identity, HostId, bootstrap
generation, canonical root device/inode, phase, and only the optional
database/provisional-lock identities defined for that phase. It is opened
without following links; is owned by the current user with mode `0600`; and is
validated against the held root and lock path/device/inode before every effect.
The exact inode remains stable after
bootstrap rather than being unlinked or replaced. A phase update rewrites only
that held inode, syncs it, and syncs the root before the next external effect;
a torn, oversized, malformed, or uncertain update refuses. Actors take a
nonblocking exclusive FD lock only while classifying, creating, or finishing
the root, and never pass that descriptor to provider processes.

Classification never creates or adopts `bootstrap.lock` in a non-empty root.
For an absent root, D18 first creates and binds the exact private directory; for
an exact private empty root, it binds the existing directory; only then may it
create-new the initial lock. For a non-empty root, an exact current-format lock
must already exist and be acquired before any schema-15 database read/open. A
missing, busy, malformed, foreign, or replaced lock refuses without creating a
substitute. A non-empty root without a current lock may receive only the raw
header/top-level refusal classification above, never a bootstrap mutation.

The current bootstrap graph is deliberately closed:

| Durable phase | Exact allowed bootstrap evidence | Authorized next action or recovery |
| --- | --- | --- |
| `root_reserved` | Private root plus the exact held `bootstrap.lock`; no database candidate or provisional lock | Reserve one opaque staging-database name in the lock record. |
| `database_create_reserved` | The same evidence plus the precommitted staging name, but no recorded staging inode | Create-new one private regular staging file. If a file is present after restart before its identity was durably recorded, creation is effect-unknown and the whole root is refused without cleanup. |
| `database_owned` | The lock record binds the staging path/device/inode; `host.sqlite` and `provisional.lock` are absent | Initialize one transactionally complete direct schema-15 database using bootstrap-local rollback journaling. An exact empty owned staging inode may be initialized; a valid complete matching database with no staging sidecar may advance; partial, changed, malformed, or sidecar-bearing content refuses rather than being opened/recovered or rebuilt. |
| `database_ready` | The exact staging database validates schema 15, HostId, bootstrap generation, and `provisional_pending`; the destination and every staging sidecar are absent | Sync/close the staging database, then rename that exact inode to `host.sqlite` and sync the root. Restart accepts the inode at either the staging or final name, never both. |
| `provisional_pending` | Exact published `host.sqlite` with matching pending metadata; no provisional file, or one exact private file whose bounded contents match the precommitted HostId and lease generation | Create-new or reacquire that one stable provisional-lock candidate, record its device/inode transactionally, commit/sync database `ready` first, then advance/sync the bootstrap record. Restart may advance an exact database-ready/bootstrap-pending pair; the inverse pair is impossible and refuses. A mismatched or replacement file refuses. |
| `ready` | Exact matching root, stable bootstrap lock, schema-15 database, and ready `provisional.lock`; no staging database | Open current state. Current SQLite sidecars and the `run/` and `presentation/` namespaces are delegated to their current owners; a transition/client/legacy-only top-level name remains refusal evidence. |

Every phase transition durably precommits the identity or opaque name needed to
classify its next effect. A crash after an effect but before its exact identity
commit is deliberately effect-unknown and refuses; D18 never converts an
unknown artifact into owned state merely because it has a plausible name or
shape. Recovery mutates only an artifact already bound by the last durable
phase and repeats all root/lock/path/inode/contents checks. There is no generic
"repair fresh root" or partial cleanup command; an operator may move an
unrecoverable never-used root aside as a whole and start again.

The schema-15 database records the same HostId, bootstrap generation, and
`provisional_pending | ready` phase. The lock record and database must agree
before either can authorize progress. Direct creation uses an explicit
rollback-journal completion/sync/close boundary with no staging sidecar before
`host.sqlite` publication, ensuring the raw main-file schema identity used by
the refusal classifier is authoritative.
`transition.lock` is not a current artifact and no current code recognizes it
as mutation authority.

#### Explicit D17.1 destructive reset

D18 is a clean state break, not a migration or rollback mechanism. The reset
discards the WSNav catalog, attention/archive state, operation records,
Runtime ownership, private tmux servers, and any retained terminal output.
Codex and OpenCode native history remains provider-owned and is not deleted.
D18 never reads, imports, restores, or adopts the discarded root.

Before ordinary-state mutation, the exact D18 candidate passes repository and
clean-host gates and its hash is recorded; installed parity is proved only
after the installed file matches it. Separately authorized disposable Codex/
OpenCode acceptance uses disposable state roots, repositories, provider homes,
and private tmux sockets. It must pass before D18 release completion, but an
operator who explicitly elects to discard the old WSNav state may execute the
destructive reset while that isolated provider gate remains pending.

The operator-controlled reset is deliberately short:

1. Show the exact selected state root and installed executable, summarize the
   owned Workstream/Runtime/observer inventory without provider content, and
   require explicit confirmation that the WSNav state and private Runtime
   output will be discarded.
2. Keep the installed D17.1 executable and old root in place while closing the
   exact owned presentation and stopping every exact WSNav-owned private
   Runtime and observer process. Unknown or changed ownership is never
   signalled or deleted; if owned-process absence cannot be established, reboot
   and perform the reset before starting WSNav again.
3. Run the installed D17.1 `remove-observer` flow while its schema-14 ownership
   record still exists. An unchanged owned declaration is removed. A modified
   or foreign declaration remains untouched and stops the reset. If exact
   removal preserves provider-owned settings at the dedicated profile path,
   move that complete settings-only file to an operator-owned inactive path;
   D18 does not adopt it.
4. Require no process executing the installed WSNav binary and no live exact
   WSNav-owned private tmux server. This is an owned-process shutdown boundary,
   not a claim that an unprivileged scan can prove zero arbitrary filesystem
   holders.
5. Atomically rename the exact private old root to a fresh sibling quarantine
   path and sync the parent. The quarantine is discarded state and never
   rollback or compatibility input. It may be deleted after D18 acceptance.
   No copy, merge, partial move, schema edit, or recursive traversal is part of
   the cutover.
6. Install the already accepted D18 artifact, verify byte identity, and start
   WSNav against the now-absent original root. It directly bootstraps schema 15.
   Re-establish Codex observer readiness only through normal explicit consent
   and native trust.

There is no symmetric state rollback. Downgrading requires another explicit
destructive reset and a fresh root for the selected older binary. A quarantined
schema-14 or schema-15 root is operator-owned archival data only; no WSNav
version may treat it as an import source.

[Spike 0027](evidence/spikes/0027-d18-root-move-falsification.md) remains useful
historical evidence for rejecting the stronger coherent-backup/online-rollback
design. Its impossible zero-arbitrary-holder proof is not required by this
destructive reset.

#### Compatibility deletion and semantic structure

D18 removes the inactive transition plane before moving current code. The
deletion boundary includes:

- schema-12/schema-13 creation, full historical schema fixtures, validators,
  migrations, recovery, and schema-14 migration support. Minimal inert raw-
  header samples generated only to prove non-mutating old-schema refusal remain
  test evidence, not open/import fixtures;
- the D16 cutover/startup orchestrator, client-catalog deletion, legacy
  presentation classifier/drain/retirement, and observer handover paths;
- D16 OpenCode observer standby/handover routes and journals;
- short-Runtime-ID path parsing and all other retired path compatibility;
- Codex observer-profile schema-1 upgrade compatibility; only the exact
  current profile schema is accepted, while other records are foreign or
  unsupported and remain untouched;
- retired Projects-page/browser root, refresh, arbitrary registration, DTO,
  error, schema, filesystem, and test surfaces, plus repository inspection used
  only by that workflow. The deletion does not include the one private
  onboarding transaction that creates or reuses the Git-discovered
  Project/Location and atomically creates its promoted Workstream/Runtime graph,
  nor contextual `n` reuse of an existing exact Location; and
- stale aliases, public exports, error variants, fixtures, and test harnesses
  whose only purpose is a retired checkpoint.

After those deletions, active code must not use `d16` or `d17` as the semantic
name of a module, function, type, error, hidden command, environment variable,
artifact, or durable table. Current internal routes use role names such as
`_navigator`, `_provider_attach`, `_provisional_shell`, and
`_opencode_observer`; exact spellings may follow the CLI structure, but no
checkpoint suffix is part of their contract. Provider protocol and marker
format versions such as `wsnav-launch-claims-v1`, the schema number, and native
provider version fingerprints remain versioned because they validate external
or durable evidence; they are not compatibility shims. Historical documents
and evidence retain their truthful D16/D17 names.

Deletion precedes broad source movement so obsolete code cannot become the
shape of the current modules. The resulting structure separates:

- current schema definition/validation from bootstrap ownership and recovery;
- registry transactions from onboarding journal and observer ownership;
- read-only Project/Location projection from Git root discovery;
- presentation ownership/topology from control, attachment, provisional-shell
  lifecycle, and cleanup; and
- provider-neutral lifecycle contracts from concrete Codex and OpenCode
  adapters.

The CLI remains a thin dispatcher, internal helpers receive only bounded typed
claims, and public exports are narrowed to actual cross-module contracts.
Module size is an outcome signal rather than an acceptance gate: D18 does not
trade clear ownership for arbitrary line-count limits or speculative generic
abstractions.

The documentation authority follows the same current-only boundary. At D18
closure, `design.md` retains the present product, architecture, invariants,
current schema/bootstrap/recovery contracts, and only a concise checkpoint
index. `roadmap.md` retains the active checkpoint, delivery order, exit gates,
and concise completed-status index. Detailed D0-D17 transition narratives,
version-bound test counts, and release procedures move out of present-tense
authority into dated evidence records when they are not already recorded
there. Historical evidence is preserved verbatim or with an explicit
provenance-preserving move; it is never rewritten as current behavior merely to
shorten the authority documents. `docs/README.md` and the installed CLI remain
the concise current operator entrypoints.

#### Retained invariants and non-goals

D18 retains every current safety and product invariant: provider output and
completed results stay in the native pane; management traffic never enters
that pane; every Runtime uses its own private tmux server; bounded metadata is
the only persisted provider information; hooks/process/tmux observations are
evidence rather than mutation authority; and ambiguous identity, ownership,
revision, or effects fail closed. Provisional-shell lease/capability recovery,
post-commit provider-effect fencing, direct-CLI parity, exact lifecycle
actions, and current presentation cleanup remain first-class current behavior,
not transition code to delete.

D18 adds no provider, UI page, session adoption, hard deletion, worktree or
branch management, transcript feature, daemon, automatic observer mutation, or
compatibility with the frozen Python prototype. It does not rename provider-
owned sessions, move native history, or infer a new Workstream from an
unmanaged provider process.

#### D18 acceptance contract

D18 is complete only when one coherent current-only acceptance matrix proves:

- absent/private-empty-root direct creation and exact schema-15 reopen/recovery,
  including every `bootstrap.lock` phase and failure injection around each
  reserve, create, identity commit, transaction, checkpoint, sync, rename,
  provisional-lock, ready, and reopen boundary. Tests prove the exact allowed
  artifacts and mutation authority for every resulting phase; effect-unknown
  gaps refuse without cleanup;
- byte/inode/hash evidence that schemas 12-14, client-catalog artifacts,
  `transition.lock`, legacy-shaped top-level artifacts, mixed roots, malformed
  roots, and future roots are refused without mutation, SQLite open/recovery,
  or legacy presentation/process inspection;
- the fresh schema contains no browser-setting table, checkpoint-qualified
  onboarding table, or migration-only column/metadata, and its checkpointed
  fixed application-ID/schema-version pair is exact;
- only full UUID Runtime paths are accepted and legacy short paths are refused
  without cleanup or adoption;
- exact current Codex profile ownership is accepted, profile schema 1 is not
  upgraded, and a post-reset profile without current ownership is treated as
  foreign until the explicit setup/removal contract resolves it. Reset
  fixtures cover both complete profile removal and an explicitly quarantined
  provider-settings-only remainder without automatic adoption;
- both providers pass the disposable Shell promotion, attach, `n`, Fork,
  Park/Resume, archive/restore, observer/recovery, presentation restart, and
  complete-cleanup matrix without prompts, transcripts, or ordinary user
  state;
- active current implementation identifiers, CLI help, internal routes,
  artifacts, and schemas no longer expose D16/D17 compatibility names.
  Explicit old-schema refusal tests, D17.1 release tooling, and historical
  evidence retain truthful historical/schema names but cannot compile a
  compatibility entrypoint;
- the current test gate no longer executes a historical D12 presentation
  harness or a checkpoint-named D17 source check, and instead exercises the
  current source/CLI/presentation contract;
- the authoritative design and roadmap contain the current contract and
  concise checkpoint/status indexes rather than duplicated D0-D17 transition
  narratives; every removed historical detail remains reachable as dated
  evidence with preserved provenance;
- `scripts/check`, the declared MSRV, dependency policy, packaging, and clean
  Ubuntu validation pass for the exact artifact; and
- only after separate operator authorization, the exact candidate passes
  disposable live Codex/OpenCode acceptance with complete cleanup; the
  installed D17.1 hash, exact owned-process shutdown, observer removal, atomic
  sibling quarantine as discarded state, exact D18 install, fresh ordinary-
  root bootstrap/trust, installed checksum parity, and separated disposable
  post-install cleanup are each recorded. Explicit authorization to discard the
  old epoch permits reset/install before the isolated provider gate, but D18 is
  not complete until every gate passes.

Deletion is expected to reduce raw source and test counts, so D18 has no
minimum test-count or line-count gate. Coverage of the retained failure,
ownership, lifecycle, and privacy matrix is the gate. A contradiction in any
retained invariant falsifies the consolidation; it does not justify weakening
the product boundary to finish the refactor.

## Git project-root policy

At presentation creation, WSNav captures the invocation cwd as a
presentation-private seed after validating and canonicalizing it as a safe
directory. Every clean provisional shell newly materialized in that
presentation starts at that seed. Detach and reattach preserve a live shell's
actual cwd; they do not reset it. A new presentation captures a new seed. A
missing, deleted, inaccessible, unsafe, symlink-ambiguous, or otherwise
unprovable seed makes onboarding unavailable with bounded guidance. WSNav
never silently falls back to another directory and never treats the seed as a
ProjectLocation or launch authority.

At broker invocation, WSNav performs bounded, read-only Git discovery from the
provisional shell's exact current cwd without contacting a network. The
authoritative operation is equivalent to `git -C <cwd> rev-parse
--show-toplevel`, followed by canonical-path, directory, ownership, and
non-bare-worktree validation under the captured request. A non-Git directory,
bare repository, changed cwd, unsafe path, timeout, or ambiguous result refuses
promotion and leaves the shell interactive. Only this broker-time discovery
creates a ProjectLocation and launch authority; WSNav does not persist
arbitrary cwd history in the host registry.

The returned top-level path is the registered `ProjectLocation` and provider
launch cwd. It is the root of the worktree containing the shell cwd, including
a linked worktree's own root; WSNav never normalizes it to the repository's main
or primary worktree. Two linked worktrees may therefore be distinct Locations,
while optional future read-only common-directory evidence may improve their
display grouping without changing action authority.

The same registration-time inspection may read configured Git remotes without
contacting them. It normalizes a credential-free origin identity into a bounded
fingerprint and safe display label for same-host Project grouping; credentials,
query strings, raw URLs, and transport-specific secrets are neither persisted
nor rendered. The fingerprint is presentation evidence only. It never
associates locations owned by separate wsnav hosts and never authorizes a
filesystem, Git, provider, or Runtime action. Repositories without a safe
origin identity remain valid and receive a host-local Project group.

There is no passive or user-triggered metadata refresh in the current
product. Snapshot, redraw, attachment, switching, resume, and provider cwd
changes perform no Git subprocess and never retarget a Workstream. Same-location
`n`, Resume, and Fork use the exact stored root. Later positive grouping
evidence may be considered only as a separate revision-checked read-only
feature; it cannot change a Location or live Runtime.

After registration, WSNav performs no Git lifecycle operation. It never creates
or removes worktrees or branches; switches a provider into another worktree;
resolves commits; fetches, pulls, commits, merges, rebases, resets, stashes,
pushes, cherry-picks, or copies files. If a task needs an isolated worktree,
the user or provider creates, enters, and manages it through the native
workflow. The Workstream remains pinned to its original ProjectLocation even
if the provider subsequently works elsewhere.

Conversation lineage remains explicit:

```text
source provider session -> forked provider session
```

It makes no claim about filesystem lineage. Parking and archiving stop or hide
the Runtime while preserving provider history and the registered ProjectLocation;
they never inspect or change project files.

## Core workflows

### Onboard a managed session from the provisional shell

```text
user starts a fresh wsnav presentation
-> navigator selects Shell and opens the one presentation-scoped account
   shell with one marker-backed candidate RuntimeId, fresh slot_generation, and
   final full-UUID
   `RuntimePaths` fields (directory, socket, configuration, and session)
-> user changes directory with ordinary shell commands
-> user types codex or opencode, with optional broker-safe native arguments
-> the controlled shell function classifies the bounded argv with that
   provider's closed grammar
-> for a promotable fresh-TUI shape, it invokes the bounded prepare broker as a
   child over presentation-private non-terminal control I/O
-> broker acquires `provisional.lock` and revalidates the
   marker, shell identity, seed/current cwd, presentation revision, and registry generation
-> host detects and validates that current cwd's exact non-bare Git worktree root
-> host revalidates provider readiness and rejects broker-owned or conflicting
   cwd/profile/session/endpoint arguments
-> broker transactionally reserves Project/Location/Workstream authority and a
   Runtime generation for that exact candidate RuntimeId and unchanged
   `RuntimePaths` fields (directory, socket, configuration, and session), then
   marks the handoff issued in the request journal
-> prepare broker returns only an exact one-shot opaque capability; no command
   or argv
-> shell function execs the hidden WSNav launch helper with capability plus
   original bounded argv
-> helper reacquires `provisional.lock` and, while holding it, revalidates every
   bound marker/process/cwd/path/revision/token claim, including candidate
   RuntimeId and each `RuntimePaths` field (directory, socket, configuration,
   and session)
-> only on successful revalidation does it atomically compare-and-consume the
   capability and commit durable `Runtime-owned` authority for that candidate;
   a mismatch does not advance ownership
-> helper revokes/removes presentation cleanup authority before releasing the
   lock; the operation enters `runtime_owned_launching`, and only the existing
   attachment to that Runtime in the originating presentation may remain usable
   (or detach through ordinary card switching)
-> selecting/materializing the fresh derived singleton card attaches only its
   separate provisional server under `provisional.lock` and grants no authority
   over the unproven Runtime; no new attachment to that Runtime is allowed
-> ordinary Park/Resume/Fork/contextual n, archive, Rename,
   recovery/start retry, and cleanup actions for that Runtime refuse or wait
   with bounded onboarding-in-progress guidance
-> helper advances to `provider_exec_started` immediately before `execve`, then
   constructs provider argv internally and attempts provider exec at the
   detected root
-> passive snapshot/action preflight or restart recovery reconciles the same
   RuntimeId/generation and exact `RuntimePaths` fields (directory, socket,
   configuration, and session), tmux pane/session, PID/birth/PGID/session, and
   expected provider executable; only full proof commits
   `provider_exec_proven` and activates ordinary Runtime authority
-> the same private tmux pane and process identity become an ordinarily
   attachable/actionable managed Runtime
-> the selected shell card becomes the managed Workstream card, even when
   native binding is not ready
-> a fresh, unmaterialized Shell card appears
```

Pressing Enter on a later unmaterialized Shell card performs the same guarded
materialization, while Enter on the initial or already materialized card
reattaches its exact existing shell. Reconnecting a detached presentation does
not reset its selected card or replace its current right-hand surface.

Promotion establishes ownership of the managed Runtime, not necessarily the
native session binding, and card/server semantics key off that ownership rather
than provider success. OpenCode's selected launch path prepares its blank root
session before the TUI starts. Any possible `POST /session` effect leaves the
same server Runtime-owned and the card visibly `recovery-required`, even if no
native TUI remains; presentation cleanup cannot touch it and recovery never
issues a second POST. A conclusive pre-effect failure after the exact helper
commit is classified by onboarding recovery, which rolls back attempt-only
graph state only when the provider-specific journal proves no external effect or
binding; the derived singleton card then remains available but unmaterialized. If
OpenCode's blank-session POST or binding already succeeded, recovery retains the
same Runtime, Workstream, and binding for exact resume and never rolls it back or
issues a second POST. A blank Codex TUI may not emit its exact native session
identity until the first prompt, so its promoted row remains `starting` and
unbound until the authoritative `SessionStart` event. It is still a managed
Runtime during that interval and is never eligible for passive session-list
inference.

The hidden launch helper passes only grammar-approved safe native arguments as
an argument vector so authentication, model selection, permissions, and
ordinary provider behavior remain native. The helper owns every argument
needed for identity, observation, working directory, exact session binding, or
OpenCode endpoint ownership. Conflicting forms such as an alternate cwd,
Codex profile or resume target, or OpenCode session/host/port fail before
reservation/provider execution; they are never silently stripped or
reinterpreted. The prepare broker returns only the token that authorizes this
specific helper handoff.

Typing a provider through an escaped path, `command`, a differently named
binary, startup-file alias, or another shell bypass is ordinary unmanaged shell
behavior. WSNav does not kill it or adopt it. The user exits it and invokes the
brokered command when a managed Workstream is desired.

### Open an existing Workstream

```text
user selects Workstream
-> navigator resolves the current host's authoritative registry
-> host confirms runtime generation and tmux session
-> provider pane attachment is replaced
-> the selected provider's native screen redraws from the host runtime
-> no provider input is sent
```

If the runtime is stopped but the native session binding is known, the user
chooses Resume:

```text
host creates a fresh dedicated tmux session at the recorded ProjectLocation root
-> the selected provider adapter launches exact resume with the namespaced
   native session ID and its WSNav-owned launch options
-> provider lifecycle evidence confirms the binding
-> navigator attaches the provider pane
```

### Start an independent Workstream

```text
user selects an existing managed Workstream and presses n
-> navigator retains that Workstream's exact provider and ProjectLocation
-> host records a new independent Workstream at that exact root
-> host launches a blank native provider TUI in a new dedicated Runtime tmux
-> provider-specific binding evidence confirms the native session
-> navigator selects the new Workstream
-> user enters the first prompt in the provider's native composer
```

`n` is deliberately contextual to a selected managed session. It is the fast
path for another blank conversation with the same provider at the same exact
registered root; it does not open a provider chooser, infer another Location,
or copy conversation context. A different provider or directory starts through
the provisional shell. On the provisional shell card, `Enter` opens or focuses
the shell and `n` performs no separate action. An archived Workstream must be
restored before it can be the source of `n`.

No workstream name, model, branch, session ID, or first prompt is required in a
manager-owned creation form. Before binding, the row shows
`starting`; a bound but unnamed tip shows `untitled`. Later native `/rename`,
navigator Rename, or an opt-in Codex naming skill updates the one
Codex-owned thread name.

### Fork a running Workstream

The action means “explore another approach from the latest settled conversation
state.” It does not fork partial model output or current filesystem state.

```text
source provider turn may still be running
-> user explicitly selects Fork Workstream
-> host validates the source binding and last settled provider boundary
-> durable Fork operation records the provider-only cutover plan at the same project root
-> the provider adapter forks the source through its exact settled boundary
-> if the provider exposes a native name, host sets a bounded provisional fork name
-> host launches the provider's exact resume shape for the returned destination
   session ID at the same ProjectLocation
-> provider lifecycle evidence confirms the new native session
-> source runtime continues unchanged
-> navigator may focus destination; source completion only raises attention
```

If the selected provider contract cannot prove a settled-prefix fork for a live
source, the action is unavailable. The user can still start an independent
Workstream. Codex uses its ephemeral App Server and exact `lastTurnId`
boundary; OpenCode uses the validated settled `messageID` boundary. These are
adapter details, not a generic provider command or a source of provider
identity.

### Native Codex thread management

Inside the provider pane, the user continues to use Codex:

- `/rename` for the same canonical thread name shown by the navigator;
- `/clear` for a fresh chat in the same Workstream;
- `/fork` for a native chat fork that remains in the same Workstream unless the
  user explicitly creates a separate Workstream; and
- native Plan choices, including current-thread implementation or clear-context
  implementation.

Workstream Navigator observes a new session binding when possible. It does not
infer that a native chat transition created a new task or Workstream. A
verified D1.5 same-Workstream `/clear` cutover displays the prior effective
name provisionally when the new thread is unnamed, but does not write that
fallback into Codex. Native `/new` is unsupported inside a managed Runtime:
although it creates a Codex thread, WSNav has no exact authority to bind it and
retains the previous tip. The user must use `/clear` for the same Workstream or
use WSNav Start/Fork for a separate Workstream. WSNav must not infer recovery
from App Server inventory or `thread/list` ordering. Other native transitions
remain visible in Codex history but do not replace the WSNav binding until their
event contracts are separately validated.

### Multi-host composition

Multi-host use is deliberately outside the WSNav control plane. The operator
opens an ordinary SSH connection to another machine in a separate terminal,
tab, or window, then starts `wsnav` on that machine. After SSH establishment,
all WSNav control work for switching, contextual observer readiness, Runtime
lifecycle, and recovery is local to that host's wsnav instance. Terminal
rendering and input still traverse the operator's SSH connection and retain
ordinary network and SSH latency. The instances do not register one another,
exchange snapshots, merge Projects, synchronize state, or transfer sessions,
and they need no cross-host WSNav release or protocol parity. Closing the outer
SSH connection may end that host's disposable presentation, but it does not
stop, park, rotate, or restart its private Runtime/provider; reconnecting and
rerunning wsnav reattaches it.

## Navigator experience

The default view is intentionally small:

```text
Shell
  ~/c/workstream-navigator
Project
├── Tip thread name         working
├── Prior name ↻ unnamed    working
├── Source · fork           result ready
└── untitled                parked

┌ navigator ┐┌────────────── native provider TUI ──────────────┐
│ tree      ││ directly interactive; no manager-owned chrome   │
│ status    ││ inside the provider surface                     │
└───────────┘└──────────────────────────────────────────────────┘
```

Required interactions:

- keyboard and mouse selection in the navigator;
- direct keyboard and mouse interaction in the provider pane;
- one action to focus or reconnect a Workstream;
- keep exactly one provisional shell card visible, lazily open its account
  shell, and promote it in place only through an exact brokered provider launch;
- detect the broker cwd's exact Git worktree root and register it atomically
  with the first managed Workstream without a Project browser or path form;
- Start another independent Workstream from a selected managed Workstream at
  its exact registered root and with its same provider;
- Fork Workstream from an exact managed source;
- inspect bounded Workstream status and rename the current tip through Codex's
  canonical thread-name field;
- park/resume without deleting provider history;
- archive a Workstream out of the active list and restore it without starting
  Codex or deleting its retained state;
- route a repeated Fork to its exact unresolved operation and reconcile it;
- detect observer readiness without mutation and guide the user contextually
  through explicit-consent profile preparation and native trust review only
  when a requested Codex operation requires it;
- show the derived current-host label outside provider content, with exact
  visual treatment deferred to a later UX checkpoint; and
- acknowledge result or recovery attention without injecting provider traffic.

The normal human workflow begins with bare `wsnav` and requires no later
`wsnav` command typed by the user. The apparent `codex` and `opencode` shell
commands are controlled functions that use the two-phase presentation-private
broker and hidden launch helper described above; this is product interaction,
not a public CLI workflow. Public CLI equivalents for supported non-creation
actions remain available for scripting, diagnosis, direct attachment, and
break-glass recovery. There is no public Workstream creation or arbitrary
registration command. The documentation and empty states never send the user
to those commands for an ordinary WSNav operation.
Installing or upgrading the host-local executable, establishing an outer SSH
connection, cloning repositories, native provider input and any provider-
specific observer/trust approval, and deferred Git cleanup remain external
prerequisites or explicitly excluded operations.

The Workstreams page has one pinned provisional shell card plus one
Project-grouped active projection; Archived is a separate direct page rather
than a view mode.
Archive is the ordinary answer to accumulated test or inactive Workstreams;
there is no hard-delete action. Project groups disappear from Workstreams when
they have no active Workstreams, while their archived Workstreams remain
available through Archived. A dormant Location with no retained Workstream has
no ordinary standalone navigator row. Archiving a working Runtime requires
explicit confirmation because parking it interrupts the current provider turn.

An unfinished Fork belongs to its already-visible source Workstream rather than
a normal global management page. Pressing `f` on that source opens a focused
choice to reconcile the exact saved operation or deliberately start another
Fork. If several unfinished Forks share the source, a bounded chooser lists
only those candidates. The source Workstream ID is transient routing metadata,
never rendered; request keys, paths, provider identifiers, and raw evidence
remain hidden. A recovered destination opens directly in the native provider
pane.

Projects remain durable presentation groups behind Workstream and Archived
rows, but WSNav provides no Projects page or Project-level action surface.
Registration resolves the provisional shell cwd locally for Git inspection;
no path is written into provider panes or public Workstream snapshots.
Credential-free origin matching may preserve same-host grouping, but manual
refresh, cross-host merge/split, permanent Project deletion, and repository
cleanup remain outside the product.

There is no Project-level hide, forget, remove, or `x` action. Workstream
archive/restore is the one reversible visibility mechanism, so an archived
Workstream never becomes unreachable from the ordinary TUI behind a second
hidden layer. Project and ProjectLocation deletion, repository cleanup, and Git
mutation remain outside the product.

There is no Projects, Hosts, or settings page. Provider capability and observer readiness
appear only as bounded context in the operation that needs them. If observer
review is required, its native profile-selected Codex TUI runs in the right
provider pane through the same host-local terminal boundary and leaves no
Workstream behind. The user alone approves trust; preparation never writes
trust state. Exact diagnosis may be surfaced in the contextual refusal, while
removal remains the exceptional documented cleanup flow defined above.

Navigator page changes, forms, and finite management actions leave the current
provider attachment and focus unchanged. Only an explicit Workstream primary
action, provisional-shell selection, or observer review replaces the right
pane. Potentially slow Git detection, provider launch, provider metadata, and
observer actions expose bounded progress in the navigator, suppress duplicate
submission, and commit only an exact current revision; they never freeze
silently or print management output into the provider pane.

The navigator does not ask for model IDs, session IDs, branch names, request
IDs, or a mandatory title in the ordinary path.

A direct mode, such as `wsnav attach <workstream>`, bypasses the navigator pane
while using the same host/runtime contracts.

## Failure and recovery model

| Failure | V1 behavior |
| --- | --- |
| Normal local tmux detach and reattach to the same owned presentation | Preserve the exact provisional shell server, pane, process, actual cwd, and pending request; never create a duplicate shell. Every managed host Runtime also continues |
| Confirmed presentation close | Acquire the shared `provisional.lock` lease and revalidate marker, journal, and revisions. Before the helper successfully revalidates every bound marker/process/cwd/path/revision/token claim and atomically consumes the capability while committing durable `Runtime-owned` authority, close may win only by atomically revoking the unconsumed capability and proving pre-effect absence; then roll back attempt-only rows and terminate only exact provisional artifacts. After that exact helper commit, never signal that server; managed Runtime servers and provider processes continue |
| Conclusive presentation loss | Under the same `provisional.lock` lease, clean only exact pre-handoff provisional artifacts whose ownership and pre-effect absence are proven; after the exact helper commit leave the Runtime-owned server untouched and let onboarding recovery reconcile. After conclusive cleanup, the next presentation's derived singleton card is available but unmaterialized; ambiguous evidence leaves it unavailable. Managed Runtime servers and provider processes continue |
| Runtime-owned onboarding before `provider_exec_proven` | Fence attachment/action authority for that unproven Runtime. Its originating presentation may retain its existing tmux Runtime attachment/pane or detach through ordinary card switching, but no new attachment to that Runtime is allowed. Selecting/materializing the fresh derived singleton card attaches only its separate provisional server under `provisional.lock` and grants no authority over the unproven Runtime. Refuse or wait on ordinary Park, Resume, Fork, contextual `n`, archive, Rename, recovery/start retry, and cleanup for that Runtime with bounded `onboarding-in-progress` guidance. Passive snapshot/probe may show `starting`/`onboarding` and reconcile, but never adopts the helper/preparation process, marks the Runtime lost, or signals it |
| Hidden helper exits before `provider_exec_started` | Reconcile the exact journal and classify a conclusive no-effect exit as known-absent; never infer provider identity or expose ordinary Runtime action from the helper process |
| `execve` returns an exact error | Record terminal known-absent failure for the final provider TUI exec before helper exit when possible; the reconciler grants no action from that evidence alone and ends onboarding through guarded rollback only when provider-specific journal evidence proves no prior effect or binding, or through the exact stopped/recovery state when a known OpenCode binding must be preserved |
| Crash after `provider_exec_started` without proof | Leave the Runtime and operation ambiguous/recovery-required; a possible live provider is never rolled back, and no second provider effect is attempted |
| Reconciler proves provider exec | Under the exact operation/revision, RuntimeId/generation and exact `RuntimePaths` fields (directory, socket, configuration, and session), tmux pane/session, PID/birth/PGID/session, and expected executable proof, atomically commit `provider_exec_proven` and activate ordinary attachment/action authority; Codex may remain `starting` and unbound until `SessionStart` |
| OpenCode has known blank-session binding but final TUI exec fails | Retain the same Runtime, Workstream, and binding for exact recovery/resume; never roll them back or issue a second POST. A possible POST effect remains `recovery-required` |
| `provisional.lock` is missing, malformed, symlinked, foreign, replaced, locked, or busy in `ready` | Fail closed with bounded onboarding guidance; never create a second lock, proceed unlocked, unlink/recreate the stable artifact, or mutate the marker/journal |
| Schema-15 bootstrap reaches `provisional_pending` | Create-new or reacquire only the precommitted private provisional inode, commit/sync database `ready`, then advance/sync `bootstrap.lock`; mismatched or replacement evidence fails closed |
| `provisional.lock` holder crashes or the host restarts | The kernel lock releases without changing the mode-`0600` file; a ready root reacquires only the same database- and bootstrap-bound inode and reconciles marker/journal under its lease generation |
| Singleton marker/journal/path/process evidence is missing, changed, multiple, unknown, or ambiguous | Block all fresh materialization and leave every artifact untouched; do not evade ambiguity with a new UUID or adopt/delete an unknown `run/runtime-*` artifact |
| Stale onboarding rollback races fresh-card selection/materialization | Reconcile only the old operation, Runtime, and `slot_generation`; leave a newer marker/card unchanged and derive at most one unmaterialized singleton card |
| Ambiguous presentation ownership or loss | Leave every artifact untouched, fail closed with bounded unavailable guidance, and block a duplicate provisional shell until exact ownership is resolved; preserve every managed Runtime |
| Outer SSH detach or loss | Apply the same detach/close/loss rules to the host-local presentation: reattach the same owned presentation when it survives, clean only a conclusive provisional loss, and never stop a managed Runtime |
| Presentation seed cwd is missing, deleted, unsafe, or ambiguous | Mark onboarding unavailable with bounded guidance; never fall back to another cwd, create a ProjectLocation, or launch a provider |
| Provisional shell exits or the user cancels before the exact helper commit | Leave no durable Project, Location, Workstream, Runtime, or provider binding after onboarding recovery; when prior artifacts are clean, the derived singleton card remains available but unmaterialized. After the exact helper commit, recovery owns classification and cleanup |
| Broker is invoked outside a valid non-bare Git worktree | Refuse promotion with bounded shell-local guidance; keep the same shell interactive and create no durable record |
| Provider command bypasses the broker | Treat it as an unmanaged shell process; never adopt it from process, pane, hook, or session evidence |
| Brokered launch fails conclusively before provider effect | Onboarding recovery rolls back graph records created only by that attempt when provider-specific journal evidence proves no external effect or binding; the derived singleton card remains available but unmaterialized, and presentation close/loss does not infer this rollback |
| Brokered promotion becomes ambiguous after an external-effect boundary | Keep the same Runtime-owned server and a visible recovery-required managed Workstream, reconcile its durable operation, and never hide it as a clean retry or issue a second OpenCode POST |
| Exact private runtime tmux server is gone | Mark that Runtime `recovery_required`; exact native resume may create a new runtime generation |
| Codex process exits normally | Keep Workstream and provider binding; offer exact native resume |
| Observer hook is absent or missed | Show `unknown`; retain live attach; block exact fork/recovery if session identity is unknown |
| Hook identity cannot be corroborated | Do not rotate the ProviderBinding; show `unknown` or `recovery required` |
| Hook events race | Resolve by runtime generation, session ID, turn ID, and transactional state; conflicting evidence becomes `unknown` |
| Exact name read returns empty | Record `known_empty` and compute the context-specific fallback |
| Name refresh is unavailable | Keep the dedicated TUI untouched and retain the cached native name with stale provenance |
| Ephemeral App Server mutation is ambiguous | Reconcile exact persisted effects; never retry a non-idempotent fork unless absence is proven, otherwise require recovery |
| Another client or direct tmux client attaches | Show the same tmux-managed screen; do not create a lease or detach either client; simultaneous input may interleave |
| Navigator crashes during focus switch | Focus is ephemeral; no durable runtime or Workstream mutation is implied |
| Navigator disconnects during Start or Fork | Start is already committed locally; reopen the exact Fork operation only when provider cutover is unresolved |
| Provider changes directory or creates, enters, or removes a worktree | Leave Git and cwd state entirely to the provider or user; keep the Workstream pinned to its launch-time ProjectLocation and perform no passive Git inspection |
| Host registry identity or generation evidence is ambiguous | Reject the affected mutation and require explicit local diagnosis or recovery |
| Host database is absent beside any state-root artifact | Return typed `state recovery required`; never mint a HostIdentity, adopt or signal a Runtime, remove a presentation, or clean an unknown artifact |
| `wsnav-observer` is absent or awaiting trust | Preserve existing Runtime attachment. Before a provisional Codex broker reservation, ask consent in the shell and complete native review; for a managed action capture the exact pending intent. Continue either path only after exact readiness and revision revalidation |
| `wsnav-observer` is foreign, modified, disabled, or ambiguous | Preserve it and existing Runtime attachment; refuse the observer-dependent request with exact contextual diagnosis and retry guidance |
| Profile update or exceptional removal is requested while a managed Runtime is live | Refuse the integration change until all WSNav-managed Codex Runtimes on that host are parked or stopped; do not block attachment |

Result completion and the sticky AttentionState update must commit in one host
transaction. This directly avoids the Python prototype's split
result/attention persistence gap.

### Durable operation recovery

An unresolved Fork remains host-private until the user repeats `f` on its
source Workstream. WSNav then routes to the exact saved operation using the
already-known source Workstream ID, without rendering that ID, request keys,
project paths, provider IDs, or raw operation evidence. `recover-operation
<id>` remains direct-CLI parity for diagnostics and break-glass use. A Fork
with no recorded provider-attempt marker may continue to the one permitted fork
call; after that marker exists it may only reconcile exact provider lineage and
can never call `thread/fork` again. Zero or multiple candidates remain
recovery-required; multiple candidates get a bounded source-scoped chooser,
never automatic selection.

This recovery path is intentionally separate from native Runtime recovery:
`recover <workstream>` resumes a known Codex thread after a lost private tmux
Runtime, while `recover-operation <id>` resolves an incomplete external
creation effect before a destination Workstream can safely exist.

## Security and privacy

- State roots are user-private; directories use mode `0700` and files use
  `0600`.
- Every live Runtime owns a private tmux socket and server with exactly one
  session, window, and pane; these sockets never reuse the user's ordinary
  socket.
- Management commands use `env -u TMUX tmux -S <absolute-runtime-socket>` and
  never bare `tmux` or `tmux -L`. A native provider retains the private `TMUX`
  environment by design, so a bare `tmux ls` inside it sees at most that one
  Runtime. Spike 0005 accepted this terminal configuration.
- Finite host-local control commands (tmux probes/actions, Git, and child
  WSNav actions) drain stdout and stderr concurrently while retaining
  only their explicit per-stream bounds. They also have wall-clock deadlines
  and terminate their complete process group on timeout. Direct provider
  attachment is a terminal stream, not captured child output.
- Private tmux sockets are a namespace and accidental-discovery boundary, not
  a same-user security boundary. Workstream Navigator does not prevent a user
  who knows the socket path from attaching or stopping the Runtime.
- WSNav opens no listener and does not inspect, configure, or manage SSH
  authentication, forwarding, `known_hosts`, or outer terminal connections.
  Ordinary SSH composition remains the operator's boundary.
- Managed Codex TUIs never use `codex --remote`, and Workstream Navigator never
  starts a persistent Codex App Server transport.
- Managed Codex TUIs use the normal user `CODEX_HOME` plus the exactly owned
  `wsnav-observer` profile. The generated profile is mode `0600`, adds only
  passive lifecycle hooks, and is selected only for WSNav launches.
- Hook trust is a native Codex user decision. WSNav neither edits the trust
  store nor bypasses trust review.
- Ephemeral provider helpers use private I/O, a distinct proven process group,
  bounded request and shutdown deadlines, and forced cleanup when graceful
  shutdown fails. A helper that can cross a non-idempotent provider boundary
  must also have bounded cleanup authority that survives abrupt loss of its
  owning WSNav action; normal-return cleanup alone is insufficient.
- Provider and Git commands are built as argument vectors. Thread names and
  paths never become shell fragments.
- Provisional-shell provider functions send only the bounded provider kind,
  request key, exact cwd, and grammar-approved argument vector over
  presentation-private control I/O. The prepare broker returns an exact
  one-shot capability bound to the request, presentation/slot, candidate
  RuntimeId and unchanged full-UUID `RuntimePaths` fields (directory, socket,
  configuration, and session), provider, cwd/root/Location, Runtime generation,
  revisions, shell process identity, argv digest, and short monotonic expiry.
  Persisted state keeps only its bounded
  identifier/verifier/phase and claim references or digests; no live token,
  argv, shell command line, history, environment, terminal capture, or
  provider output is persisted. Secret-bearing argv cannot enter this path.
- The stable host-private `provisional.lock` serializes provisional materialization,
  close/loss cleanup, broker preparation, helper consume, singleton reconciliation,
  and marker cleanup. It is bootstrap-bound operational rather than
  presentation-private state and contains only its bounded format, HostId, and
  lease generation. Every actor opens it no-follow/CLOEXEC,
  retains one nonblocking exclusive kernel-lock FD, and revalidates canonical
  root, pathname, and FD device/inode identity before mutation. A prepared
  reservation alone does not revoke cleanup; before the successful helper
  commit, close may win only by atomically revoking an unconsumed capability and
  proving pre-effect absence. While holding the lock, the helper revalidates
  every bound marker/process/cwd/path/revision/token claim, including exact
  `RuntimePaths` fields (directory, socket, configuration, and session); only
  then does it atomically compare-and-consume the
  capability and commit durable Runtime ownership. A mismatch does not advance
  ownership. It then revokes presentation cleanup; only afterward may provider
  effects occur. Replay, expiry, duplicate helpers, busy/timeout, or any
  mismatch fails closed. Unknown or multiple markerless/registryless
  `run/runtime-*` artifacts remain untouched, and no Runtime action or attach is
  exposed while the onboarding operation is before `provider_exec_proven`.
  Process observation, provider hooks, pane text, native inventory, and shell
  bypasses remain evidence only and can never adopt or promote a process.
- Hook stdin is fully drained even for unmanaged, stale, oversized, or malformed
  events.
- Hook payloads, prompts, transcripts, terminal screens, credentials, process
  environments, and raw external diagnostics are not logged or committed.
- An observer-degraded marker stores only its format version, typed RuntimeId,
  Runtime generation, and closed failure-reason enum. It never stores the
  failed event, native session or turn/message ID, payload, response, or error
  text, and it cannot authorize replay.
- App Server `preview`, turns, items, transcript paths, raw responses, and
  process-local runtime status are discarded on the owning host.
- Explicit navigator actions or declared managed-session policies authorize
  mutation. Hooks, tmux metadata, screen text, agent shell commands, and
  same-user socket calls are observations only.
- Provider identity used by a later explicit resume or fork must be
  launch-correlated and corroborated; an untrusted observation cannot replace a
  known binding.
- Every Runtime carries a generation and process-birth fingerprint so stale
  hooks and attachments cannot silently bind to a replacement process.
- V1 exposes no Git mutation. Project registration is read-only discovery;
  every later Git decision stays inside the native provider session or
  ordinary user tooling.

## Current Rust structure

```text
src/
├── main.rs, app/             thin CLI parsing and dispatch
├── domain/                  typed IDs, lifecycle, operations, invariants
├── state/
│   ├── current/             schema/bootstrap, registry, onboarding,
│   │                        observer, and Project/Location projection
│   └── ...                  retained compound/lifecycle/runtime primitives
├── navigator/
│   ├── view.rs              Shell-first Workstreams/Archived Ratatui model
│   └── controller.rs        action, attachment, review, reconciliation loop
├── presentation/
│   ├── ownership.rs         exact private presentation identity
│   ├── topology.rs          two-pane role and geometry validation
│   ├── control.rs           bounded tmux control
│   ├── attachment.rs        shell/provider surface attachment
│   ├── provisional.rs       marker-backed provisional lifecycle
│   └── cleanup.rs           exact presentation teardown
├── provider/
│   ├── lifecycle.rs         narrow provider-neutral observation contract
│   ├── grammar.rs           closed fresh native-TUI invocation grammar
│   ├── codex/               profile, hooks, and ephemeral App Server
│   └── opencode/            Runtime HTTP/SSE and observer guardian
├── account_shell.rs         controlled Bash/Zsh account-shell bootstrap
├── onboarding_broker.rs    promotion preparation and capability issuance
├── onboarding_helper.rs    exact capability consume and provider exec
├── provider_reconcile.rs   post-effect provider-specific reconciliation
├── provisional.rs          presentation marker and singleton recovery
├── runtime/                private tmux/process ownership
├── repository.rs           bounded promotion-time Git-root discovery
└── snapshot.rs             bounded current navigator projection
```

Generic host clients, remote endpoints, framed control protocols, SSH
adapters, client catalogs, transition openers, and checkpoint-qualified
modules are absent. The provider-neutral lifecycle types carry only the
bounded observations shared by the concrete adapters; they do not form a
speculative provider framework.

## Validation and acceptance contract

Repository acceptance combines:

- direct schema-15 bootstrap failure injection and raw old/future/malformed
  refusal evidence;
- typed lifecycle, onboarding, observer, operation, and revision tests;
- disposable private-tmux presentation/topology/cleanup tests;
- provider grammar, bounded I/O, process identity, and exact recovery tests;
- generated public-help and semantic source/module inventory;
- formatting, strict Clippy, packaging, dependency license/advisory/source
  policy, and staged/unstaged diff checks; and
- the declared MSRV and equivalent clean Ubuntu jobs for an exact committed
  candidate.

Tests use disposable state roots, repositories, provider homes, and private
tmux sockets. Live Codex/OpenCode acceptance, observer trust, installation,
and ordinary-root destructive reset require separate operator authorization.
Passing evidence is sanitized to versions, checksums, closed assertion
categories, timings, and cleanup relationships; it contains no native IDs,
paths, prompts, results, pane capture, credentials, transcript, or raw
provider payload.

A narrow test passing does not prove a broader gate. D18 completion requires
the coherent matrix in the active roadmap. The reset establishes absence for
exact WSNav-owned processes and private servers before quarantining discarded
state; it does not claim authority over arbitrary filesystem holders.


## Checkpoint index

Delivery history does not define current behavior. The active checkpoint and
exit gates live in [`roadmap.md`](roadmap.md); the complete pre-slimming
roadmap and version-specific decisions remain in
[dated evidence](roadmap-through-d18-design.md).

| Checkpoint family | Durable outcome | Evidence |
| --- | --- | --- |
| D0-D7 | Core state, native Codex Runtime, Navigator, recovery, and operator beta | [Acceptance index](evidence/README.md#acceptance-records) |
| D8 | Concrete Codex/OpenCode provider contract | [D8.1](evidence/acceptance/d8.1-multi-provider.md), [D8.2](evidence/acceptance/d8.2-opencode-fork-recovery.md) |
| D9-D15 | Reliability, presentation, interaction, and terminal refinements | [Archived roadmap](roadmap-through-d18-design.md) |
| D16 | Host-local clean break | [D16 acceptance](evidence/acceptance/d16-host-local.md) |
| D17-D17.1 | Shell-first onboarding and correctness closure | [D17](evidence/acceptance/d17-shell-first.md), [D17.1](evidence/acceptance/d17.1-correctness-closure.md) |
| D18 | Current-only schema-15 consolidation | [Completed roadmap](roadmap.md#completed-checkpoint-d18-current-only-consolidation) |

## Current concrete provider boundary

`ProviderKind` is typed and persisted on every Workstream, Runtime, provider
binding, and operation that needs it. Codex and OpenCode remain concrete
adapters, not interchangeable protocol implementations. The provisional
shell's native `codex` or `opencode` command selects the provider for a new
Location; contextual `n` and Fork inherit the selected Workstream's exact
provider and Location. There is no provider picker, default-provider setting,
or fallback from one provider to another.

Codex lifecycle identity comes from bounded passive hooks plus exact Runtime
and App Server evidence. OpenCode uses its exact loopback Runtime handle and
one generation-bound observer sidecar. Neither adapter persists prompts,
responses, terminal content, raw payloads, or transcripts, and neither
observer is mutation authority.


## Evidence basis

- [Spike 0001: tmux remote-session transport](evidence/spikes/0001-tmux-remote-transport.md)
- [Spike 0002: native Codex TUI over remote tmux](evidence/spikes/0002-codex-native-tui.md)
- [Spike 0004: per-Workstream tmux runtime isolation](evidence/spikes/0004-tmux-runtime-isolation.md)
- [Spike 0005: native Codex two-pane terminal presentation](evidence/spikes/0005-codex-terminal-presentation.md)
- [Spike 0006: scoped Codex observer profile](evidence/spikes/0006-codex-observer-profile.md)
- [Spike 0007: ephemeral Codex metadata and naming](evidence/spikes/0007-codex-app-server-naming.md)
- [Spike 0008: running-source settled-prefix fork](evidence/spikes/0008-codex-running-settled-fork.md)
- [Spike 0015: OpenCode provider feasibility](evidence/spikes/0015-opencode-provider-feasibility.md)
- [Spike 0016: OpenCode native Runtime contract](evidence/spikes/0016-opencode-runtime-contract.md)
- [Spike 0017: OpenCode blank-session binding](evidence/spikes/0017-opencode-fresh-session.md)
- [Spike 0019: brokered onboarding shell](evidence/spikes/0019-brokered-onboarding-shell.md)
- [Spike 0020: OpenCode 1.18.23 revalidation](evidence/spikes/0020-opencode-1.18.23-revalidation.md)
- [Python Phase 7F terminal evidence](https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/phase-7f-acceptance.md)
- [Study 0003: Codex App Server runtime boundary](evidence/studies/0003-codex-app-server-runtime-boundary.md)
- [Study 0004: Herdr 0.8.0 competitive comparison](evidence/studies/0004-herdr-v0.8-comparison.md)
- [D6 source-installed operator-beta acceptance](evidence/acceptance/d6-operator-beta.md)
- [Current Codex CLI commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- [Current Codex configuration profiles](https://learn.chatgpt.com/docs/config-file/config-advanced#profiles)
- [Current Codex App Server](https://learn.chatgpt.com/docs/app-server)
- [Current Codex lifecycle hooks](https://learn.chatgpt.com/docs/hooks)
- [DMS Agent Picker](https://github.com/byebyebryan/dms-agent-picker)
- Frozen Python reference:
  [checkpoint](https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/python-reference-checkpoint.md),
  [Phase 7A contract](https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/phase-7a-contract.md),
  and
  [Herdr assessment](https://github.com/byebyebryan/agent-switchboard-python-reference/blob/main/docs/herdr-assessment.md)

The current Codex documentation confirms native `resume`, `/new`, `/clear`,
and `/rename` flows; App Server `thread/read`, `thread/name/set`, and exact-turn
`thread/fork`; plus lifecycle hook fields for session, turn, cwd, start source,
prompt submission, and stop. The design uses those interfaces narrowly and
treats installed behavioral spikes as the final capability authority. In
particular, the documentation that `/new` starts a new chat does not establish
an exact live-Runtime transition; [Spikes
0011](evidence/spikes/0011-codex-native-new-rebinding.md),
[0012](evidence/spikes/0012-codex-new-prompt-session-rotation.md), and
[0013](evidence/spikes/0013-codex-new-thread-inventory.md) retain the
unsupported boundary until an authoritative binding contract exists.
