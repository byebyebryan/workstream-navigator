# Workstream Navigator V1 Design

Date: 2026-08-01

Status: implemented operator-beta contract through D8.2; no compatibility
contract

The design is the current product and architecture contract. Dated acceptance,
spike, and study records preserve the evidence and limitations of the candidate
they tested; their historical version numbers, test counts, and presentation
details do not supersede this contract.

## Product thesis

Workstream Navigator is a thin terminal navigator for persistent coding-agent
workstreams across hosts. It adds organization, attachment, status, and a few
compound workstream actions around the provider's native terminal UI.

It is not a replacement terminal, provider frontend, task manager, transcript
store, project-memory system, or autonomous agent orchestrator.

The central design rule is:

> Workstream Navigator owns where work runs and how the user reaches it. The
> provider owns the conversation and how the user works inside it.

The tmux/SSH and native Codex spikes establish that this split is technically
viable. A remote Codex process can remain alive behind a dedicated tmux server,
accept native terminal input, survive local detach and reconnect, and preserve
its completed visible result.

The retained two-server presentation can show minor cursor artifacts in Ghostty
during high-churn native TUI activity such as typing or streaming. After
removing WSNav's continuous runtime and presentation control probes, that
residual is accepted as non-blocking V1 visual polish: it does not alter input,
provider output, result retention, or provider ownership. The artifact is
caused by upstream tmux behavior ([tmux issue
5419](https://github.com/tmux/tmux/issues/5419)): every full client redraw
emits `civis`/`cnorm` cursor-visibility toggles, which repeatedly restart the
cursor blink phase in the nested path. This is version-bound and unfixed in
every tmux currently available on Arch (`3.7b` is the latest released; the AUR
`tmux-git` package is stale and upstream master lacks the fix), so WSNav
configures its private servers with the best available settings and defers a
fix until the upstream release lands. The A/B instrument, the ruled-out
workarounds, and the decision gates are recorded in
[Spike 0014](evidence/spikes/0014-terminal-fidelity-a-b.md) and the
[roadmap](roadmap.md#2026-08-04-terminal-fidelity-root-cause-is-upstream-tmux).

## V1 tenets

1. **Preserve the native provider workflow.** Codex owns its composer, models,
   permissions, Plan choices, `/new`, `/clear`, `/fork`, `/rename`, resume,
   history, and transcripts.
2. **Augment instead of intercepting.** Normal work does not pass through a
   manager-owned prompt box, plan router, session wizard, or model picker.
3. **Keep the completed result visible.** Workstream Navigator never writes
   status, routing, synthesis, or completion traffic into the provider pane.
4. **Make workstreams explicit.** A workstream is an independent provider and
   runtime lane at a registered project root, not a task record, filesystem
   owner, or synonym for a provider chat.
5. **Treat hosts as locations, not agents.** Local and SSH hosts use the same
   runtime contract. V1 does not transfer repositories, chats, or task context
   between hosts.
6. **Fail visibly and conservatively.** Unknown provider identity, runtime
   ownership, provider identity, or remote state becomes `unknown`,
   `unreachable`, or `recovery required`; it is never guessed.
7. **Keep provider history canonical.** Workstream Navigator stores provider
   identifiers needed for exact resume, but no prompts, responses, tool output,
   transcript copies, or rendered-history substitute.
8. **No legacy constraints.** The Python prototype is behavioral evidence only.
   V1 has no schema, command, state, or compatibility obligation to it.
9. **Keep ordinary operation inside the TUI.** After WSNav and its declared
   external prerequisites are installed, a user can perform every ordinary
   WSNav-owned catalog, lifecycle, recovery, and observer action from the
   two-pane navigator. Direct CLI commands remain optional scripting,
   diagnostics, and break-glass parity, never a required normal workflow.

## V1 scope

### Included

- Codex and contract-compatible OpenCode local and SSH-host operation through
  the bounded D8 provider-aware New, exact resume, same-provider Fork, and
  lost-Runtime recovery contract. Real OpenCode acceptance currently covers
  `1.18.11`; release numbers are diagnostic evidence, not compatibility
  authority.
- A minimal two-pane terminal experience: navigator beside the directly
  interactive native provider TUI.
- Explicit host registration and capability checks.
- Logical projects with one or more explicitly registered host locations.
- Host-private Project-directory browsing for ordinary registration, rooted at
  each host's configurable `~/code`-style workspace root.
- Workstream creation, switching, parking, exact resume, and display through
  the current tip's provider-owned native name when that metadata surface is
  supported.
- Navigator-local Workstreams, Projects, and Hosts pages, with Workstreams as
  the default operational home rather than a generic management dashboard.
- Reversible Workstream archive and restore for removing inactive work from the
  ordinary navigator without deleting provider history or Git state.
- Independent workstreams started at a registered project root.
- Conversation-forked workstreams that retain the same registered project root.
- Read-only Git registration and optional remote fingerprinting for Project
  grouping; no Git lifecycle ownership.
- Activity and durable result attention for Workstream Navigator-started
  provider sessions.
- Navigator-owned observer activation, native trust review, status, and exact
  removal of the observer integration from one dedicated Codex profile on each
  managed host; an accepted provider-owned model prefix survives removal.
  OpenCode uses the separate read-only per-Runtime sidecar contract in D8.1 and
  has no generic onboarding flow.
- Reconnection after local UI or SSH loss.
- Recovery after the host tmux runtime disappears, using the provider's native
  session identity.
- TUI access to every ordinary WSNav-owned action, with optional direct CLI
  equivalents for scripting, diagnostics, and recovery.
- Multiple same-user attachment points to one provider runtime, using tmux's
  native shared-screen behavior without a separate input-lease system.

### Explicitly outside V1

- Importing or controlling arbitrary existing provider sessions.
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
- A public network service or always-running remote daemon.
- Cloning repositories, managing worktrees, synchronizing repositories, moving
  a live workstream between hosts, or transferring chats between hosts or
  providers.
- Automatic Git fetch, pull, commit, merge, rebase, reset, stash, push,
  cherry-pick, or conflict resolution.
- Copying files, commits, branches, or worktrees between Workstreams.
- Hard deletion of Workstream records, native provider sessions, or project
  files. Archive is visibility and retention, not cleanup authority.
- Automatic remote installation, upgrade, repository cloning, or host-wide
  teardown from the Hosts page.
- Claude or provider parity beyond the explicitly bounded OpenCode D8 scope.
- Multiple-controller catalog synchronization. Each navigator client may
  independently reconstruct the same presentation grouping from host-supplied
  repository fingerprints, but clients do not replicate their catalogs.

Cross-host operation in V1 means that one navigator can see, start, attach to,
and resume work at explicitly registered project locations on several hosts.
It does not mean that one workstream migrates between them.

## Concepts and ownership

| Concept | Meaning | Canonical owner |
| --- | --- | --- |
| `Host` | A local or SSH-reachable machine with `wsnav`, tmux, Git, and dynamic provider capabilities | Workstream Navigator client catalog plus host handshake/snapshot |
| `Project` | A logical repository-shaped grouping shown by the navigator | Workstream Navigator |
| `ProjectLocation` | One registered Git project's canonical root on one host | That host's Workstream Navigator registry |
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
local terminal
└── dedicated local tmux presentation session (disposable)
    ├── navigator pane
    │   └── wsnav TUI
    └── provider pane
        └── wsnav attach helper
            ├── local host: wsnav host helper -> exact runtime tmux server
            └── SSH host: ssh -tt -> remote wsnav helper -> exact runtime tmux
                                                       └── native provider TUI

wsnav TUI
├── local client catalog
├── local host adapter
└── SSH host adapters
    └── fixed, versioned JSON protocol over SSH stdin/stdout

each managed host
├── private SQLite state
├── one private tmux server per live workstream runtime
│   └── exactly one session, window, and provider pane
├── short-lived wsnav action and snapshot commands
├── short-lived per-operation provider metadata helpers
└── observation scoped to managed Runtimes
    ├── Codex hooks active only in wsnav-started sessions
    └── D8.1: one host-owned OpenCode sidecar per Runtime generation
```

### Presentation layer

The local presentation session is a dedicated tmux server with its own socket
and configuration. It never modifies or depends on the user's ordinary tmux
server.

The initial presentation sets the navigator to its normal 32-cell width and
gives every remaining terminal column to the provider pane. Tmux can rescale a
new split when it adopts the controlling terminal, so the navigator reapplies
that width after each terminal resize. Individual renderers retain their
compact fallbacks for explicitly narrowed panes.

The navigator is a small Rust TUI in one pane. The provider pane is not a
terminal widget rendered by Rust; it is a real tmux attachment to the host-owned
provider runtime. This retains direct keyboard, mouse, resize, color, and native
TUI behavior without building a PTY server or terminal emulator.

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
While expanded, all other navigator keyboard and mouse actions are inert, so
help cannot accidentally activate or mutate a Workstream.

The navigator pane has one Workstreams home page and two infrequent child
management pages rather than a generic management landing page:

```text
Workstreams
├── Recent / By project / By host / Archived
├── Recovery
├── Projects
│   ├── Project list with inline locations
│   ├── Register location
│   └── Remove archived Project from navigator
└── Hosts
    ├── Host list with inline health / observer state
    └── Register / verify / activate / remove observer / forget
```

Workstreams is the default page and retains the product's ordinary switching
workflow. Projects and Hosts are inventory/configuration pages, reached by
their contextual `,` and `.` keys rather than a persistent tab bar. Pressing
the current page's key again, or `Esc`, returns to the ordinary Workstreams
page; either key otherwise switches directly to its management page. Projects
does not start Workstreams: it only registers and removes Project locations,
so `n` always means a new Workstream from the operational home. Project
locations, Host health, and observer state render inline in their respective
lists, so those pages have no detail drill-down. A thin `Workstreams · <view>`
parent frame above the bordered Projects or Hosts child block retains the
current operational view and makes that hierarchy visible without a dashboard
or a global tab row. Project actions remain explicit page-local keys, as do
Host actions.
No page creates a tmux popup, overlays the provider pane, or replaces the
native TUI.

Direct page-local keys are the canonical control path. The compact footer
shows the most relevant bindings for the current page and state; `?` reveals
the complete list. The management lists provide bounded status and context
inline, but D7 does not require a menu-driven action system. A later clickable action menu may
augment the same operations without replacing or delaying the direct keys.
Each stateful action introduces its own bounded text entry, confirmation, and
progress state with the authority that consumes it; the navigator does not keep
an unconnected generic modal that could imply an action is available before its
host contract exists.

Project registration is an explicit two-step navigator flow: `Projects → a`
first selects the owning host, then opens a centered browser inside the
navigator pane. The browser starts at that host's configured workspace root
(`~/code` by default), lists only bounded direct-child names plus a Git marker,
omits dot-prefixed files and folders by default, and uses a root label and
relative cursor rather than returning absolute paths.
`Enter` opens a directory or registers a marked Git Project; `r` registers the
current browser directory. The Hosts page owns the explicit root-setting action
for the selected host. A local or SSH host resolves the selected relative cursor
and performs Git inspection itself, so raw paths never enter a snapshot, client
catalog, protocol response, or provider pane. This is a navigator-only modal,
not a tmux popup, window, or provider overlay. Its rows stay single-line at the
32-cell navigator width, truncating names before layout; a bounded viewport
follows the selected directory rather than allowing the selector to scroll out
of the visible modal.
Mouse support in D7 covers selection, primary row activation, forms, and
confirmation. Full mouse parity for every management action is not an
acceptance requirement.

The Workstreams page retains its accepted muscle memory: `Enter` performs the
primary open/start/recover action, `n` starts a sibling, `f` forks, `p` parks,
`a` acknowledges, `←/→` changes grouping, and `?` toggles the full
reference.
New D7 actions receive page-local single-key bindings without reinterpreting
those keys. Projects and Hosts may reuse letters because the active page and
its visible footer make the page explicit.

Within the Workstreams page, right cycles the local-only `Recent`, `By project`,
`By host`, and `Archived` views in that order; left moves in the reverse
direction. The first three show active Workstreams; `Archived` is a flat,
recency-ordered restore surface. These remain operational Workstream-browsing
projections, not substitutes for the Project or Host configuration pages. `Recent` is the
default global activity order. Grouped views retain that order by placing the
group containing the newest visible activity first and keeping its Workstreams
newest-first. Headers are non-actionable display rows; selection, mouse
activation, and provider attachment remain exact Workstream operations. Page,
detail, visibility-filter, and grouping choices are client presentation state,
not host action authority.

The navigator assumes horizontal space is scarce and spends vertical space to
keep rows scannable. A `Recent` Workstream uses exactly two display lines:

```text
local · workstream-navigator
✓ lifecycle hook repair                              3m
```

The first line owns host and Project context. The second owns the lifecycle
indicator, Codex thread title, and compact activity age. Age is right-aligned
when space permits; the title truncates before the indicator or age is lost.
Both display lines are one selectable and mouse-actionable Workstream row.
Long context labels truncate within their own line rather than pushing title,
status, or age onto a third line.

Project inventory rows use the same narrow-pane discipline. At the 32-cell
navigator minimum, Project counts use the compact `loc` label so the active
count, archived count, and location count remain visible on one line; wider
panes restore the full `location` or `locations` label.

Grouped views render explicit trees instead of communicating hierarchy through
indentation alone. The group header is the selected axis; each Workstream is a
two-line child whose context line names the other axis and whose continuation
line carries status, title, and age:

```text
By project

workstream-navigator
├─ local
│  ✓ lifecycle repair   3m
└─ snap
   p remote follow-up    1d
```

```text
By host

local
├─ workstream-navigator
│  ✓ lifecycle repair   3m
└─ cubey
   p terrain review      1d
```

Tree branch and continuation glyphs are structural, neutral-colored chrome.
They do not become lifecycle indicators, selection targets, or identity. A
group header remains non-actionable; either line of a child resolves to the
same exact host and Workstream identity.

The navigator uses two deliberately quiet color axes. A readable host-label
accent distinguishes the few active hosts. A deterministic collision-resolved
muted 256-color marker and compact Project label distinguish up to twelve
concurrently visible Projects without coloring the Workstream title or whole
row. Selection changes only the row background. Green, yellow, and red remain
reserved for completed, working, and recovery/error state, so color never
becomes action authority or pulls focus from the native provider pane.

Switching workstreams replaces only the provider pane's attachment helper. It
does not stop, restart, type into, or resize an inactive provider process beyond
the normal detach/attach terminal negotiation.

Each replacement gets one presentation-private attempt ID and a mode-`0600`
pending/running/completed/failed status file. The attachment helper updates
that file, never the provider pane. The navigator clears its non-durable
attachment marker when the helper completes or fails and permits an exact
same-row retry; a helper pane that dies before reporting a terminal phase is
also classified as failed. These files disappear with the disposable
presentation and contain only the host alias, Workstream ID, attempt ID, and
phase.

Focus is local presentation state, not durable Workstream state. Two navigator
clients may look at different workstreams without racing over a global
`current` record. Durable state records activity and attention, never an
authoritative focused pane.

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
- the Workstream, ProjectLocation, Fork recovery, binding, and attention records
  for work physically running on that host.

Every newly created Runtime derives its private tmux directory and session name
from the complete opaque Runtime UUID. The persisted session value must match
that exact current form before WSNav probes, attaches, parks, or removes a
private server. A narrowly defined former short-ID form is read only for a
Runtime record created by an older build; any other value is ambiguous and no
tmux action is attempted.

tmux owns live process persistence. SQLite owns metadata and recoverable
Start/Fork state. The native provider owns session history.

Each live Runtime is a bounded tmux unit:

```text
Runtime -> one private socket and server -> one session -> one window -> one pane
```

No private runtime server contains a sibling Workstream. Parking or stopping a
Runtime removes its server rather than leaving an empty session.
The registry, not tmux's own session list, is the cross-Workstream catalog.
This contains server failure, terminal sizing, attachment, and `tmux ls`
visibility to one Workstream at a time.

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

There is no shared or always-running remote daemon in V1. Control requests
launch short-lived `wsnav _remote` commands through SSH. An interactive
`ssh -tt` attachment to a provider Runtime carries the native terminal and no
management watch stream. D8.1 adds only one host-local observer sidecar scoped
to each live OpenCode Runtime generation; it is neither a shared service nor a
network control plane. A connected navigator refreshes hosts through
cursor-paged bounded snapshots: each response contains at most one fixed-size
page, cursors must advance, replayed Workstream identities are rejected, and
the client enforces a finite page count. Focused or recently active hosts may
be polled more frequently, while background and repeatedly unreachable hosts
back off. Action responses update local state immediately; the next complete
snapshot reconciles it.

All mutation commands use host-local SQLite transactions and optimistic
revisions. Independent Start commits its Workstream and private Runtime
reservation before launching the selected native provider, so reopening that
Workstream automatically continues the normal start/resume path after a client
loss. Fork additionally uses a durable request key and recovery phases because
native provider session creation is non-idempotent. Concurrent observations
and clients may race, but only one transaction can commit a particular record
revision.

Focus, attach, snapshot, and refresh are not durable operations. Rename is a
repeatable provider setting. Park and Resume reconcile through the authoritative
Runtime record plus live tmux/process probes. Only Fork uses the
CompoundOperation journal.

Resume transactionally reserves one new Runtime generation before launching
tmux or the selected native provider. The launcher must match that exact
prepared record, and another Resume is refused while the generation is
`starting` or live. If the response is lost, a snapshot reconciles the prepared
record with the exact private tmux socket and process evidence instead of
starting a second Runtime.

The private pane initially runs a silent one-shot WSNav launch barrier. Its
process birth is recorded against the prepared Runtime before the owning action
releases the barrier. The barrier then `exec`s the selected provider TUI in
place, preserving the same PID and birth token. For Codex, this prevents an
immediate `SessionStart` from racing ahead of its recorded hook authority;
OpenCode additionally must satisfy the D8.1 endpoint and sidecar readiness
barrier before attachment.

### Host transport

Local and SSH hosts implement one internal interface:

```text
hello() -> protocol, host identity, versions, fixed registration capabilities
snapshot() -> locations, workstreams, dynamic provider capabilities, runtime probes, attention
apply(action, expected revisions) -> deterministic outcome
attach(runtime_id) -> native terminal attachment
```

The SSH command is fixed and machine-oriented. Request bodies travel as bounded
JSON on stdin; stdout contains only versioned protocol frames and stderr
contains bounded diagnostics. Thread names, repository paths, prompts, and shell
fragments are never interpolated into an SSH command string.

Read-only probes, handshakes, snapshots, and non-Runtime control calls retain
the short eight-second process deadline. Runtime-creating and recovery mutations
use a separate bounded 45-second deadline so the outer SSH process contains
the adapter's bounded endpoint and observer-readiness barriers instead of
terminating a valid OpenCode start mid-handoff. This does not widen the SSH
connection deadline or make any control process persistent.

The remote binary validates protocol compatibility before reading or mutating
state. An incompatible host is visible but unavailable for actions. V1 requires
the user to install `wsnav` on each host; `wsnav register-remote <host>` uses
the fixed standard remote path `~/.local/bin/wsnav`, while an explicit absolute
override remains available for a nonstandard installation. It diagnoses missing
or incompatible binaries but does not copy, bootstrap, or update remote
executables.

Before a client uses the stateful protocol, it runs a stateless release probe on
the registered executable. The probe contains only the package version, control
ABI, protocol version, and host-schema version; it does not open the host state
or disclose a path, host identity, registry generation, or provider metadata.
A missing, malformed, or incompatible probe leaves the cached host visible but
unavailable for actions and tells the operator to install a matching build. The
Hosts page reports a bounded, credential-free cause: checking, SSH/`wsnav`
unavailable, timeout, malformed probe, a local/remote ABI, protocol, or schema
mismatch, changed identity/stale registration, rejected remote request, or a
generic control communication failure. Version mismatches show both version
numbers; raw SSH diagnostics, paths, and protocol frames never reach the UI.
Normal registration, polling, and mutation repeat that check. This is a manual
deployment boundary: V1 diagnoses an upgrade requirement and provides a
runbook, but never copies, bootstraps, or updates a remote executable.

The handshake returns a stable host ID and registry generation. If either
changes unexpectedly for an existing client registration, the client preserves
its cached view but disables mutation. V1 does not merge catalogs, adopt
unknown runtimes, or reconcile divergent host registries. The user explicitly
resets that client registration and registers the current host state again.
The registry generation changes only when the registry is replaced or
explicitly reset; ordinary record mutations use their own revisions.

Polling introduces bounded display staleness, not state loss: observer hooks
commit status and AttentionState on the host before any client sees them. The
next snapshot exposes the durable state, while manual refresh and mutation
responses provide immediate paths when the user is actively interacting.

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

Opening `wsnav` is the explicit host-local activation intent. Before a fresh
navigator presentation opens normal work, it verifies the observer. It may
create an absent exact owned profile or atomically migrate an exact legacy
declaration only when no WSNav Runtime is live. Its generated file starts with
a human-readable managed marker, but write and removal authority comes from a
private host record containing an owner ID, schema version, canonical profile
path, absolute WSNav hook executable path, and exact generated-declaration
hash. Creation and replacement use a mode-`0600` temporary file plus atomic
rename. An existing unowned path, a missing ownership record, a modified
declaration, or any live Runtime is never overwritten, replaced, or removed
automatically.

The hook definition is reviewed and trusted through Codex's native `/hooks`
UI. WSNav never writes Codex's trust database and never passes
`--dangerously-bypass-hook-trust`. When trust is missing, the fresh navigator
replaces only its blank right pane with a temporary native, profile-selected
Codex review process in an empty disposable cwd. The navigator remains visible;
the operator uses the normal `/hooks` UI, trusts the exact generated command,
and exits without submitting a prompt. That temporary process is not a managed
Runtime or Workstream and deliberately has no observer authority: an invoked
hook drains and does nothing. On exit, WSNav silently verifies the complete
native trust record and returns the right pane to its blank state. If review is
declined, cancelled, or incomplete, it remains `trust_pending`; managed launch
remains fail-closed and opening a new fresh navigator reruns the review. The
activation process neither inspects the current cwd nor creates a
ProjectLocation or Workstream. A blank Codex landing screen emits no
`SessionStart`, so no stronger passive activation signal is fabricated. The
first managed `SessionStart` must instead pass the normal provider-side
corroboration gate. Whether an unprompted review process leaves any native
history residue is a validation gate and must be disclosed if it cannot be
avoided.

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

Profile update or removal requires no live WSNav-managed Codex Runtime. The
next fresh host-local `wsnav` validates an exact legacy declaration, atomically
replaces it, and discards its co-located native trust suffix before entering the
same native review. A declaration-changing update preserves an accepted
provider prefix byte for byte and returns the integration to `trust_pending`
until native review succeeds again; an exact no-op preserves both prefix and
trust. The hidden diagnostic update command follows the same rule.

Removal validates all three regions, removes the WSNav declaration and native
trust suffix, and then removes the ownership record. With no provider prefix it
deletes the profile; with an accepted prefix it atomically leaves only those
provider-owned model settings at the same path. That model-only file is foreign
to a later setup and is never silently adopted. Base configuration, other
profiles, user and project hooks, plugins, history, credentials, and all state
outside the dedicated profile remain untouched.

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
dedicated TUI. A remote host filters App Server responses before writing the
Workstream Navigator protocol to SSH stdout.

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

The host extracts only approved fields from responses. It never returns or
persists `preview`, turns, items, transcript paths, or the raw response.
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
means the read did not complete or the host could not be reached; it does not
erase a cached name.

| Context | Effective display when the current tip has no native name |
| --- | --- |
| New Workstream before thread binding | `starting` |
| New or existing Workstream with a known-empty name | `untitled` |
| Same-Workstream cutover from named A to unnamed B | `<A name> ↻ unnamed` |
| Same-Workstream cutover when A was also unnamed | `untitled ↻` |
| Fork to a new Workstream from a named source | `<source name> · fork` |
| Fork from an unnamed source | `forked workstream` |
| Metadata refresh unavailable with a current-tip cache | Last cached native name with a stale or unreachable indicator |
| Metadata refresh unavailable without a current-tip cache | The contextual transition display with `name unavailable`; otherwise `name unavailable` |
| Provider thread missing during recovery | Last cached native name with `recovery required`; otherwise `recovery required` |

Resolution prefers a current non-empty native name, then a current-binding
cache when refresh is unavailable, then transition context, and finally a
synthetic lifecycle fallback. An unavailable observation never becomes
`unnamed` or `untitled`; those displays require `known_empty`. Fallbacks never
expose a workstream or provider identifier. Git state, host, and cwd remain
secondary context rather than naming authority.

An exact thread ID, not any displayed text, remains identity and action
authority. Names and computed fallbacks need not be unique.

Navigator rows show the readable host alias, project, current tip name, and a
relative age from the last observed native conversation activity. Activity
sequence remains the deterministic ordering key within each host. In a combined
client view, rows with a known wall-clock activity time sort newest first, then
fall back to host, Project, and Workstream identity for a stable order; unknown
activity sorts last. This presentation-only cross-host ordering does not mutate
or authorize any host state. The wall-clock value survives start, resume, and
park. A migrated Workstream or one with no observed turn visibly reports
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

### Client catalog

The local client catalog contains only:

- configured host aliases and stable host IDs;
- client-generated opaque Project IDs, presentation names, optional
  credential-free repository fingerprints, and optional normalized remote
  display labels;
- mappings from those local presentation records to exact host locations; and
- local UI preferences.

The client catalog is not authority for a remote runtime, Git state, provider
binding, or mutation. Losing it does not stop local or remote work. A readable
host alias and one stable Project label identify each row. The Project label is
derived from repository registration metadata, never from a generated path.

Host actions address opaque Location and Workstream IDs, not a replicated
project catalog. A host may expose a bounded opaque fingerprint and a
credential-free normalized `host/path` display label for one unambiguous
canonical fetch remote. The client reuses one Project ID only for locations
with the same fingerprint; the display label never grants authority. Missing
or ambiguous remote evidence keeps locations separate. This grouping is
presentation only and never grants one host authority over another host's
repository or Runtime. Raw remote URLs, schemes, user information, query
strings, fragments, filesystem paths, and repository common directories never
cross the host protocol.

### Host registry

The host registry contains:

```text
HostIdentity
  host_id, registry_generation, schema_version

CodexIntegration
  profile_name, canonical_profile_path, owner_id, profile_schema_version,
  hook_executable_path, generated_content_hash, lifecycle, revision

ProjectLocation
  location_id, repository_path, repository_display_name,
  remote_identity_fingerprint?, remote_identity_display?, revision

Workstream
  workstream_id, location_id, origin,
  source_workstream_id?, lifecycle, archived_at?, revision

Runtime
  runtime_id, workstream_id, provider, tmux_generation,
  tmux_session, cwd, provider_pid, process_birth, lifecycle, revision

ProviderBinding
  binding_id, runtime_id, native_session_id, start_source,
  last_settled_turn_id?, observed_thread_name?, name_state,
  name_observed_at?,
  predecessor_native_session_id?, predecessor_effective_name?,
  runtime_generation, revision

AttentionState
  workstream_id, result_unseen_since_revision?,
  recovery_unseen_since_revision?, latest_native_session_id?,
  latest_turn_id?, revision

CompoundOperation
  operation_id, request_key, kind=start|fork, phase, expected_revisions,
  effect_watermark, outcome
```

Paths and provider identifiers are private host fields. Public snapshots return
bounded Project and thread names, opaque repository fingerprints, optional
credential-free normalized remote labels, name provenance, statuses,
capabilities, and opaque Workstream Navigator IDs. No raw remote URL, prompt,
preview, response, transcript, tool payload, terminal capture, credential, or
environment dump is persisted.

### State relationships

- One Workstream references exactly one ProjectLocation root.
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
starting | idle | working | attention | stopped | unknown | unreachable
```

`unreachable` is a transport observation, not proof that a runtime stopped.

## Git project-root policy

Registration performs bounded, read-only Git discovery without contacting a
network. It normalizes a supplied repository path, linked worktree, or child
directory to the repository's primary non-bare worktree. That canonical path
is the registered `ProjectLocation` root and the launch cwd for every WSNav
Workstream at that location.

Registration may also derive one unambiguous canonical fetch-remote fingerprint
and a credential-free `host/path` display label. `origin` is preferred; one
unambiguous sole fetch remote is the fallback. Local paths, `file:` remotes,
missing remotes, and conflicting remotes produce neither label nor fingerprint
and therefore no automatic cross-host grouping.

After registration, WSNav performs no Git lifecycle operation. It never creates
or removes worktrees or branches; resolves commits; fetches, pulls, commits,
merges, rebases, resets, stashes, pushes, cherry-picks, or copies files. New
and forked Workstreams both launch at the same registered project root. If a
task needs an isolated worktree, the user or Codex creates and manages it inside
the native session using the provider's normal workflow.

Conversation lineage remains explicit:

```text
source Codex session -> forked Codex session
```

It makes no claim about filesystem lineage. Parking and archiving stop or hide
the Runtime while preserving provider history and the registered ProjectLocation;
they never inspect or change project files.

## Core workflows

### Open an existing Workstream

```text
user selects Workstream
-> navigator resolves its authoritative host
-> host confirms runtime generation and tmux session
-> provider pane attachment is replaced
-> native Codex screen redraws from the host runtime
-> no provider input is sent
```

If the runtime is stopped but the native session binding is known, the user
chooses Resume:

```text
host creates a fresh dedicated tmux session at the recorded project root
-> launches native codex -C <project-root> resume with the exact session ID
-> SessionStart(source=resume) confirms the binding
-> navigator attaches the provider pane
```

### Start an independent Workstream

```text
user selects an existing Workstream and presses n
-> navigator retains that Workstream's exact ProjectLocation; no project picker opens
-> host records a new Workstream at the same ProjectLocation root
-> host launches a blank native Codex TUI in dedicated tmux
-> SessionStart confirms the native session
-> navigator focuses the new Workstream
-> user enters the first prompt in Codex's native composer
```

When the Workstreams home is empty, `n` instead opens ProjectLocation
registration. That flow selects the host and checkout, registers the location,
and supplies the initial unstarted Workstream. Project selection and
registration are therefore bootstrap behavior, not part of ordinary `n` from
an existing Workstream.

No workstream name, model, branch, session ID, or first prompt is required in a
manager-owned creation form. Before binding, the row shows
`starting`; a bound but unnamed tip shows `untitled`. Later native `/rename`,
navigator Rename, or an opt-in Codex naming skill updates the one
Codex-owned thread name.

### Fork a running Workstream

The action means “explore another approach from the latest settled conversation
state.” It does not fork partial model output or current filesystem state.

```text
source Codex turn may still be running
-> user explicitly selects Fork Workstream
-> host validates the source binding and last settled provider boundary
-> durable Fork operation records the provider-only cutover plan at the same project root
-> ephemeral App Server forks source through exact lastTurnId and requests destination cwd
-> if source has a native name, host sets a bounded provisional fork name
-> host launches native codex -C <project-root> resume for the returned destination thread ID
-> destination SessionStart confirms the new native session
-> source runtime continues unchanged
-> navigator may focus destination; source completion only raises attention
```

If the installed Codex contract cannot prove a settled-prefix fork for a live
source, the action is unavailable. The user can still start an independent
Workstream.

### Native thread management

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

## Navigator experience

The default view is intentionally small:

```text
Host
└── Project
    ├── Tip thread name         working
    ├── Prior name ↻ unnamed    working
    ├── Source · fork           result ready
    └── untitled                parked

┌ navigator ┐┌──────────────── native Codex TUI ────────────────┐
│ tree      ││ directly interactive; no manager-owned chrome   │
│ status    ││ inside the provider surface                     │
└───────────┘└──────────────────────────────────────────────────┘
```

Required interactions:

- keyboard and mouse selection in the navigator;
- direct keyboard and mouse interaction in the provider pane;
- one action to focus or reconnect a Workstream;
- register the first local ProjectLocation from the empty navigator without a
  shell command or cwd inference;
- Start Workstream from a selected Workstream at its registered root, with
  ProjectLocation registration used only to bootstrap an empty navigator;
- Fork Workstream from an exact managed source;
- inspect bounded Workstream status and rename the current tip through Codex's
  canonical thread-name field;
- park/resume without deleting provider history;
- archive a Workstream out of the active list and restore it without starting
  Codex or deleting its retained state;
- route a repeated Fork to its exact unresolved operation and reconcile it;
- inspect and register ProjectLocations on local or SSH hosts;
- add or remove SSH host registrations without leaving the navigator;
- acknowledge result or recovery attention without injecting provider traffic.

The normal human workflow begins with bare `wsnav` and requires no later
`wsnav` shell command. Public CLI equivalents remain supported for scripting,
diagnosis, direct attachment, and break-glass recovery, but the documentation
and empty states never send the user to them for an ordinary WSNav operation.
Installing or upgrading local/remote executables, configuring SSH trust,
cloning repositories, native Codex input and hook approval, and deferred Git
cleanup remain external prerequisites or explicitly excluded operations.

The Workstreams page has three active views plus the separate `Archived` view.
Archive is the ordinary answer to accumulated test or inactive Workstreams;
there is no hard-delete action. Projects disappear from the active operational
views when they have no active Workstreams, but remain available through
Projects and `Archived`. Archiving a working Runtime requires explicit
confirmation because parking it interrupts the current provider turn.

An unfinished Fork belongs to its already-visible source Workstream rather than
a normal global management page. Pressing `f` on that source opens a focused
choice to reconcile the exact saved operation or deliberately start another
Fork. If several unfinished Forks share the source, a bounded chooser lists
only those candidates. The source Workstream ID is transient routing metadata,
never rendered; request keys, paths, provider identifiers, and raw evidence
remain hidden. A recovered destination opens directly in the native provider
pane.

The Projects page reflects the ownership boundary explicitly: a logical
client-side Project contains one or more host-owned ProjectLocations. Adding a
remote location sends one bounded structured project path to that host for
local Git inspection; paths are never interpolated into SSH shell syntax or
returned in public snapshots. Permanent Project deletion, manual repository
cleanup, and automatic cross-host merge/split remain outside D7.

Projects render every host-owned location as a bounded tree under its logical
Project, with active/archived counts. When available, it renders the compact
display-only `↗ org/repo` label beneath the Project name, eliding the normalized
remote host for narrow terminals; the fingerprint remains hidden and
grouping-only. Projects remains management-only: `a` adds an existing project
path through the navigator-local host picker and bounded directory browser.
When the Workstreams home is empty, `n` enters that same registration flow and
supplies the initial Workstream at the chosen location. Once a Workstream
exists, `n` starts another at that selected Workstream's exact ProjectLocation
without reopening host, Project, or location selection. The selected host alone
receives a registration path for local Git inspection; no path is written into
provider panes, returned by the SSH control response, or shown in Workstream
snapshots. Matching credential-free repository fingerprints associate locations
into one logical Project.

`x` removes an archived Project from this navigator only after confirmation. It
records the selected locations as hidden client inventory and refuses while any
of the Project's Workstreams are active. It never deletes or mutates the host
registry, Git state, project files, or Codex history; this is intentional
visibility cleanup, not permanent Project deletion.

The Hosts page renders each host's reachability and observer state, followed by
a bounded tree of its active Projects and their active Workstream counts. A
ready observer is a compact green check; review, modification, and disabled
states remain explicit text. An unavailable host shows its bounded transport
diagnosis rather than a generic unavailable label. `a`
adds a remote SSH host: WSNav verifies and registers it, prepares its exact
observer profile, then opens the native Codex hook review in the right pane.
The user alone approves that profile in Codex; preparation never writes trust
state. `x` first asks whether to disconnect or offboard a selected remote Host.
Disconnect is client-catalog-only: it removes the alias and local Project
associations but does not contact the host, stop a Runtime, delete remote state,
or uninstall anything. Offboard first removes only WSNav's exact declaration
and native trust from the dedicated observer profile, preserving an accepted
provider-owned model prefix when present, then performs that same
client-catalog removal. It refuses while a managed Runtime is live or when the
remote host cannot be reached. The choice shows retained Workstream,
ProjectLocation, and unresolved-operation counts so the visibility effect is
explicit. The protected local Host cannot be removed.
If an observer review is required, its native profile-selected Codex TUI runs
in the right provider pane through the same local or SSH terminal boundary and
leaves no Workstream behind. Remote review uses a one-shot remote endpoint that
returns after the native Codex TUI exits; the local provider pane then resumes
its own blank wait state, so no SSH review connection becomes a second remote
persistence layer.

Navigator page changes, forms, and finite management actions leave the current
provider attachment and focus unchanged. Only an explicit Workstream primary
action or observer review replaces the right pane. Potentially slow Git, SSH,
provider-metadata, and observer actions expose bounded progress in the
navigator, suppress duplicate submission, and commit only an exact current
revision; they never freeze silently or print management output into Codex.

The navigator does not ask for model IDs, session IDs, branch names, request
IDs, or a mandatory title in the ordinary path.

A direct mode, such as `wsnav attach <workstream>`, bypasses the navigator pane
while using the same host/runtime contracts.

## Failure and recovery model

| Failure | V1 behavior |
| --- | --- |
| Local presentation exits | Remote or local host runtime continues; reopen the navigator and attach |
| SSH connection drops | Provider remains behind host tmux; show `unreachable`, then reconnect |
| Remote host is offline | Preserve last known metadata; never claim the runtime stopped |
| Exact private runtime tmux server is gone | Mark that Runtime `recovery_required`; exact native resume may create a new runtime generation |
| Codex process exits normally | Keep Workstream and provider binding; offer exact native resume |
| Observer hook is absent or missed | Show `unknown`; retain live attach; block exact fork/recovery if session identity is unknown |
| Hook identity cannot be corroborated | Do not rotate the ProviderBinding; show `unknown` or `recovery required` |
| Hook events race | Resolve by runtime generation, session ID, turn ID, and transactional state; conflicting evidence becomes `unknown` |
| Exact name read returns empty | Record `known_empty` and compute the context-specific fallback |
| Name refresh is unavailable | Keep the dedicated TUI untouched and retain the cached native name with stale or unreachable provenance |
| Ephemeral App Server mutation is ambiguous | Reconcile exact persisted effects; never retry a non-idempotent fork unless absence is proven, otherwise require recovery |
| Another client or direct tmux client attaches | Show the same tmux-managed screen; do not create a lease or detach either client; simultaneous input may interleave |
| Navigator crashes during focus switch | Focus is ephemeral; no durable runtime or Workstream mutation is implied |
| Client disconnects during Start or Fork | Start is already committed locally; reopen the exact Fork operation only when provider cutover is unresolved |
| Project Git state changes | Leave it entirely to Codex or the user; WSNav neither validates nor changes it |
| Host protocol versions differ | Reject mutation and show an actionable compatibility diagnostic |
| Registered host ID or registry generation changes unexpectedly | Preserve the cached view, reject mutation, and require explicit client-side reset and registration |
| `wsnav-observer` is absent, foreign, modified, disabled, or awaiting trust | Preserve existing Runtime attachment; block new observer-dependent launch and report the exact setup or native `/hooks` action |
| Profile update or removal is requested while a managed Runtime is live | Refuse the integration change until all WSNav-managed Codex Runtimes on that host are parked or stopped |

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
- Finite local control commands (tmux probes/actions, Git, SSH control, and
  child WSNav actions) drain stdout and stderr concurrently while retaining
  only their explicit per-stream bounds. They also have wall-clock deadlines
  and terminate their complete process group on timeout. Direct provider
  attachment is a terminal stream, not captured child output.
- Private tmux sockets are a namespace and accidental-discovery boundary, not
  a same-user security boundary. Workstream Navigator does not prevent a user
  who knows the socket path from attaching or stopping the Runtime.
- SSH relies on the user's existing host authentication and `known_hosts`;
  Workstream Navigator opens no listener.
- Managed Codex TUIs never use `codex --remote`, and Workstream Navigator never
  starts a persistent Codex App Server transport.
- Managed Codex TUIs use the normal user `CODEX_HOME` plus the exactly owned
  `wsnav-observer` profile. The generated profile is mode `0600`, adds only
  passive lifecycle hooks, and is selected only for WSNav launches.
- Hook trust is a native Codex user decision. WSNav neither edits the trust
  store nor bypasses trust review.
- Ephemeral App Server helpers use private stdio, a distinct process group,
  bounded request and shutdown deadlines, and forced cleanup when graceful
  shutdown fails.
- Remote commands disable forwarding and use bounded fixed protocol entrypoints.
  Snapshot workstream metadata includes a bounded project display label derived
  only from the registered ProjectLocation repository basename; it never
  includes an absolute or relative repository or checkout path.
- Provider and Git commands are built as argument vectors. Thread names and
  paths never become shell fragments.
- Hook stdin is fully drained even for unmanaged, stale, oversized, or malformed
  events.
- Hook payloads, prompts, transcripts, terminal screens, credentials, process
  environments, and raw remote errors are not logged or committed.
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
  every later Git decision stays inside the native Codex session or ordinary
  user tooling.

## Proposed Rust structure

```text
src/
├── main.rs               CLI entrypoint
├── app.rs                top-level command orchestration
├── domain/               pure IDs, entities, statuses, invariants
├── state/                SQLite schema, revisions, and provider-Fork recovery
├── protocol/             versioned host request/response frames
├── host/
│   ├── local.rs          direct host adapter
│   └── ssh.rs            SSH transport adapter
├── runtime/
│   ├── tmux.rs           dedicated server/session ownership and probes
│   └── attach.rs         safe local/remote native attachment helpers
├── provider/
│   ├── mod.rs            capability-driven provider interface
│   └── codex/
│       ├── runtime.rs    dedicated native launch and resume
│       ├── profile.rs    observer profile ownership and native trust setup
│       ├── app_server.rs ephemeral stdio metadata/name/fork client
│       └── hooks.rs      passive lifecycle event handling
├── repository.rs         read-only project-root and remote identity discovery
├── tui/                  minimal navigator state, rendering, input, mouse
└── internal/             hidden remote, hook, and snapshot entrypoints
```

The provider interface should remain small and capability-based. V1 has one
real implementation. No speculative Claude abstractions or generic
lowest-common-denominator behavior should shape the Codex implementation.

## Validation and acceptance evidence

The original spikes validate transport, native presentation, the shell-only
per-Runtime tmux topology, and the automated local two-pane Codex presentation
path. Terminal presentation is a settled design prerequisite: Spike 0005
proves the selected retained-TMUX configuration, direct native attachment,
keyboard submission, image attachment request, resize/focus, reconnect, and
result-tip preservation. The frozen Python Phase 7F trial independently
observed direct native-pane interaction, terminal color, and click-to-select
mouse support in an equivalent private-tmux layout. That implementation is
behavioral evidence only; it is not a Rust dependency or compatibility
constraint.

Spikes 0006-0008 settled the remaining provider-facing prerequisites:

- the selected observer profile layers over a disabled base, uses native trust,
  leaves ordinary launches unobserved, drains large unmanaged input, and rejects
  missing, stale, or forged authority;
- one-shot stdio helpers can read and rename an exact managed thread without
  disturbing its native TUI, while shared App Server transports and
  `codex --remote` are excluded;
- native and App Server rename converge on `thread.name`, missing/unavailable
  fallback resolution is complete, and the installed rename contract has no
  compare-and-set field; and
- a running native source can be forked exactly through its last settled turn,
  recovered after an unread response without retry, and resumed in an
  same registered project root while both native Workstreams diverge.

The implemented checkpoints and their acceptance records corroborate the
following behavior without widening the product:

1. **Integration lifecycle:** another selected named profile is rejected
   clearly, disabled-hook policy is visible, malformed/racing/unavailable hook
   input remains fail-open to Codex, and exact update/removal preserves
   unrelated state.
2. **Status transactions and native transitions:** accepted startup/resume
   hooks and the separately proven native `/clear` transition update binding,
   settled-turn, and sticky attention atomically. Native `/new` is unsupported
   in a managed Runtime because it lacks an exact changed-binding claim;
   `/fork` and compact remain Codex-owned workflow. Missed events and races
   fail closed.
3. **Cold recovery:** loss of an exact private runtime followed by
   `codex -C <project-root> resume <session-id>` restores the same native history
   and creates one new runtime generation.
4. **Project-root preservation:** independent and forked Workstreams launch at
   the registered project root; WSNav performs no Git mutation or validation.
5. **Multi-host protocol:** local and SSH adapters return the same semantic
   results through bounded polling, reject version or host-generation mismatch,
   survive disconnect, tolerate multiple tmux attachments, and never mutate an
   ordinary tmux server.
6. **Combined acceptance:** start local work, start remote work while it runs,
   switch between both, fork one, observe background completion without focus
   theft, reconnect, resume after runtime loss, and preserve every provider
   result tip.

Passing fixtures contain only provider/version fingerprints, assertion
booleans, event relationships, timings, and cleanup proof. Assisted diagnostics
cannot become passing fixtures.

Finite host snapshots are cursor-paged in deterministic per-host activity
order. Each frame and the aggregate client refresh have separate limits, so a
large retained Workstream history cannot turn the first page or the whole
navigator into one oversized response.

## V1 delivery checkpoints

The checkpoint sequence and current implementation status are maintained in the
[V1 roadmap](roadmap.md). The summaries below define the architectural boundary
of each checkpoint.

### D0 — Contract kernel

- Domain types, IDs, statuses, invariants, and errors.
- Fresh SQLite state with migrations only from V1 development schemas.
- Start/Fork phase recovery, request deduplication, and failure-injection tests.
- Versioned host protocol types.

### D1 — Local Codex runtime

- One private tmux server, session, window, and pane per live local Runtime.
- Ephemeral App Server client for exact thread-name reads and writes.
- Explicit `wsnav-observer` setup, ownership, native trust review, doctor, and
  exact removal contracts.
- Scoped observer hook plus normal user and trusted-project configuration
  preservation.
- Local project location, external initial project registration, start, attach, status,
  tip naming, attention, park, and exact resume.
- No TUI requirement yet; CLI acceptance first.

### D2 — Minimal navigator

- Dedicated local presentation tmux session.
- Ratatui navigator pane plus directly interactive provider pane.
- Keyboard/mouse selection, focus, switching, and attention.
- Product-level terminal regression tests against the already selected
  retained-TMUX substrate.

### D3 — SSH hosts

- Host registration, handshake, snapshot polling, apply, and attach.
- Remote start, attach, reconnect, status, and cold resume.
- Strict protocol and capability diagnostics.

### D4 — Workstreams and forks

- Independent Workstream action at the registered project root.
- Exact-turn App Server conversation fork at that same project root, followed
  by native TUI resume. Git branches and worktrees remain a native user/Codex
  concern rather than a WSNav operation.

### D5 — Recovery and V1 acceptance

- Crash/failure reconciliation for Start and Fork.
- Install, doctor, uninstall, and residue checks.
- Combined local/remote workflow acceptance.
- UX polish after behavior is complete.

Each checkpoint should be reviewable, committed, and accepted separately. No
checkpoint should install hooks, adopt existing sessions, or mutate ordinary
tmux/provider state during automated tests.

### D5.1 — Operational closure

- Automatic normal-open recovery for independently started Workstreams and
  source-scoped recovery for unresolved Fork operations after a client or
  transport loss.
- Stateless remote release/schema compatibility probe and manual-upgrade
  diagnostics.
- Streaming bounds for every local child-process output path.
- Explicit empty-state registration guidance, exact private-runtime path
  identity, and declared/tested MSRV.

D5.1 is hardening within the approved V1 product boundary. It adds no daemon,
automatic deployment, session adoption, provider mutation beyond the existing
Fork action, or replacement provider UI.

### D5.2 — Correctness closure

- Locked-dependency-compatible MSRV and pinned CI.
- Stable ProjectLocation labels for external and managed remote Workstreams.
- Conclusive-only tmux-loss classification and time-bounded finite child
  process groups.
- Navigator-owned attachment outcome evidence and retry without provider-pane
  diagnostics.
- Scroll-aware mouse targeting and bounded cursor-paged snapshots.

### D6 — Source-installed operator-beta closure

- Present-tense product, architecture, and acceptance documentation.
- Exact-candidate local and SSH operator smoke against matching builds.
- Sanitized final isolation, cleanup, privacy, and release evidence.
- Explicit source-installed `0.1.0` posture without an implied public release
  channel.

## Settled V1 design decisions

The reconciled design settles these potentially expansive questions:

- project grouping is client-local and uses explicit opaque Location mappings,
  not repository heuristics or a replicated host-side Project ID;
- remote hosts require a preinstalled compatible binary at a registered path;
  V1 has no deployment system;
- multiple same-user tmux attachments are allowed without an input lease;
  simultaneous typing is a user-coordination concern;
- host or registry-generation disagreement fails closed and requires explicit
  reset and re-registration rather than adoption, merge, or reconciliation;
- status propagation uses bounded adaptive snapshot polling rather than a
  long-lived watch transport;
- durable compound-operation recovery is limited to Fork; independent
  Workstream creation is transactional before native launch;
- V1 parks Workstreams but never operates on Git worktrees or branches;
- managed Codex launches select the exactly owned `wsnav-observer` profile over
  the normal user configuration while ordinary launches remain untouched;
  composing another selected profile is deferred;
- Workstream display names come from the current Codex tip thread rather than a
  shadow Workstream label, with context-specific computed fallbacks ending in
  the stable Workstream short ID; and
- live TUIs use dedicated process-owned runtimes while App Server access is
  short-lived stdio only; each Runtime has its own bounded private tmux server.

No product-boundary decision remains open for V1. Future implementation or
provider evidence that contradicts this contract must narrow or reopen the
affected workflow; it does not authorize silently weakening isolation, trust,
result-tip preservation, or the no-transcript boundary.

## Multi-provider and multi-agent design

This section is a forward contract for generalizing the single-Codex V1 into a
multi-provider, multi-agent navigator. It is not implemented as a whole; the
roadmap authorizes only its explicitly active delivery checkpoint. It is
motivated by the [opencode feasibility spikes](evidence/spikes/0015-opencode-provider-feasibility.md),
which establishes settled-prefix Fork exactness, absent Fork lineage, and
probe-local database concurrency. The [native runtime contract](evidence/spikes/0016-opencode-runtime-contract.md)
adds exact native TUI resume, probe-local observer wiring, and the per-Runtime
server boundary. [Spike 0017](evidence/spikes/0017-opencode-fresh-session.md)
now proves the selected blank-session precreation path, exact endpoint
ownership, and per-Runtime observer sidecar lifecycle on OpenCode `1.18.11`.

### Framing

The current code has no provider abstraction: `src/provider/` is a namespace,
and Codex adapter types are imported directly across the state, action, app,
navigator, and remote layers. Generalizing to a second provider therefore
requires inventing a real boundary, not extending one. The tmux runtime and
SSH transport layers are already provider-agnostic and stay that way.

The design treats this as a **provider-kind generalization**, not "add
opencode". Codex and opencode become two instances of the same provider
contract. A second agent kind is then a third instance later, not another
one-off integration.

### Provider is a first-class, typed, persisted concept

- `ProviderKind` is an enum (`codex | opencode`) carried on:
  - the **Workstream** (created as one kind and fixed for its lifetime;
    a Workstream never switches provider);
  - its **Runtime** (the live provider process in its private tmux server);
  - each **ProviderBinding** (which provider produced this native session);
  - and every typed provider-session identifier carried through state or wire
    records.
- Hosts expose capability sets. A host may run Codex and opencode work
  concurrently, but each Workstream lane is single-provider.
- Provider identity is never inferred from display text. `native_session_id`
  is namespaced by `ProviderKind` (a `(ProviderKind, session_id)` pair), so
  opaque identifiers stay unambiguous across providers.

Pre-D8 Rust state migrates transactionally to `codex`: host schema 9 migrates
to 10 by adding explicit provider kind to Workstream and ProviderBinding,
validating the existing Runtime `provider` value, and rejecting any non-Codex
or cross-record mismatch. Fresh-schema writes have no implicit provider
default. Client schema 4 migrates to 5 by removing the old `codex`
executable-presence bit from fixed host registration without losing host
aliases or Project associations. The provider-bearing wire contract bumps
protocol 16 to 17. No migration fabricates a model, role, agent, or provider
session ID.

### Provider capabilities and availability

Provider availability is dynamic host state, not client-registration identity.
Each bounded host snapshot carries exactly one sorted, duplicate-free record
for each known `ProviderKind`:

```text
ProviderCapability {
  kind,
  status: available | unavailable | unknown,
  reason: none | adapter_unavailable | not_installed | unsupported_version |
          observer_not_ready | runtime_prerequisite_missing | probe_failed,
  fresh_launch,
  exact_resume,
  observe,
  metadata_read,
  rename,
  fork,
}
```

`unsupported_version` remains a reserved bounded capability reason; the
OpenCode adapter does not use it as a release-number gate.

The fixed client host record continues to verify host ID, registry generation,
release, protocol, and schema compatibility, but does not persist or compare
`ProviderCapability`. Installing, removing, or upgrading a provider therefore
does not stale a host registration. The process-local local/remote monitor
caches provider records only with the snapshot that supplied them; an
unreachable host remains non-actionable even if its last snapshot said a
provider was available. Snapshot pagination repeats the same provider set on
every page and rejects inconsistent pages.

A provider is eligible for New only when `status=available` and
`fresh_launch`, `exact_resume`, and `observe` are all true. Exact resume is a
creation prerequisite because every retained Workstream must survive a lost
Runtime. Metadata read, navigator Rename, and Fork are independent optional
capabilities and never make an otherwise recoverable provider eligible or
ineligible for New. Unknown status, a missing record, a duplicate record, or
an incomplete required surface fails closed.

An available record has `reason=none`; unavailable and unknown records carry
one bounded reason and expose no true operation flag that the host cannot
currently honor. Capability records are advisory UI evidence only: every host
action still validates the Workstream's fixed provider and the exact operation
surface it needs.

Discovery is read-only, bounded, and credential-free. It resolves a fixed
adapter-owned executable name, requires a successful bounded `--version`
probe, and verifies existing observer/runtime prerequisites; it never installs
software, reads provider credentials, tests account access, or selects a model.
The version output is discarded from public capability state and never gates
eligibility. OpenCode actions instead validate the exact HTTP/SSE response
shapes, session/root identity, endpoint/process ownership, and operation
surface they consume. Malformed or missing contract evidence fails closed
without fallback or adoption.

The actual version reported by the owned `/global/health` endpoint is retained
only as bounded host-private Runtime-generation evidence. Every later health
check for that generation must report the same opaque value; an endpoint or
version change makes the Runtime ambiguous. A recovery generation may record a
new value when the upgraded provider still satisfies the adapter contract.
The passing real acceptance used OpenCode `1.18.11`, but that observation does
not create a production release allowlist. A bounded reason may be shown in
diagnostics, while raw process output and executable paths never enter
snapshots.

### New Workstream provider choice

`n` creates an independent, empty native conversation; it never transfers a
conversation between providers.

- From an existing Workstream, its exact host and ProjectLocation are already
  the target. `n` never reopens Project or location selection.
- From an empty Workstreams home, the existing host and ProjectLocation
  registration flow runs first. Provider choice then applies to the initial
  Workstream at that location.
- The target host reports which providers are eligible for New by the exact
  capability predicate above. With no eligible provider, creation stops before
  a Workstream is recorded. With one, WSNav selects it without another prompt.
  With more than one, a small navigator-local chooser asks only for provider
  kind.
- When the chooser was opened from an existing Workstream and that
  Workstream's provider remains eligible, it is the initial selection. This is
  contextual UI state, not a remembered per-Project default. If it is not
  eligible, the user chooses from the remaining host-authoritative set.
- The client sends the selected `ProviderKind` in the creation request. The
  authoritative host repeats discovery immediately before its creation
  transaction and rejects stale or ineligible selection without recording a
  Workstream. It never substitutes another provider. If eligibility was
  revalidated and a later process launch fails, the already-recorded
  fixed-provider Workstream follows the ordinary visible recovery path; it is
  never deleted, silently retried, or moved to another provider.
- Availability detection is read-only and non-mutating. D8 does not install
  providers, configure credentials, approve trust, or add generic onboarding.
  Provider authentication and model choice remain native provider behavior.
- A fresh launch supplies no WSNav-owned model, effort, role, agent, prompt, or
  preset. The provider TUI and its native configuration choose those values,
  and WSNav neither persists nor claims their current state.

The provider selection is fixed on the created Workstream. Resume and Fork use
that recorded provider without prompting. A different-provider Workstream at
the same ProjectLocation is always another New action, never a Fork, migration,
or handoff.

Request deduplication includes provider kind. Reusing one request key with a
different provider is an operation mismatch even when source Workstream and
revision are unchanged.

Direct CLI creation remains deterministic and the internal host wire always
carries an explicit provider. `new-workstream` and remote `host new` accept an
optional `--provider`: omission means the source Workstream's provider when it
is still eligible, otherwise the command fails and asks for an explicit kind.
Empty-state/direct Project registration selects the sole eligible provider or
requires `--provider` when several are eligible. CLI commands never select the
first provider by catalog order.

### Provider-scoped readiness

Opening the navigator must not mutate or block on an unrelated provider.
D8.0 retains the existing Codex-only activation behavior while Codex is the
only production adapter. Before OpenCode can become eligible in D8.1, startup
becomes provider-scoped:

- if no provider is ready and Codex is detected, the existing exact Codex
  observer review remains the bootstrap path;
- if another provider is already eligible, an unready Codex adapter cannot
  block the navigator or hide that provider;
- Codex remains `unavailable/observer_not_ready` until the Codex-specific
  Hosts review action completes; and
- OpenCode adds no installation, credential, trust, or provider-management
  flow.

This is a relocation of the existing Codex readiness action, not a generic
onboarding system. No Workstream is created merely to perform provider setup.

### Provider boundary with dispatch at the action boundary

The production provider boundary covers five provider-specific surfaces, with
capabilities describing which optional surfaces each adapter implements:

1. **Launch program**: how to start a fresh or resumed native TUI
   (Codex: `codex --profile wsnav-observer -C <root> [resume <id>]`;
   OpenCode exact resume: `opencode <root> --hostname 127.0.0.1
   --port <runtime-port> --session <id>` in the private tmux pane). Production
   OpenCode never adds `--pure`, `--model`, `--agent`, or `--prompt`; the user
   retains normal plugins, configuration, model choice, and native first
   input. OpenCode fresh launch uses the evidence-selected, contract-validated
   precreation path below. The runtime
   launch barrier and process-birth authority remain generic.
2. **Lifecycle observer**: how passive lifecycle evidence is obtained
   (Codex: stdin JSON hook payload; OpenCode: one read-only SSE event stream
   plus status polling per Runtime). The OpenCode helper binds events to the
   observed session ID, keeps only bounded lifecycle metadata, discards event
   content, and ignores child or unrelated sessions. Both providers adapt into
   the same internal lifecycle events (`start`, `resume`, `working`, `settled`,
   `stopped`).
3. **Metadata operations**: read current tip name, rename when supported, list
   recovery candidates when supported, and fork by exact settled boundary
   when supported (Codex: ephemeral App Server
   `thread/read|name/set|list|fork`; OpenCode evidence currently covers exact
   session read and `POST /session/:id/fork`, not navigator Rename). OpenCode
   Fork uses the exact settled `messageID` boundary and does not provide
   structural lineage for recovery.
4. **Fork reconciliation**: resolve a non-idempotent fork after a lost
   response using provider structural lineage when available, else the
   accepted degradation below.
5. **Observer/trust setup**: how WSNav installs and verifies passive
   observation (Codex: exactly owned `wsnav-observer` profile plus native
   `/hooks` trust review; opencode: no profile or trust review is required,
   since observation is read-only SSE).

The provider contract reports prerequisites without requiring a generic
provider-onboarding surface. Construction is dispatched once at the host
action boundary from the Workstream's `ProviderKind`; provider kind is never
accepted from display text or an untrusted hook/event.

D8.0 deliberately establishes only the provider-neutral data kernel and Codex
parity: typed provider identity, lifecycle/name DTOs used by shared state and
presentation, dynamic capability records, schema/wire changes, and one Codex
dispatch branch. It does not invent OpenCode behavior or require a speculative
five-surface implementation. With the fresh-session and observer evidence gate
passing on the contract observed with `1.18.11`, D8.1 may add the second
adapter and make shared action/app/state/navigator/remote call sites depend on
the provider boundary
rather than concrete Codex adapter types. Provider-specific profile, HTTP,
SSE, and App Server code remains inside its adapter.

### OpenCode fresh-session evidence gate

Spike 0016 proves exact native resume of sessions that were created by earlier
prompted `opencode run` commands. [Spike 0017](evidence/spikes/0017-opencode-fresh-session.md)
proves the New contract through the selected provider-native candidate:
pre-create a blank session through a short-lived server, stop that server, then
launch the native TUI with the returned exact session ID. The probe also proves
two same-root TUIs, production launch without `--pure`, exact endpoint
ownership, and one replaceable observer sidecar per Runtime generation on
OpenCode `1.18.11`.

The selected path uses the production command shape without `--pure`,
`--model`, `--agent`, or `--prompt`, runs two blank native TUIs at the same
project root, keeps their first native prompts and events non-crossing,
establishes exact session IDs without using transcript content, title/recency
inference, or session-list ordering, resumes an exact session after Runtime
restart, and cleans up all provider/tmux/observer state. The probe's disposable
postcondition checks are discarded and never enter WSNav state. A candidate
that relies on title text, database recency, or a WSNav-supplied first prompt
is rejected.

OpenCode-native creation or switching to another session inside an already
managed TUI is unsupported unless the same evidence proves an exact active-TUI
changed-binding claim. Ordinary global `session.created` events are not enough.
Because Spike 0017 does not prove an exact active-TUI changed-binding claim,
WSNav retains the prior binding and instructs the user to use `n` for another
Workstream, matching the fail-closed Codex native `/new` boundary.

### OpenCode Runtime handle and observer sidecar

An OpenCode Runtime keeps bounded host-private provider state scoped to one
exact Runtime generation:

```text
OpenCodeRuntimeHandle {
  loopback_endpoint,
  observed_provider_version,
  observer_pid?,
  observer_process_birth?,
  observer_status: starting | ready | unknown | stopped,
}
```

This handle is stored only in the authoritative host registry. It never enters
the client catalog or public Workstream snapshots. A new Runtime generation
gets a new handle; a stopped generation's endpoint or helper can never be
reused or adopted.

The host starts the native TUI in the Runtime's existing sole tmux
server/session/window/pane, records the exact pane process birth, and validates
that the loopback listener belongs to that exact process or a proven descendant
before using the endpoint. `/global/health`, version, cwd, session identity,
and process ancestry are corroborating checks; health plus `GET /session/:id`
alone is insufficient because another OpenCode server may share the same
provider database. Port selection is bounded, collision failure is explicit,
and WSNav never searches for or adopts another listening endpoint.

One separate host-owned `wsnav` observer sidecar runs per OpenCode Runtime. It
is not a provider process, shared daemon, tmux pane/window, or client process.
It starts with stdin/stdout/stderr disconnected from the provider pane, carries
the exact Runtime ID/generation/endpoint, records its PID plus process birth,
and reaches `ready` only after the exact SSE stream is established, before the
provider pane can be attached for native input.
On the authoritative host—including an SSH host—it reads the one SSE stream,
applies the strict session/root metadata allowlist, discards content and raw
payloads before state adaptation, and writes only provider-neutral lifecycle
metadata through revision-guarded host transactions.

A completed assistant `message.updated` is retained as a candidate until exact
idle corroboration. A trailing busy status does not erase that completed ID;
only a new incomplete message update can invalidate the transient candidate.
This preserves the last exact settled boundary without deriving an ID from
polling or provider content.

Spike 0017 validates this ownership model on the acceptance-tested release: the
sidecar is independently replaceable, reconnects to the same endpoint and
generation, survives a detached/reopened tmux attachment, and is removed with
the disposable Runtime.

The sidecar reconnects only to the same corroborated endpoint with bounded
backoff. A missing helper, changed process birth, inconsistent endpoint, parse
failure, or exhausted reconnect budget makes observation `unknown`, blocks
Fork and other exact-boundary mutations, and never stops or rebinds the native
TUI. Park validates and stops the exact sidecar before killing the private tmux
server; recovery replaces both endpoint and helper under a new generation.
Tests inject helper crash, stale PID, endpoint reuse, port collision, action
failure between provider and helper start, detach/reopen, and complete cleanup.

OpenCode lost-Runtime recovery is narrower than Codex recovery. It is available
only when WSNav already holds an exact OpenCode root-session binding and the
recorded private tmux Runtime is conclusively missing. Recovery validates and
stops only the recorded observer by PID plus process birth, removes only the
matching prior-generation handle and private Runtime artifacts, reserves a new
generation, allocates a new loopback endpoint and observer, and resumes that
same bound session. A missing binding, live or ambiguous private Runtime,
uncorroborated observer, or mismatched handle fails closed. WSNav never opens a
native OpenCode picker, discovers another session, adopts an endpoint, or
creates a blank replacement conversation during recovery.

### Multi-agent model

"Multi-agent" has two distinct meanings, and only one is WSNav-owned:

- **WSNav lanes are independent single-provider Workstreams.** More agents =
  more Workstreams, each one provider process in its own private tmux server.
  This is unchanged from V1 and is validated by the concurrency spike.
- **Provider-owned subagents stay provider-owned.** opencode subagents are
  child sessions inside one opencode runtime (`session.parent_id`), not
  WSNav Workstreams, not separate processes. WSNav treats them as provider
  internal state: its observer discards child-session events and never creates,
  names, rebinds, or manages a subagent as a Workstream.

There is no cross-provider migration: a Codex Workstream never becomes an
opencode Workstream, and a live conversation is never transferred between
providers. Parallel Workstreams may share the same ProjectLocation and use
different providers, but they begin as independent empty conversations. Files
and user-authored notes in the shared project are the explicit context bridge;
WSNav does not copy prompts, transcripts, summaries, or provider state between
them.

### Fork-recovery known limitation

The [opencode fork-lineage spikes](evidence/spikes/0015-opencode-provider-feasibility.md)
and [native runtime contract](evidence/spikes/0016-opencode-runtime-contract.md)
prove that opencode creates a fork destination with no structural lineage
(`parent_id` is null, the source children API is empty; the only marker is the
title suffix `(fork #N)`). Codex exposes structural lineage and keeps the V1
reconciliation contract. For a provider without structural lineage, the
accepted degradation is:

- the happy path is unchanged (the fork response carries the destination ID
  and WSNav records it immediately);
- a lost fork response is terminal `Failed` with
  `external_effect_unknown`; the source returns to its pre-Fork visible state,
  no destination Workstream is created, and the user sees an error explaining
  that an unmanaged provider session may need inspection or cleanup in the
  provider's native UI;
- WSNav never re-forks, guesses from title text, automatically adopts a
  destination, or shows a recovery picker; the same request key cannot replay
  the provider Fork, while a new explicit Fork is a new request.

A future provider release that populates fork lineage removes the limitation
without a design change.

### Navigator presentation

- A quiet provider-kind marker and label on the Workstream row's context line
  (styled like the existing muted project marker accent), never the thread
  title, lifecycle state, or selection color. The complete `Codex` or
  `OpenCode` label is reserved before variable Project/Host context is
  truncated, so provider identity remains visible at the supported 32-cell
  navigator width and in bounded Workstream detail.
- The first delivery adds no provider filter, grouping axis, role, preset, or
  provider-management page. Those remain deferred unless use demonstrates a
  need. Existing Codex observer management stays unchanged.
- Hardcoded "Codex"/"native Codex UI" strings become provider-aware labels.

### Unchanged invariants

- Each live Runtime remains one provider process in its own private tmux
  server; never a shared cross-Runtime daemon or provider `--remote`/
  client-server topology. OpenCode's per-Runtime embedded loopback backend is
  allowed only as a private provider implementation detail. Its one exact
  host-owned observer sidecar is WSNav control-plane observation, not another
  provider process, and never owns a terminal pane.
- WSNav never writes status or management traffic into the provider pane.
- WSNav never persists prompts, responses, tool output, transcripts, or raw
  provider payloads from any provider. The no-transcript boundary applies to
  opencode's event-sourced SQLite transcript store as much as Codex's history:
  the opencode observer discards event content before state adaptation, and
  WSNav scopes provider state per managed host without ingesting it.
- The provider owns the conversation, composer, models, naming, resume,
  history, and native workflow for each provider kind.
- Fail-closed identity, revision-guarded transactions, and exact
  single-candidate recovery remain provider-independent requirements.

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
