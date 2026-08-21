# Workstream Navigator V1 Design

Date: 2026-08-20

Status: D16 host-local simplification complete, including operator-gated live
local and SSH-entered-host acceptance; V1 remains a source-installed operator
beta; no compatibility contract

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
[roadmap](roadmap.md#2026-08-04-terminal-fidelity-root-cause-is-upstream-tmux).

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
  bounded D8 provider-aware New, exact resume, same-provider Fork, and
  lost-Runtime recovery contract. Real OpenCode acceptance currently covers
  `1.18.11`; release numbers are diagnostic evidence, not compatibility
  authority.
- A minimal terminal experience that defaults to the Navigator beside the
  directly interactive native provider TUI and may temporarily add at most one
  ephemeral utility shell below the provider.
- One current-host registry with read-only capability and observer-readiness
  checks plus contextual readiness guidance.
- Projects represented by one or more explicitly registered roots on that host
  only. Project grouping is presentation state and never grants host authority.
- Host-private Project-directory browsing for ordinary registration, rooted at
  the current host's configurable workspace root (`~` by default).
- Workstream creation, switching, parking, exact resume, and display through
  the current tip's provider-owned native name when that metadata surface is
  supported.
- Navigator-local Workstreams, Projects, and Archived pages. Workstreams is the
  default operational home and always groups active Workstreams by Project;
  Archived is a separate restore page rather than another Workstreams view.
- Reversible Workstream archive and restore for removing inactive work from the
  ordinary navigator without deleting provider history or Git state.
- Independent workstreams started at a registered project root.
- Conversation-forked workstreams that retain the same registered project root.
- Read-only Git registration and credential-free origin metadata for host-local
  Project grouping; no Git lifecycle ownership and no association between
  separate execution hosts.
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
  outer operator-established SSH session, that session may end the disposable
  presentation; reconnecting and rerunning wsnav on the host reattaches to the
  same private Runtime/provider.
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
| `ProjectLocation` | One registered Git project's canonical root on the current host | That host's Workstream Navigator registry |
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
    │   └── wsnav TUI
    ├── provider pane
    │   └── wsnav attach helper
    │       └── local host helper -> exact Runtime tmux server
    │                                                  └── native provider TUI
    └── optional utility-shell pane (at most one; below provider)
        └── account shell at exact ProjectLocation root

wsnav TUI
├── current-host registry projection
├── contextual provider-readiness guidance
└── host-local action and attachment boundary

the execution host
├── private SQLite state
├── one private tmux server per live workstream runtime
│   └── exactly one session, window, and provider pane
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

The presentation begins with exactly the Navigator and provider panes. The
D12 utility-shell action may split only the provider region once,
placing one shell below the provider. `Ctrl+b "` is the sole shell-creation
gesture: it creates and focuses that pane when absent and otherwise focuses the
existing shell. `Ctrl+b %` does not create an alternate orientation or a
second pane. Unknown or duplicate pane-role evidence is ambiguity and must
leave the layout unchanged.

The private presentation does not inherit tmux's general-purpose prefix or root
management tables. Its prefix table is rebuilt as an explicit allowlist:
`Ctrl+b "` opens or focuses the shell; `Ctrl+b %` gives bounded guidance;
`Ctrl+b x` confirms close
only for the utility shell; `Ctrl+b d` detaches; `Ctrl+b o` and directional
keys move among owned panes; `Ctrl+b Ctrl+b` delivers a literal `Ctrl+b` to the
focused application without exposing the nested Runtime's tmux prefix table;
and `Ctrl+b ?` shows only this curated help. Its root table retains only the
primary mouse selection/forwarding and bounded scrolling/copy interactions
required by the existing Navigator and native provider surfaces. Default
right-click management menus, mouse split/swap/kill/respawn actions, arbitrary
tmux command prompts, additional splits, windows, sessions, and layout mutation
bindings are absent. These restrictions belong only to WSNav's private
presentation server and never modify the user's ordinary tmux server or
configuration.

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

The optional utility shell is presentation state, not a Workstream, Runtime,
provider session, or durable terminal. It may open only beside one exact live
`Running` provider attachment. The host resolves that attachment's opaque
Workstream identity to its canonical registered ProjectLocation root. The
host-local account shell starts there directly. Pending, completed, failed,
blank, observer-review, dead, stale, or ambiguous provider surfaces create no
shell.

The shell keeps its launch host and root until it exits. Selecting a different
Workstream automatically closes the exact utility pane before replacing the
provider attachment; it never retargets the live shell or leaves Workstream B's
provider above Workstream A's shell. This adds no confirmation step to the core
switching workflow. Reselecting or reconnecting the same exact Workstream does
not close its shell. If exact utility ownership or cleanup
cannot be proven, the switch fails closed before changing the provider pane.
This is a deliberate V1 simplicity choice: the utility is short-lived scratch
space for the currently displayed Workstream, while longer-running commands
belong in an ordinary terminal. Shell exit, `Ctrl+d`, automatic cross-Workstream
cleanup, or the guarded shell-only close binding removes its pane immediately
and restores the two-pane geometry; Navigator and provider dead-pane retention
remain unchanged. WSNav persists no shell identity, command, output, history,
terminal capture, or restoration record. A live shell may naturally survive a
client detach only while its disposable presentation tmux server remains alive;
presentation loss ends it, and WSNav never reconstructs it.

When this presentation is itself running inside an ordinary operator SSH
session, an outer disconnect may end or detach the disposable presentation and
its shell. It must not stop, park, rotate, or restart the host's private
Runtime or provider. Reconnect to that host, rerun `wsnav`, and attach again.

An optional, undecided future expansion could instead treat the provider TUI
and shell as one per-Workstream presentation surface. The private presentation
tmux session could park an inactive Workstream's live utility pane in an
internal window and rejoin it when that Workstream becomes active again. That
would add multi-shell ownership, bounded resource policy, hidden-shell
indication, failure rollback, and cleanup semantics. It is not a V1 commitment
and does not authorize durable shell state or a general-purpose tmux surface.

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

The navigator pane has one Workstreams home page and two infrequent direct
pages rather than a generic management landing page:

```text
Workstreams
├── Project-grouped active Workstreams
└── Recovery

Projects
├── Host-local Project and exact Location list
├── Register Location
├── Start Workstream at selected Location
├── Refresh repository metadata
└── Configure the registration-browser root

Archived
└── Project-grouped archived Workstreams and Restore
```

Workstreams is the default page and retains the product's ordinary switching
workflow. `,` opens Projects and `.` opens Archived; pressing the current
page's key again, or `Esc`, returns to Workstreams. These are direct pages, not
members of a cyclable view mode, and there is no persistent tab bar. A
project-browser modal consumes its own `.` hidden-directory toggle before page
navigation. Workstreams has no Recent or By-host projection and `Left`/`Right`
do not change pages or grouping.

Projects with one Location flatten to one selectable row so a Project name and
its label-source Location are not repeated. Projects with several Locations
retain one display-only Project header and a minimal selectable child tree.
Internal label-source markers, generic `Location` prefixes, inventory counts,
and repeated inline action hints are not rendered. Project actions remain
explicit page-local keys. Provider readiness is not a page or manual setup
mode: the navigator detects it read-only and offers a contextual guide only
when the requested operation needs missing readiness.

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
host contract exists.

Project registration is an explicit navigator flow: `Projects → a` opens a
centered browser inside the navigator pane for the current host. The browser
starts at the host-local configured workspace root
(`~` by default), lists only bounded direct-child names plus a Git marker,
omits dot-prefixed directories by default, and uses a root label and relative
cursor rather than returning absolute paths. `.` toggles one explicit
modal-local hidden-directory request: every new browser starts hidden-off, and
the selected visibility persists while entering children or moving to a parent
inside that browser. The host returns safe dot-directory names only when that
request flag is true; files, `.`, `..`, unsafe names, and canonical paths outside
the configured root remain excluded. The browser sends the visibility flag in
an explicit bounded host-local directory request. The host returns the echoed
visibility state and safe names in the corresponding bounded host-local result.
The bounded result ranks direct Git
repositories before navigation-only directories. Within each tier it groups
explicitly shown dot-directories before visible directories, sorts by a
locale-independent Unicode lowercase key, and uses the original exact name as
its deterministic tie-breaker. This ordering does not change Git detection,
the bounded filesystem scan, or the completeness claim beyond that scan.
`Right` enters the selected directory even when it is a marked Git repository,
`Left` moves to the parent without crossing the configured root, `Enter`
registers only a selected marked Git Project, and `Esc` closes the browser. A
plain-folder `Enter` remains in the browser with bounded guidance to use
`Right`. No letter is reserved for navigation or registration: letters filter
the current listing, including `h`, `j`, `k`, and `r`. Adding a repository that
would otherwise be the configured browser root requires configuring its parent
as the root and selecting the repository normally. `Projects → b` owns that
explicit root-setting action; it updates only the typed host-local
`ProjectBrowserSettings` row and is not a general settings page. The host
resolves the selected relative cursor and performs Git inspection itself, so
raw paths never enter a public snapshot, Project presentation row, host-local
result, or provider pane.
This is a navigator-only modal, not a tmux popup, window, or provider overlay.
Its rows stay single-line at the
32-cell navigator width, truncating names before layout; a bounded viewport
follows the selected directory rather than allowing the selector to scroll out
of the visible modal.
Mouse support in D7 covers selection, primary row activation, forms, and
confirmation. Full mouse parity for every management action is not an
acceptance requirement.

The Workstreams page retains its accepted muscle memory: `Enter` performs the
primary open/start/recover action, `n` starts a sibling at the selected
Workstream's exact ProjectLocation, `f` forks, `p` parks, `a` acknowledges,
and `?` toggles the full reference. `Left` and `Right` remain inert on this
page; they no longer cycle a view. Projects and Archived may reuse letters
because the active page and its visible footer make the page explicit.

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
On Projects, a one-Location Project omits that redundant header and renders the
exact Location as one accented selectable name. Only multi-Location Projects
render a header and child branches. Location rows omit the generic `Location`
prefix, internal label-source marker, active/archived inventory, and repeated
`n new` hint; the page footer owns action discovery.

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
└ Codex                                      1 day ago
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

AwaitRuntime serializes the Start operation; it does not require the durable
Runtime lifecycle to leave `Starting` before native attachment. Once the exact
owned private Runtime record and live process identity exist, the navigator may
attach while its status is `Starting`. The provider's later native
SessionStart observation confirms lifecycle progress; it is not a prerequisite
for giving that provider its terminal client. `Stopped`, `Unknown`, missing,
or identity-changed Runtime evidence still refuses or waits without retargeting.
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
- the Workstream, ProjectLocation, Fork recovery, binding, and attention records
  for work physically running on that host.

Every newly created Runtime derives its private tmux directory and session name
from the complete opaque Runtime UUID. The persisted session value must match
that exact current form before WSNav probes, attaches, parks, or removes a
private server. A narrowly defined former short-ID form is read only for a
Runtime record created by an older build; any other value is ambiguous and no
tmux action is attempted.

The private tmux server owns the terminal container, while SQLite owns bounded
Runtime metadata, exact provider-process authority, and recoverable Start/Fork
state. The native provider owns session history. A closed tmux server is not
proof that its former pane process exited: a provider may survive terminal
hangup or spin on a deleted PTY.

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
composition, not a WSNav transport. If it disconnects, WSNav treats the
disposable presentation as lost while leaving the host's private Runtime and
provider untouched. A later host-local `wsnav` invocation reopens the
presentation and attaches to that exact Runtime.

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
Runtime record plus live tmux/process probes. Fork uses the CompoundOperation
journal, as does OpenCode's non-idempotent blank-session creation boundary.

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
D16 deletes those abstractions together with remote-only JSON, SSH transport,
release/capability handshakes, polling, and cache machinery rather than
retaining compatibility behavior. Subprocesses remain only where the owned
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
adapter does not block Projects, Archived, or attachment to an existing live
Runtime. When the user requests a Codex Start, Resume, Fork, or other operation
that requires an unready observer, the navigator captures that exact intent
and its expected Workstream, Location, integration, and registry revisions,
then offers a contextual readiness guide.

A non-interactive public CLI action never installs, updates, or opens native
review. It returns a typed `observer readiness required` result with bounded
guidance to use interactive `wsnav`; hidden internal preparation entrypoints
remain inaccessible as normal public workflow.

For an absent profile or an exact owned declaration requiring update, the
guide explains the bounded mutation and asks for explicit consent before any
write. Declining cancels the pending action without mutation. A missing
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
`trust_pending`, the contextual guide may replace only the presentation's
right pane with a temporary native, profile-selected Codex review process in
an empty disposable cwd. The navigator remains visible; the operator uses the
normal `/hooks` UI, trusts the exact generated command if desired, and exits
without submitting a prompt. That temporary process is not a managed Runtime
or Workstream and deliberately has no observer authority: an invoked hook
drains and does nothing.

On review exit, WSNav silently re-detects the complete native trust record and
revalidates every captured revision. It continues the pending action only when
the profile is exactly ready and the original intent is still current. An
incomplete or declined native review leaves the owned profile accurately
`trust_pending` and cancels the pending action with retry guidance; changed
revisions likewise cancel rather than retargeting the operation. The guide
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

### D16 breaking state boundary

D16 removes the client catalog from the active architecture. There is no
`ClientCatalog`, client schema, host-registration table, generic preferences
table, importer, client-state compatibility reader, dual write, or rollback
adapter in the target product. The one current-host registry owns both
authoritative runtime state and the narrowly bounded host-local presentation
records needed by the navigator.

Host schema 13 is the explicit D16 boundary. Its migration from host schema 12
is transactional and reads only `host.sqlite`; it never reads `client.sqlite`.
It preserves every existing `HostIdentity`, integration, ProjectLocation,
`ProjectBrowserSettings`, Workstream including provider and activity fields,
independent-creation request, Runtime generation, OpenCode Runtime handle,
provider binding, AttentionState, and CompoundOperation. It creates fresh
host-local Project records from the preserved ProjectLocations and creates no
persisted page/view preference or host-label state. Client-only hidden state
has no schema-13 replacement.

Production schema support is deliberately narrow: current-only open accepts
exact schema 13, confirmed cutover accepts exact schema 12, and
observer-transition accepts only schema 12 or 13. Host schemas 0 through 11,
missing or malformed schema evidence, and all other versions return typed
state-recovery/reset-required without mutation; versions newer than 13 fail
closed as unsupported future state. D16 removes the production incremental
pre-12 migration code and its behavioral tests, retaining only an exact
schema-12 fixture for the 12-to-13 migration tests. Fresh-create writes schema
13 directly.

Project reconstruction is deterministic apart from the deliberately fresh
opaque Project IDs. Locations with one exact credential-free origin
fingerprint share a Project on this host. Missing, ambiguous, or local-path
identities each create a separate Project. Reconstruction orders grouped
locations by opaque LocationId, stores the first as `label_location_id`, and
takes that location's bounded repository display name as the primary Project
label. The safe origin display stays separate secondary `↗` context. Identical
fingerprints on separate wsnav hosts still create independent Projects because
no state crosses the host boundary.

After cutover, Project IDs and associations persist in schema 13. A newly
registered location joins an existing Project only on one exact fingerprint;
otherwise it creates a fresh Project. At most one Project on this host may own
each non-empty exact fingerprint. Missing, ambiguous, and local-path identities
are stored without a grouping fingerprint and never collide through an empty
sentinel.

Membership changes use one deterministic state machine. The Project already
owning a matching fingerprint survives a merge and retains its existing label
source. An unmatched changed fingerprint updates the same Project when the
location is its sole member, or creates a fresh Project and splits that
location from a multi-location Project. A new split Project selects the lowest
moved LocationId as its label source. A source Project retains its label source
while that location remains a member; only departure of that exact location
selects the lowest remaining LocationId. An emptied source Project is deleted.
An accepted display-name change updates the Project label only when it belongs
to the recorded label source. A missing, foreign, or non-member label source is
invalid persisted state and fails closed. Joins therefore cannot randomly
rename a Project merely because a new opaque LocationId sorts earlier. The
matching member's safe origin display remains separate secondary `↗` context.
Missing or ambiguous later evidence may update bounded location display
metadata but never dissolves an existing association or clears the Project's
last positive fingerprint.

Changed repository evidence is never gathered during ordinary navigation,
snapshot, redraw, attachment, or Workstream switching. Initial registration
inspects only the selected path. Later reassociation requires the explicit
Projects-page metadata refresh action. That action captures the selected
Project and Location revisions, performs bounded network-free Git inspection
outside SQLite in LocationId order, then applies the complete still-current
result in one transaction. If any inspection fails, any output is unsafe, or
any captured revision is stale, the whole refresh performs no metadata or
association mutation. A successful inspection with no exact unambiguous
identity may update bounded location display metadata but preserves its
association. These rules change presentation only and never retarget a
Workstream's exact ProjectLocation.

The retired client files are exactly `client.sqlite`, `client.sqlite-wal`, and
`client.sqlite-shm` under the selected state root. Their contents—including
remote registrations, cross-host Project associations, old Project IDs,
hidden state, aliases, cached capabilities, executable paths, and preferences—
are discarded without inspection or import. D16 does not create a backup. An
operator who wants downgrade insurance must first park or stop managed
Runtimes, exit WSNav, and create a verified offline copy of the complete state
root before accepting cutover. That procedure is outside D16; restoring the
complete external copy before launching an old binary is the only rollback
path.

Only an ordinary interactive `wsnav` launch may authorize an existing state
root's D16 cutover. It detects the boundary before opening current host state
or reusing a presentation, then presents one launcher-owned,
pre-presentation terminal confirmation. The confirmation names the discarded
client/presentation data, the preserved host/runtime data, and any exact
legacy presentation that will be retired. It never renders in or writes to a
provider pane. Declining exits without mutation. Hooks, observer sidecars,
hidden helpers, and direct scripting commands never confirm or start the
transition; they drain or fail closed with bounded guidance to run interactive
`wsnav`.

Opening host state does not implicitly cross this boundary. The state layer has
separate current-only, observer-transition, fresh-create, and
confirmed-cutover entrypoints. Current-only accepts schema 13 only when no
exact legacy client file exists and never creates, removes, or migrates;
Navigator snapshots, actions, helpers, and scripts use it.
Fresh-create may create schema 13 only when the selected state root was absent,
or when an existing private directory is empty or contains exactly one private
unlocked `transition.lock` regular file owned by the current user. It then
acquires that exact lease, repeats the complete allowlist check while holding
it, and holds it through database creation. Any host SQLite main, WAL, or SHM
file; any legacy client file; `run/`; `presentation/`; either exact D16 observer
handover journal path; a locked, malformed, foreign, or non-regular transition
lease; or any unknown entry returns typed `state recovery required`.
Fresh-create never scans, adopts, signals, cleans, or overwrites those
artifacts. This prevents both a time-of-check race and a missing database from
minting a new HostIdentity beside an orphaned live Runtime or presentation.

Seeing schema 12 or any legacy client file returns a typed `cutover required`
outcome. Only the confirmed interactive transition entrypoint may invoke
client-file reset and, for schema 12, the explicit 12-to-13 transaction. A
schema-13 root contaminated later by any exact legacy client file therefore
requires confirmation again, removes only those files, and performs no schema
migration. A missing host database beside any non-fresh artifact is recovery,
not cutover or fresh creation.

Observer-transition is the one narrow upgrade bridge. Codex hooks and new
generation-bound OpenCode observer sidecars may open exactly host schema 12 or
13 without creating, migrating, or reading any client file. That handle
exposes only the unchanged Runtime, ProviderBinding, AttentionState, and
observer-lifecycle reads and writes required to accept already-authorized
provider evidence; Project, presentation, navigation, and user-action methods
are unavailable through its type.

Observer-transition has an explicit contention contract rather than relying on
SQLite's default immediate `BUSY` result. The unchanged native Codex profile
retains its three-second hook timeout. D16 limits its end-to-end hook work to
2.75 seconds: payload, provenance, and App Server work finish within the first
1.75 seconds; the next 750 milliseconds are reserved for the host transaction;
and the last 250 milliseconds are reserved for bounded failure recording. Only
`SQLITE_BUSY` and `SQLITE_LOCKED` are retried, with monotonic bounded backoff
until the database deadline; every other database error leaves that retry loop
immediately. The final 250 milliseconds before the native timeout remain an
outer scheduling and exit margin.

Once an event has passed exact managed-Runtime authority, failure to commit it
by the database deadline atomically creates or retains one private
`run/<RuntimeId>/observer-degraded/<sha256-generation>` regular file. The
full lowercase digest makes concurrent or stale Runtime generations distinct;
snapshot and action paths derive only the current generation's exact filename
and never discover markers by scanning. Its versioned bounded body contains
only the RuntimeId, Runtime generation, and a closed failure-reason enum; it
contains no session event, turn/message ID, provider payload, or diagnostic
text. A matching marker makes snapshots render that Runtime `unknown` and
makes observer-dependent actions unavailable. A later unrelated event never
clears it: only exact provider reconciliation or deliberate Runtime retirement
may do so. Marker failure itself returns a bounded hook error, emits no pane
output, and grants no mutation authority. OpenCode observer-transition writes
use the same retry and degraded-marker contract. The schema migration
constructs and validates its complete Project plan before client-file deletion
or writer acquisition, revalidates the plan inside the transaction, and limits
writer acquisition plus transactional work to 500 milliseconds. Reaching that
deadline rolls back and leaves schema 12 for an ordinary retry. The strict
500-before-750 database budget makes a racing D16 observer commit wholly before
or after migration or leave explicit degraded evidence instead of being
silently discarded.

An already-running pre-D16 OpenCode observer lacks that contention contract and
does not remain the writer across migration. After confirmation but before any
client-file deletion, D16 enumerates every live OpenCode Runtime in opaque
RuntimeId order and corroborates its recorded helper PID/birth, executable
identity, generation, endpoint, and observer status. An exact D16 observer is
revalidated in place; a pre-D16 observer enters the handover below. Ambiguous
or mixed identity refuses before signaling. For each required handover, D16
starts one observer-transition sidecar in standby and waits until it has proved
endpoint ownership and opened a live SSE stream. Standby parses into a bounded
in-memory event buffer but has no host-state mutation authority.

Before the first sidecar signal, the launcher durably writes an exact private
`d16-observer-handover.json` journal through its one recognized
`d16-observer-handover.json.tmp` replacement path and syncs the state
directory. The journal contains only bounded Runtime and generation IDs, old
and standby PID/birth/executable identities, the expected observer-handle
revision, and a handover phase; it contains no provider payload. With the
transition lease still held, the launcher sends `SIGSTOP` only to the
corroborated old sidecar, proves that it is stopped and the old handle is
unchanged, then compare-and-swaps the observer handle to the standby PID/birth.
The standby becomes authoritative only after observing that committed
assignment and then reconciles and drains its parsed buffer. It durably records
an exact private `d16-observer-handover.ack` through the sole recognized
`d16-observer-handover.ack.tmp` path only after replay completes; the bounded
proof contains Runtime/generation, standby PID/birth/executable, and assigned
handle revision, never provider payload. The launcher requires that exact
post-activation acknowledgement and rechecks the process before terminating
only the frozen old PID/birth. The acknowledgement lets a newly confirmed
launcher finish post-swap cleanup even when the original readiness pipe was
lost.
Repeated non-terminal status is idempotent, and settled evidence is
deduplicated by exact generation, native session, and provider message ID.

The handover journal is restartable under a newly confirmed cutover and the
same exclusive lease. A valid pre-swap phase either restores the exact old
observer and removes the standby or safely repeats the swap; a valid post-swap
phase completes exact old-sidecar cleanup and proves the assigned D16 sidecar
ready. Missing, changed, malformed, or internally inconsistent process,
handle, journal, or activation acknowledgement signals nothing and returns
typed transition recovery required. Current-only refuses while any exact
journal or acknowledgement path exists, and all are removed with a synced
directory only after every recorded handover is complete. Failure to establish
all replacements refuses before client-file deletion. The provider process,
Runtime generation, native session, terminal, and completed output remain
unchanged. A failed later migration leaves the new sidecars operating against
intact schema 12. This bounded observer handover and the schema-12/13 handle
are host-state transition support, not a client-catalog reader or remote
compatibility path.

The interactive launcher proves and retires legacy presentation ownership
before touching durable state. It enumerates only the selected state root's
owned presentation directories and exact private tmux sockets, and verifies
the session, pane roles, navigator PID, process birth, executable identity,
client count, and auxiliary-pane state. Multiple, malformed, inaccessible, or
foreign presentations fail closed. An attached client, live utility shell, or
native observer-review surface also blocks mutation; the launcher may attach
the exact legacy presentation in a drain-only path without opening host state
so the operator can finish or exit that ephemeral work and quit the old
presentation, then rerun cutover. The drain path starts no D16 state action and
never touches a Runtime server.

After confirmation, the launcher takes one exclusive state-root transition
lease before any presentation or durable mutation. Current-only,
fresh-create, confirmed-cutover, and D16 control paths honor that lease;
observer-transition deliberately does not and continues to serialize its
narrow writes through SQLite. With the lease held, the launcher repeats
presentation discovery and the exact client-file-holder proof. One detached,
ordinary two-pane legacy presentation may then be retired by killing only its
exact presentation tmux server. Cutover waits for the verified navigator
PID/birth to disappear and for the presentation socket and directory to be
gone. Exact dead owned presentation artifacts may be removed under the same
confirmed lease; malformed or foreign artifacts are never guessed or deleted.
Cutover then repeats both proofs and holds the lease through reset and
migration. Provider Runtime tmux servers, provider processes, completed output,
and provider sessions are not signaled or restarted; only the exact confirmed
OpenCode observer handover above may replace a sidecar. Any ambiguity or
concurrent D16 owner refuses before mutation.

With that proof held, D16 removes only the three exact legacy client paths,
syncs the private state directory, then transactionally migrates host schema 12
to 13. Removal and migration are restartable: absence of any retired client
file is success, a partial removal is retried, and a failed host transaction
leaves `host.sqlite` at schema 12. Observer-transition remains available while
ordinary navigation stays blocked. No Start, Resume, Park, provider signal,
Runtime rotation, provider-input action, utility-shell termination, or
observer-review termination is part of the durable transition. The confirmed
OpenCode observer handover is complete before this reset phase and never
changes provider lifecycle.

Downgrade after cutover is unsupported. A pre-D16 binary sees future host
schema 13 and fails closed. D16-only Project IDs and label-source state are not
reverse-synchronized. This is an intentional clean break, not a
migration-preservation promise.

The current-host display label is derived, not durable state. The operating-
system hostname is used only when its trimmed value passes the bounded
single-line, no-control-or-format-character, 64-scalar validator; otherwise
the stable fallback is `host-` plus the first eight lowercase hexadecimal
digits of `HostId`. Labels are rendered with bounded-width truncation and are
never identity or command input.

### Host registry

The host registry contains:

```text
HostIdentity
  host_id, registry_generation, schema_version

CodexIntegration
  integration_id, profile_name, canonical_profile_path, owner_id,
  profile_schema_version,
  hook_executable_path, generated_content_hash, lifecycle, revision

ProjectBrowserSettings
  singleton, root_path, revision

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
  operation_id, request_key, kind=start|fork, phase, expected_revisions_json,
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

- One Project contains one or more ProjectLocations owned by this host.
- One ProjectLocation references exactly one Project.
- Project identity and label-source state are presentation metadata and never
  authorize a host, Git, provider, Workstream, or Runtime action. The label
  source references exactly one current member location.
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
starting | idle | working | attention | stopped | unknown
```

`unknown` is an observation boundary, not proof that a runtime stopped.

## Git project-root policy

Registration performs bounded, read-only Git discovery without contacting a
network. It normalizes a supplied repository path, linked worktree, or child
directory to the repository's primary non-bare worktree. That canonical path
is the registered `ProjectLocation` root and the launch cwd for every WSNav
Workstream at that location.

The explicit Projects-page metadata refresh repeats that same bounded,
network-free inspection only on the selected Project's current locations and
under captured revisions. No passive snapshot, redraw, attachment, or switch
reinspects Git.

Registration may read configured Git remotes without contacting them. It
normalizes a credential-free origin identity into a bounded fingerprint and
safe display label for same-host Project grouping; credentials, query strings,
raw URLs, and transport-specific secrets are neither persisted nor rendered.
The fingerprint is presentation evidence only. It never associates locations
owned by separate wsnav hosts and never authorizes a filesystem, Git, provider,
or Runtime action. Repositories without a safe origin identity remain valid
and group only through explicit host-local presentation state.

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
-> navigator resolves the current host's authoritative registry
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

The Projects page makes each exact ProjectLocation selectable. Pressing `n` on
a Location starts an independent Workstream at that root, including when every
older Workstream there is archived. A Project header is non-actionable and
shows bounded guidance to select one of its Locations. When Workstreams has no
active row, `n` opens Projects for Location selection if any registered
Location exists; it enters ProjectLocation registration only when none exists.
The registration flow supplies the initial unstarted Workstream at the chosen
new Location. It never silently chooses among multiple Locations or revives an
archived Workstream. This closes the dormant-Project path while preserving the
same-Location fast path for ordinary `n` from an active Workstream.

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
Current host
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
- Start Workstream from a selected Workstream at its exact registered root or
  from an exact selected ProjectLocation on Projects; route an empty active
  page to existing Locations before offering registration;
- Fork Workstream from an exact managed source;
- inspect bounded Workstream status and rename the current tip through Codex's
  canonical thread-name field;
- park/resume without deleting provider history;
- archive a Workstream out of the active list and restore it without starting
  Codex or deleting its retained state;
- route a repeated Fork to its exact unresolved operation and reconcile it;
- inspect and register ProjectLocations on the current host and configure the
  registration-browser root from Projects;
- detect observer readiness without mutation and guide the user contextually
  through explicit-consent profile preparation and native trust review only
  when a requested Codex operation requires it;
- show the derived current-host label outside provider content, with exact
  visual treatment deferred to a later UX checkpoint; and
- acknowledge result or recovery attention without injecting provider traffic.

The normal human workflow begins with bare `wsnav` and requires no later
`wsnav` shell command. Public CLI equivalents remain supported for scripting,
diagnosis, direct attachment, and break-glass recovery, but the documentation
and empty states never send the user to them for an ordinary WSNav operation.
Installing or upgrading the host-local executable, establishing an outer SSH
connection, cloning repositories, native Codex input and hook approval, and
deferred Git cleanup remain external prerequisites or explicitly excluded
operations.

The Workstreams page has one Project-grouped active projection; Archived is a
separate direct page rather than a view mode.
Archive is the ordinary answer to accumulated test or inactive Workstreams;
there is no hard-delete action. Projects disappear from the active operational
page when they have no active Workstreams, but remain available through
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

The Projects page reflects the ownership boundary explicitly: each host-local
Project is a presentation group of one or more registered ProjectLocation
roots owned by the current host. Registration resolves the bounded relative
cursor locally for Git inspection; no path is written into provider panes or
public Workstream snapshots. Credential-free origin matching may preserve
same-host grouping, but cross-host merge/split, permanent Project deletion, and
manual repository cleanup remain outside the product.

Projects render explicit selectable host-local ProjectLocation rows with a
bounded repository display name. A single Location is flattened without a
duplicate Project header; multiple Locations retain a display-only group
header and minimal tree. `n` starts an independent Workstream at the selected
exact Location; it is inert on a Project header. `a` adds an existing project
path through the navigator-local directory browser, and `b` changes that
browser's typed host-local root.
`r` explicitly refreshes bounded network-free repository metadata for the
selected Project's current locations under the revision-checked transaction
defined above; ordinary navigation and switching never perform that work.
When Workstreams is empty but registered Locations remain, `n` routes to this
selection surface; only a registry with no Locations enters registration.
Once an active Workstream is selected, `n` retains the exact same-Location
fast path without reopening Projects. Registration may produce the bounded safe
origin fingerprint and label used for same-host presentation; no cross-host
association is produced.

There is no Project-level hide, forget, remove, or `x` action in D16. Workstream
archive/restore is the one reversible visibility mechanism, so an archived
Workstream never becomes unreachable from the ordinary TUI behind a second
hidden layer. Project and ProjectLocation deletion, repository cleanup, and Git
mutation remain outside the product.

There is no Hosts or settings page. Provider capability and observer readiness
appear only as bounded context in the operation that needs them. If observer
review is required, its native profile-selected Codex TUI runs in the right
provider pane through the same host-local terminal boundary and leaves no
Workstream behind. The user alone approves trust; preparation never writes
trust state. Exact diagnosis may be surfaced in the contextual refusal, while
removal remains the exceptional documented cleanup flow defined above.

Navigator page changes, forms, and finite management actions leave the current
provider attachment and focus unchanged. Only an explicit Workstream primary
action or observer review replaces the right pane. Potentially slow Git,
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
| Local presentation exits | The host Runtime continues; reopen the host-local navigator and attach |
| Outer SSH connection drops | The disposable presentation may end or detach; the host Runtime/provider continues, and rerunning host-local `wsnav` reattaches |
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
| Project working-tree, branch, or origin state changes | Leave working-tree and branch state entirely to Codex or the user; keep grouping unchanged unless the user invokes the explicit bounded metadata refresh, which never mutates Git |
| Host registry identity or generation evidence is ambiguous | Reject the affected mutation and require explicit local diagnosis or recovery |
| Host database is absent beside any state-root artifact | Return typed `state recovery required`; never mint a HostIdentity, adopt or signal a Runtime, remove a presentation, or clean an unknown artifact |
| A pre-D16 presentation/controller exists at cutover | Before confirmation, allow only an exact no-state-open drain attachment; after confirmation and the transition lease, retire one verified detached ordinary presentation, but refuse ambiguous, foreign, attached, utility, or observer-review state before durable mutation |
| Confirmed D16 reset is interrupted after client-file removal | Retry exact cleanup and the transactional schema 12-to-13 migration; never infer rollback or mutate provider lifecycle |
| A D16 observer meets a concurrent host writer | Retry only SQLite `BUSY`/`LOCKED` within the reserved database deadline; if an exact authorized event still cannot commit, write the bounded generation-scoped degraded marker so snapshots show `unknown` and observer-dependent actions remain unavailable |
| D16 is installed while a schema-12 Codex Runtime remains live | New hooks use only the observer-transition handle so accepted lifecycle and attention evidence continues before confirmation; keep every action and Navigator open behind cutover-required |
| A pre-D16 OpenCode observer remains live at cutover | Before reset, journal exact identities, establish a mutation-inert D16 standby stream, freeze the proven old helper, compare-and-swap the observer handle, activate the standby, and terminate only the frozen old sidecar |
| OpenCode observer handover is interrupted | Under a newly confirmed cutover and the transition lease, replay only a valid exact journal phase; restore the old observer before the swap or complete new authority and old-helper cleanup after it, otherwise signal nothing and require transition recovery |
| `wsnav-observer` is absent or awaiting trust | Preserve existing Runtime attachment; on an observer-dependent request offer the explicit-consent contextual guide, then continue only after exact readiness and revision revalidation |
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
  every later Git decision stays inside the native Codex session or ordinary
  user tooling.

## Proposed Rust structure

```text
src/
├── main.rs               CLI entrypoint
├── app/
│   └── local.rs          typed in-process snapshot/apply/attach facade
├── domain/               pure IDs, entities, statuses, invariants
├── state/                SQLite schema, revisions, and provider-Fork recovery
├── runtime/
│   ├── tmux.rs           dedicated server/session ownership and probes
│   └── attach.rs         safe host-local native attachment helpers
├── provider/
│   ├── mod.rs            capability-driven provider interface
│   ├── codex/            native runtime, profile, App Server, hooks
│   └── opencode/         native runtime, HTTP metadata, SSE observer
├── repository.rs         read-only host-local project-root discovery
├── tui/                  minimal navigator state, rendering, input, mouse
└── internal/             hook/observer and launch-barrier entrypoints only
```

Generic host clients, local/remote endpoints, framed protocols, control ABIs,
SSH adapters, release handshakes, and cross-host catalog plumbing are not
target modules. D16 removes them with their compatibility dimensions; it must
not preserve dead surfaces merely to keep the retired protocol working.

The provider interface remains small and capability-based, with concrete Codex
and OpenCode implementations. No speculative Claude abstractions or generic
lowest-common-denominator behavior should shape either implementation.

## Validation and acceptance evidence

Historical spikes validate transport, native presentation, the shell-only
per-Runtime tmux topology, and the automated local two-pane Codex presentation
path. Terminal presentation is a settled design prerequisite: Spike 0005
proves the selected retained-TMUX configuration, direct native attachment,
keyboard submission, image attachment request, resize/focus, reconnect, and
result-tip preservation. The frozen Python Phase 7F trial independently
observed direct native-pane interaction, terminal color, and click-to-select
mouse support in an equivalent private-tmux layout. That implementation is
behavioral evidence only; it is not a Rust dependency or compatibility
constraint.

The D3 and later SSH acceptance records remain truthful historical evidence of
the former WSNav-managed cross-host behavior. They are not current-contract
acceptance. D16 replaces that path with host-local acceptance on the machine
where wsnav runs; ordinary SSH used to enter another host is operator-created
composition and is not a WSNav adapter or managed transport.

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
   the registered project root; WSNav performs no Git lifecycle mutation, and
   only registration or explicit metadata refresh performs bounded read-only
   repository inspection.
5. **Host-local operation:** the typed in-process application facade returns
   bounded semantic results to navigator and public CLI, tolerates local
   presentation loss, supports multiple same-user tmux attachments, and never
   mutates an ordinary tmux server. Generic host clients, local endpoints,
   framed JSON, and control ABIs have no active caller or compiled surface.
6. **Composed-host acceptance:** run separate host-local wsnav instances on a
   local machine and on a machine entered through ordinary operator SSH;
   exercise each host's start, switch, fork, attention, reconnect, and runtime
   recovery independently without a WSNav cross-host control path.
7. **Clean-break state cutover:** detect schema 12 before current-state or
   presentation open, confirm in the launcher, exercise exact legacy
   presentation drain/refusal/retirement cases under the transition lease,
   remove only the three exact client files, and migrate host schema 12 to 13
   without reading client state or changing a Runtime/provider lifecycle.
   Prove fresh creation accepts only an absent root or the exact private,
   unlocked transition-lease artifact and returns state-recovery-required for
   every database, Runtime, presentation, locked/malformed lease, and unknown
   artifact. Interruption is retryable; downgrade without a complete external
   state-root backup is unsupported. Prove schema 13 preserves the typed
   Project-browser setting, Workstream provider/activity fields, independent-
   creation requests, OpenCode handles, and all other enumerated host rows;
   schema 0 through 11 and malformed or future state fail closed without an
   incremental production migration path.
8. **Host-local presentation:** preserve deterministic same-host origin-based
   Project grouping, exercise merge/update/split/orphan behavior, and prove
   Project labels remain bound to their stable source across joins and merges,
   update only from that source, and select the lowest remaining LocationId
   only when the exact source leaves. Prove only the explicit revision-checked
   Projects action reinspects Git, remove Project hide/forget/`x`, retain
   Workstream archive/restore as the only visibility mechanism. Prove the only
   ordinary pages are Project-grouped Workstreams, Projects, and Project-
   grouped Archived; Recent, `ViewMode`, Hosts, and remote selectors are absent.
   Verify an exact Projects Location can start a Workstream for a dormant
   Project, empty active state routes to existing Location selection before
   registration, restore returns selected-but-unstarted to Workstreams, and the
   derived hostname/HostId display has no persistence or action authority.
9. **Live-observer continuity:** replace the installed executable while an
   exact schema-12 provider Runtime remains live and accept lifecycle/attention
   evidence before confirmation through observer-transition. Hold a competing
   writer lock to prove a D16 Codex hook retries `BUSY`/`LOCKED` within its
   750-millisecond database reserve and that migration either commits within
   its shorter 500-millisecond writer budget or rolls back at schema 12. Prove
   an exact event that cannot commit creates only the bounded generation-scoped
   degraded marker, renders `unknown`, blocks observer-dependent actions, and
   emits no provider-pane output; malformed, unmanaged, or raw event data never
   enters that marker. Race another exact event across schema 13 migration and
   prove it commits wholly before or after, or leaves that explicit degraded
   evidence, without binding rotation. For multiple pre-D16
   OpenCode Runtimes, prove deterministic handover order, standby SSE readiness
   and bounded parsed buffering without mutation authority, durable exact
   journal phases, old-helper freeze before compare-and-swap, activation only
   from the assigned handle, idempotent status and exact settled-message
   deduplication, and exact old-PID/birth termination. Inject a process exit or
   launcher interruption at every journal phase and prove exact restore or
   completion; malformed or changed evidence signals nothing, and inability to
   establish every replacement refuses before reset. The provider process,
   terminal, Runtime generation, native session, and completed output remain
   unchanged.
10. **Contextual Codex readiness:** navigator startup and unrelated provider
    use are read-only and non-blocking. Prove explicit consent precedes every
    exact owned-profile creation/update; decline mutates nothing; native review
    never grants trust; only exact readiness plus captured-revision
    revalidation resumes the pending intent; and incomplete, stale, foreign,
    modified, disabled, ambiguous, or live-Runtime-blocked cases fail closed
    without disrupting existing attachment. No ordinary setup/settings page or
    public setup/update command remains, while exceptional removal preserves
    any state it cannot prove it owns exactly.

Passing fixtures contain only provider/version fingerprints, assertion
booleans, event relationships, timings, and cleanup proof. Assisted diagnostics
cannot become passing fixtures.

The in-process facade returns one deterministic, bounded host-local snapshot.
The existing hard Workstream limit remains an explicit typed refusal rather
than a cursor protocol; D16 removes snapshot cursors, page frames, replay
tracking, and page-count machinery with the transport that required them.

## V1 delivery checkpoints

The checkpoint sequence and current implementation status are maintained in the
[V1 roadmap](roadmap.md). The summaries below define the architectural boundary
of each checkpoint.

The D0-D15 summaries below preserve the implementation and acceptance evidence
that was true when each checkpoint completed. D3 and later references to
WSNav-managed SSH, remote hosts, cross-host grouping, or combined local/remote
acceptance are historical surface descriptions, not current requirements;
D16 retires those surfaces in favor of host-local wsnav instances composed by
ordinary operator SSH.

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

### D3 — SSH hosts (historical; retired by D16)

- Host registration, handshake, snapshot polling, apply, and attach.
- Remote start, attach, reconnect, status, and cold resume.
- Strict protocol and capability diagnostics.

### D4 — Workstreams and forks

- Independent Workstream action at the registered project root.
- Exact-turn App Server conversation fork at that same project root, followed
  by native TUI resume. Git branches and worktrees remain a native user/Codex
  concern rather than a WSNav operation.

### D5 — Recovery and V1 acceptance (historical; cross-host surface retired by D16)

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

- project grouping uses persisted host-registry Project rows and
  credential-free origin evidence only within one execution host; D16 discards
  the retired client catalog and rebuilds fresh presentation identity from
  authoritative ProjectLocations;
- each execution host runs its own installed wsnav; ordinary SSH is an
  operator-established composition boundary, not a WSNav deployment or control
  system;
- multiple same-user tmux attachments are allowed without an input lease;
  simultaneous typing is a user-coordination concern;
- ambiguous host identity, registry generation, or ownership evidence fails
  closed rather than authorizing adoption or mutation;
- host-local status propagation uses bounded snapshots and observer evidence;
  remote polling, cache, backoff, and unreachable-state machinery are retired;
- durable compound-operation recovery is limited to Fork; independent
  Workstream creation is transactional before native launch;
- V1 parks Workstreams but never operates on Git worktrees or branches;
- managed Codex launches select the exactly owned `wsnav-observer` profile over
  the normal user configuration while ordinary launches remain untouched;
  composing another selected profile is deferred, and readiness preparation is
  contextual to an observer-dependent request rather than a setup page;
- Workstream display names come from the current Codex tip thread rather than a
  shadow Workstream label, with context-specific computed fallbacks ending in
  the stable Workstream short ID; and
- live TUIs use dedicated process-owned runtimes while App Server access is
  short-lived stdio only; each Runtime has its own bounded private tmux server.

The D16 product boundary, clean-break state reset, same-host Project semantics,
direct local facade, reduced pages, and derived current-host display are
settled by the choices above.
Future implementation or provider evidence that contradicts this contract
must narrow or reopen the affected workflow; it does not authorize silently
weakening isolation, trust, result-tip preservation, or the no-transcript
boundary.

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

The current code uses a concrete provider-kind boundary rather than a generic
plugin abstraction. `ProviderKind` and provider-neutral lifecycle, capability,
name, session, state, and bounded DTOs cross the shared layers; one explicit
dispatch at the action boundary selects the concrete Codex or OpenCode
adapter. Provider-owned profile, App Server, HTTP, SSE, and process contracts
remain inside their concrete adapters. The tmux runtime and host-local control
layers remain provider-agnostic.

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
  - and every typed provider-session identifier carried through state or bounded
    action/result DTOs.
- The current host exposes capability sets. It may run Codex and opencode work
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
aliases or Project associations. D16 retires that client schema completely,
imports none of its rows, and rebuilds fresh same-host Project presentation
from authoritative host-registry locations. Historical D8 evidence recorded the provider-bearing
wire-contract bump from protocol 16 to 17. D16 does not preserve that revision
as a compatibility requirement; any surviving host-local boundary or DTO
revision remains implementation-owned. No migration fabricates a model, role,
agent, or provider session ID.

### Provider capabilities and availability

Provider availability is dynamic current-host state, not persisted Project or
display identity. Each bounded host snapshot carries exactly one sorted,
duplicate-free record
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

The current-host state boundary verifies its host identity and registry
generation as required by local authority, but does not persist or compare
`ProviderCapability` as client registration identity. Installing, removing, or
upgrading a provider therefore does not stale host-local setup.
Provider records are scoped to the snapshot that supplied them; there is no
remote-host cache or unreachable-host presentation. Snapshot pagination
repeats the same provider set on every page and rejects inconsistent pages.

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

- From an existing Workstream, its exact current-host ProjectLocation is
  already the target. `n` never reopens Project or location selection.
- From an exact selected ProjectLocation on Projects, that Location is the
  target even when all of its prior Workstreams are archived.
- From an empty Workstreams home, existing registered Locations are selected
  through Projects first; ProjectLocation registration runs only when none
  exists. Provider choice then applies to the new Workstream at that exact
  Location.
- The current host reports which providers are eligible for New by the exact
  capability predicate above. With no eligible provider, creation stops before
  a Workstream is recorded. With one, WSNav selects it without another prompt.
  With more than one, a small navigator-local chooser asks only for provider
  kind.
- When the chooser was opened from an existing Workstream and that
  Workstream's provider remains eligible, it is the initial selection. This is
  contextual UI state, not a remembered per-Project default. If it is not
  eligible, the user chooses from the remaining host-authoritative set.
- The navigator sends the selected `ProviderKind` in the creation request. The
  authoritative current host repeats discovery immediately before its creation
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

Direct CLI creation remains deterministic and any internal host boundary always
carries an explicit provider. `new-workstream` accepts an optional
`--provider`: omission means the source Workstream's provider when it is still
eligible, otherwise the command fails and asks for an explicit kind.
Empty-state Project registration selects the sole eligible provider or requires
`--provider` when several are eligible. CLI commands never select the first
provider by catalog order.

### Provider-scoped readiness

Opening the navigator detects readiness read-only and must not mutate or block
on any provider. Readiness guidance is scoped to the requested provider:

- an unready Codex adapter cannot block Projects, Archived, existing Runtime
  attachment, or an eligible OpenCode action;
- Codex remains `unavailable/observer_not_ready` until an observer-dependent
  request invokes the explicit-consent contextual guide and exact native trust
  review completes;
- the pending request resumes only after exact readiness and captured revision
  revalidation; and
- OpenCode adds no installation, credential, trust, or provider-management
  flow.

This reuses the exact Codex ownership and trust contract without creating a
setup page or generic onboarding system. No Workstream is created merely to
perform provider setup.

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
presentation, dynamic capability records, schema and bounded-boundary DTO
changes, and one Codex
dispatch branch. It does not invent OpenCode behavior or require a speculative
five-surface implementation. With the fresh-session and observer evidence gate
passing on the contract observed with `1.18.11`, D8.1 may add the second
adapter and make shared action/app/state/navigator call sites depend on the
provider boundary
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

`POST /session` is non-idempotent because OpenCode chooses the native session
ID. After reserving an OpenCode Runtime generation, WSNav therefore persists a
bounded `Start` operation while it is still `prepared`. The short-lived server
must pass health first; immediately before the one POST, WSNav durably advances
that exact Runtime/generation operation to `external_effect_started`. The
returned blank root-session ID and ProviderBinding commit atomically with the
operation. A failure before the durable boundary is terminally known-absent and
may be retried in a new Runtime generation. A lost or rejected response after
the boundary atomically records terminal `external_effect_unknown` while moving
the exact Runtime and Workstream to recovery-required; it can never issue a
second blank-session request. No raw response or provider content enters the
journal.

The short-lived server is owned by a state-free WSNav guardian rather than the
mutating action process. The guardian and server use separate private process
groups, and an anonymous pipe held only by the action is their lifetime lease.
A second state-free launch barrier blocks before provider execution while the
guardian captures and proves the future server leader's PID, birth token,
process group, and session; releasing the barrier `exec`s `opencode serve` in
place so that authority cannot race an early provider fork. Before reporting
readiness, the guardian proves that the selected loopback listener belongs to
that exact process tree. The action revalidates guardian liveness, listener
ownership, and the same process-group/session authority after health and again
after journaling, immediately before `POST /session`.

Normal completion closes the lease only after the provider request has
returned; abrupt loss of the action closes it in the kernel. In either case the
guardian performs bounded, revalidated process-group termination, reaps the
leader, and corroborates that the private loopback endpoint is no longer
occupied. A malformed handshake, early server exit, ambiguous identity,
inconclusive cleanup, or surviving listener fails closed. A `prepared` Start
operation abandoned by abrupt action loss is also unresolved because cleanup
was not synchronously observed; unlike a normally returned and terminally
known-absent failure, it cannot automatically retry. The guardian never opens
the state registry or stores provider payloads; the durable `Start` operation
remains the sole authority for whether the non-idempotent boundary may have
been crossed. Independent loss of both the action and guardian is outside this
V1 in-process ownership boundary and would require a stronger external
supervisor or cgroup authority.

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
Project rows or public Workstream snapshots. A new Runtime generation
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
On the current authoritative host it reads the one SSE stream,
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
stops only the recorded observer and provider process by their respective PID
plus process birth, removes only the matching prior-generation handle and
private Runtime artifacts, reserves a new generation, allocates a new loopback
endpoint and observer, and resumes that same bound session. The replacement is
not launched until the exact prior provider identity is gone. A missing binding,
live or ambiguous private Runtime, uncorroborated observer, provider-cleanup
failure, or mismatched handle fails closed. WSNav never opens a native OpenCode
picker, discovers another session, adopts an endpoint, or creates a blank
replacement conversation during recovery.

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
  `OpenCode` label is reserved before variable age or thread context is
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
