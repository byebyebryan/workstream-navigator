# Workstream Navigator V1 Design

Date: 2026-07-28

Status: proposed first pass; not an implementation or compatibility contract

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

## V1 tenets

1. **Preserve the native provider workflow.** Codex owns its composer, models,
   permissions, Plan choices, `/new`, `/clear`, `/fork`, `/rename`, resume,
   history, and transcripts.
2. **Augment instead of intercepting.** Normal work does not pass through a
   manager-owned prompt box, plan router, session wizard, or model picker.
3. **Keep the completed result visible.** Workstream Navigator never writes
   status, routing, synthesis, or completion traffic into the provider pane.
4. **Make workstreams explicit.** A workstream is an independent filesystem and
   runtime lane, not a task record or a synonym for a provider chat.
5. **Treat hosts as locations, not agents.** Local and SSH hosts use the same
   runtime contract. V1 does not transfer repositories, chats, or task context
   between hosts.
6. **Fail visibly and conservatively.** Unknown provider identity, runtime
   ownership, worktree ownership, or remote state becomes `unknown`,
   `unreachable`, or `recovery required`; it is never guessed.
7. **Keep provider history canonical.** Workstream Navigator stores provider
   identifiers needed for exact resume, but no prompts, responses, tool output,
   transcript copies, or rendered-history substitute.
8. **No legacy constraints.** The Python prototype is behavioral evidence only.
   V1 has no schema, command, state, or compatibility obligation to it.

## V1 scope

### Included

- Codex-first local and SSH-host operation.
- A minimal two-pane terminal experience: navigator beside the directly
  interactive native Codex TUI.
- Explicit host registration and capability checks.
- Logical projects with one or more pre-registered host locations.
- Workstream creation, switching, parking, exact resume, and display through
  the current tip's Codex-owned thread name.
- Independent workstreams created from a project location's configured default
  Git base.
- Conversation-forked workstreams whose filesystem also starts independently
  from that configured default base.
- One external checkout for an initial workstream and conservatively owned
  managed worktrees for additional workstreams.
- Activity and durable result attention for Workstream Navigator-started Codex
  sessions.
- Reconnection after local UI or SSH loss.
- Recovery after the host tmux runtime disappears, using the provider's native
  session identity.
- Direct CLI equivalents for the TUI's important actions.
- One input owner per provider runtime, with explicit takeover rather than
  shared or ambiguous terminal input.

### Explicitly outside V1

- Importing or controlling arbitrary existing Codex sessions or worktrees.
- A persisted `Task` entity, assignments, priorities, plans, schedules, queues,
  dependencies, or task-context transfer.
- Automatic plan detection, plan acceptance inference, prompt interception, or
  automatic thread rollover.
- A replacement implementation or altered semantics for Codex `/new`,
  `/clear`, `/fork`, `/rename`, Plan mode, history, settings, permissions, or
  model selection. Navigator Rename is a thin call to the same Codex-owned name
  field, not a separate naming system.
- Transcript storage, transcript rendering, history search, or project memory.
- A custom PTY server, terminal emulator, browser UI, desktop UI, or mobile UI.
- A public network service or always-running remote daemon.
- Cloning repositories, synchronizing checkouts, moving a live workstream
  between hosts, or transferring chats between hosts.
- Automatic Git fetch, pull, commit, merge, rebase, reset, stash, push,
  cherry-pick, or conflict resolution.
- Copying uncommitted files or source-only commits into a forked workstream.
- Claude or broad provider parity.
- Multiple-controller catalog synchronization.

Cross-host operation in V1 means that one navigator can see, start, attach to,
and resume work at pre-registered project locations on several hosts. It does
not mean that one workstream migrates between them.

## Concepts and ownership

| Concept | Meaning | Canonical owner |
| --- | --- | --- |
| `Host` | A local or SSH-reachable machine with `wsnav`, tmux, Git, and Codex capabilities | Workstream Navigator client catalog plus host handshake |
| `Project` | A logical repository-shaped grouping shown by the navigator | Workstream Navigator |
| `ProjectLocation` | One registered Git repository and worktree root on one host | That host's Workstream Navigator registry |
| `Workstream` | One independent checkout, runtime lane, and current provider-session binding | That host's Workstream Navigator registry |
| `Checkout` | An external checkout or a Workstream Navigator-created Git worktree | Git plus the host registry's ownership record |
| `Runtime` | One provider process in one private tmux server, session, window, and pane | tmux and live process evidence |
| `ProviderSession` | A Codex chat/session referenced by its native identifier | Codex |
| `ConversationTip` | The current native thread plus its latest accepted settled turn | Workstream Navigator binding plus Codex identities |
| `ThreadName` | The current tip's user-facing name, changed through native `/rename` or App Server `thread/name/set` | Codex |
| `Attention` | A durable indication that background work completed or needs recovery | Workstream Navigator |
| `AttachmentLease` | The single client currently allowed to send terminal input to a runtime | That host's Workstream Navigator registry plus live tmux evidence |

V1 deliberately has no `Task` record. Tasks remain what the user asks Codex to
do inside a provider session. A workstream may carry many successive tasks and
many native chats over time without becoming a task manager.

The Workstream ID is stable; its ConversationTip moves. A native `/new`,
`/clear`, or managed cutover may replace thread A with thread B without
replacing the Workstream. A Workstream fork creates a new Workstream ID,
Checkout, Runtime, and ConversationTip while retaining explicit ancestry.

There is no separate Workstream label in V1. The current tip's native
`thread.name` is the canonical display name and exact resume still relies on
the native thread ID. The navigator may cache the last observed name for
availability, but it never creates a second naming authority.

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
                                                       └── native Codex TUI

wsnav TUI
├── local client catalog
├── local host adapter
└── SSH host adapters
    └── fixed, versioned JSON protocol over SSH stdin/stdout

each managed host
├── private SQLite state
├── one private tmux server per live workstream runtime
│   └── exactly one session, window, and provider pane
├── short-lived wsnav action/snapshot/watch commands
├── per-operation Codex App Server stdio helpers
└── Codex observer hooks active only in wsnav-started sessions
```

### Presentation layer

The local presentation session is a dedicated tmux server with its own socket
and configuration. It never modifies or depends on the user's ordinary tmux
server.

The navigator is a small Rust TUI in one pane. The provider pane is not a
terminal widget rendered by Rust; it is a real tmux attachment to the host-owned
provider runtime. This retains direct keyboard, mouse, resize, color, and native
TUI behavior without building a PTY server or terminal emulator.

The dedicated tmux status line stays disabled because it consumes a row from
the provider surface. Navigation and status live in the navigator pane.

Switching workstreams replaces only the provider pane's attachment helper. It
does not stop, restart, type into, or resize an inactive provider process beyond
the normal detach/attach terminal negotiation.

Focus is local presentation state, not durable Workstream state. Two navigator
clients may look at different workstreams without racing over a global
`current` record. Durable state records activity and attention, never an
authoritative focused pane.

A provider Runtime has at most one input-enabled attachment. The host grants a
short-lived AttachmentLease to the attachment helper and corroborates it with
the live dedicated tmux client. A second navigator may inspect metadata but
cannot attach for input unless the first attachment is gone or the user
explicitly takes over. A takeover detaches the previous presentation client; it
does not send provider input or stop the provider process.

### Host runtime layer

Every managed host owns:

- one private state root;
- one stable host identity;
- zero or more runtime-private tmux sockets and server generations, one for
  each live Runtime;
- the workstream, checkout, operation, binding, and attention records for work
  physically running on that host.

tmux owns live process persistence. SQLite owns metadata and recoverable
operation state. Codex owns session history.

Each live Runtime is a bounded tmux unit:

```text
Runtime -> one private socket and server -> one session -> one window -> one pane
```

No private runtime server contains a sibling Workstream. Parking, stopping, or
retiring a Runtime removes its server rather than leaving an empty session.
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

There is no remote daemon in V1. Remote requests launch short-lived
`wsnav _remote` commands through SSH. A connected navigator may keep one
read-only `watch` command open per host; that process exits with the client and
does not own provider runtimes or mutation authority.

All mutation commands use host-local SQLite transactions, optimistic revisions,
and idempotency keys. Concurrent hooks and clients may race, but only one
transaction can commit a particular record revision.

### Host transport

Local and SSH hosts implement one internal interface:

```text
hello() -> protocol, host identity, versions, capabilities
snapshot() -> projects, workstreams, runtime probes, attention
watch(revision) -> bounded state changes
apply(operation, expected revisions) -> deterministic outcome
attach(runtime_id) -> native terminal attachment
```

The SSH command is fixed and machine-oriented. Request bodies travel as bounded
JSON on stdin; stdout contains only versioned protocol frames and stderr
contains bounded diagnostics. Thread names, repository paths, prompts, and shell
fragments are never interpolated into an SSH command string.

The remote binary validates protocol compatibility before reading or mutating
state. An incompatible host is visible but unavailable for actions. V1 requires
the user to install `wsnav` on each host and register a fixed executable path.
It diagnoses missing or incompatible binaries but does not copy, bootstrap, or
update remote executables.

### Codex adapter

Production sessions use the user's normal Codex home, authentication,
configuration, plugins, skills, models, permissions, and native history.
Temporary Codex homes remain test-only.

Every live Workstream runs one dedicated native `codex` or
`codex resume <thread-id>` process in its own host tmux session. The TUI owns
that process's runtime for its entire lifetime. Workstream Navigator never
launches a managed TUI with `codex --remote`.

Workstream Navigator also never starts a persistent App Server listener. A
Unix, WebSocket, or other shared listener plus one or more `codex --remote`
clients changes the runtime into a client/shared-server topology. That
contradicts the Workstream isolation boundary even if the provider surface
still looks native.

Workstream Navigator adds one narrowly scoped Codex profile for sessions that
it launches. The profile layers on top of normal user configuration and adds
observation-only lifecycle hooks. It is activated explicitly on managed
launches and is inactive for ordinary Codex sessions.

The profile and hook definition must be owned, versioned, collision-checked,
reviewable, and removable. Workstream Navigator never overwrites a profile it
does not own and never installs a catch-all global hook.

Existing user-configured Codex hooks remain the user's integrations. Workstream
Navigator neither disables nor rewrites them, and cannot guarantee that an
unrelated failing hook will preserve the native UI. `doctor` reports detected
overlap or failures when Codex exposes enough information, without silently
mutating the user's configuration.

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
- its environment, ancestry, session, runtime generation, cwd, and binding
  revision are checked before an observation is accepted.

Hook evidence can update status and bind an observed native session inside an
already managed runtime. It cannot authorize workstream creation, fork,
retirement, provider input, Git mutation, or focus.

A ProviderBinding is stronger than an untrusted hook claim. Initial and changed
bindings are accepted only when the event agrees with a pending launch or
native transition, the recorded runtime generation, pane, cwd, process birth
and ancestry, and provider-side session existence. Events that cannot be
corroborated may make status `unknown`, but cannot replace a known binding.
Whether the installed Codex hook contract can provide this distinction is a
falsification gate. If it cannot distinguish a legitimate transition from an
agent-shell invocation, V1 must require an explicit native resume/fork
selection and observe the resulting launch; it must not weaken the authority
rule.

#### Ephemeral App Server adapter

Persisted thread metadata and bounded thread-store mutations use a separate,
per-operation App Server process on the host that owns the Codex state:

```text
wsnav host action
-> spawn codex app-server --listen stdio://
-> initialize one private stdin/stdout connection
-> issue one or more bounded requests
-> close stdin and wait
-> terminate, then kill, on bounded shutdown failure
```

No TUI connects to this process. It does not host interactive work, listen on a
socket, remain alive between operations, or become activity authority for a
dedicated TUI. A remote host filters App Server responses before writing the
Workstream Navigator protocol to SSH stdout.

V1 allowlists only:

- `thread/read` with `includeTurns: false` for exact managed thread IDs;
- `thread/list` with `sourceKinds: ["cli"]` and `useStateDbOnly: true` only for
  bounded recovery and `doctor` operations;
- `thread/name/set` for an explicit Rename action or a provisional fork name
  set before its destination TUI starts; and
- `thread/fork` with an exact accepted `lastTurnId` and destination `cwd` for
  an explicit Fork Workstream operation.

V1 does not call App Server turn start, steer, interrupt, item injection,
runtime configuration, shell, approval, or provider-input methods. App Server
runtime `status` is scoped to that short-lived process and is never treated as
the status of a separately running native TUI.

`thread/read` and setting an already-requested name are safely repeatable.
`thread/fork` is not assumed idempotent: if the helper exits after Codex may
have created a destination but before returning its ID, Workstream Navigator
must reconcile exact provider lineage and recorded operation evidence. It must
not retry and risk a duplicate destination while the effect remains ambiguous.

The host extracts only approved fields from responses. It never returns or
persists `preview`, turns, items, transcript paths, or the raw response.
`thread.preview` is prompt-derived and therefore is not a naming fallback.

Codex's native CLI and ephemeral App Server divide the action boundary:

- fresh work uses `codex`;
- recovery uses `codex resume <session-id>`;
- a Workstream fork uses App Server `thread/fork`, then starts the resulting
  thread through `codex resume <destination-id>`; and
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
| New Workstream before thread binding | `starting · <workstream-short-id>` |
| New or existing Workstream with a known-empty name | `untitled · <workstream-short-id>` |
| Same-Workstream cutover from named A to unnamed B | `<A name> ↻ unnamed` |
| Same-Workstream cutover when A was also unnamed | `untitled · <same-workstream-short-id> ↻` |
| Fork to a new Workstream from a named source | `<source name> · fork · <destination-short-id>` |
| Fork from an unnamed source | `fork of <source-workstream-short-id> · <destination-short-id>` |
| Metadata refresh unavailable with a current-tip cache | Last cached native name with a stale or unreachable indicator |
| Metadata refresh unavailable without a current-tip cache | The contextual transition display with `name unavailable`; otherwise `name unavailable · <workstream-short-id>` |
| Provider thread missing during recovery | Last cached native name with `recovery required`; otherwise `recovery required · <workstream-short-id>` |

Resolution prefers a current non-empty native name, then a current-binding
cache when refresh is unavailable, then transition context, and finally a
synthetic lifecycle fallback. An unavailable observation never becomes
`unnamed` or `untitled`; those displays require `known_empty`. Every final
fallback uses the stable Workstream short ID, never the moving thread UUID.
Branch, worktree, host, and cwd remain secondary context rather than naming
authority.

An exact thread ID, not any displayed text, remains identity and action
authority. Names and computed fallbacks need not be unique.

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

V1 uses fresh SQLite schemas with no migration from Agent Switchboard.

### Client catalog

The local client catalog contains only:

- configured host aliases and stable host IDs;
- logical project names and client-generated opaque IDs;
- mappings from a Project to registered ProjectLocations; and
- local UI preferences.

The client catalog is not authority for a remote runtime, worktree, provider
binding, or mutation. Losing it does not stop remote work. Reconstructing a
catalog from host registries may be added later.

Project identity is explicit, not inferred from a repository URL, directory
name, branch, or Git remote. Registering another host location against the same
client-generated Project ID groups it in the navigator. Repository identity is
still recorded at each location to prevent accidental cross-repository
operations. If the client catalog is lost, host-local Workstreams remain
usable, but the cross-host grouping must be registered again in V1.

### Host registry

The host registry contains:

```text
HostIdentity
  host_id, registry_generation, schema_version

ProjectLocation
  location_id, project_id, repository_identity, repository_path,
  default_base_ref, managed_worktree_root, revision

Workstream
  workstream_id, location_id, origin,
  source_workstream_id?, checkout_id, lifecycle, revision

Checkout
  checkout_id, path, ownership, branch?, creation_commit?,
  repository_identity, revision

Runtime
  runtime_id, workstream_id, provider, tmux_generation,
  tmux_session, cwd, process_birth, lifecycle, revision

ProviderBinding
  binding_id, runtime_id, native_session_id, start_source,
  last_settled_turn_id?, observed_thread_name?, name_state,
  name_observed_at?,
  previous_binding_id?, runtime_generation, revision

Attention
  attention_id, workstream_id, native_session_id, turn_id,
  kind, observed_revision, acknowledged_at?

AttachmentLease
  lease_id, runtime_id, client_id, tmux_client_fingerprint,
  acquired_at, heartbeat_at, revision

Operation
  operation_id, idempotency_key, kind, phase, expected_revisions,
  effect_watermark, outcome
```

Paths and provider identifiers are private host fields. Public snapshots return
bounded thread names, name provenance, statuses, capabilities, and opaque
Workstream Navigator IDs. No prompt, preview, response, transcript, tool
payload, terminal capture, credential, or environment dump is persisted.

### State relationships

- One open Workstream owns exactly one Checkout.
- One managed Checkout has exactly one open Workstream owner.
- One Workstream has at most one input-enabled Runtime.
- One Runtime has at most one live input-enabled AttachmentLease.
- One Runtime has one current ProviderBinding and may retain prior binding IDs
  for exact lineage and recovery.
- The current ProviderBinding plus its accepted `last_settled_turn_id` is the
  Workstream's ConversationTip.
- `observed_thread_name` is a cache of Codex-owned metadata, not a second
  naming authority.
- `name_state=unavailable` retains a prior cached name; an unavailable refresh
  never becomes evidence that the provider name is empty.
- `EffectiveNameSource` is derived presentation state and is not persisted as a
  user-authored name.
- Many native Codex sessions may appear sequentially inside one Workstream as
  the user uses native `/new`, `/clear`, or `/fork`.
- Attention never changes presentation focus.
- Runtime status and Workstream lifecycle are separate.

Every accepted settled turn creates durable Attention independent of focus.
The presentation may render the currently attached Workstream less urgently,
but it never uses focus to decide whether completion was persisted. Attention
is cleared only by an explicit acknowledge action, so a UI crash or focus race
cannot lose a completed result.

Suggested Workstream lifecycle values:

```text
open | parked | recovery_required | retiring | retired
```

Suggested observed Runtime status values:

```text
starting | idle | working | attention | stopped | unknown | unreachable
```

`unreachable` is a transport observation, not proof that a runtime stopped.

## Git and worktree policy

A ProjectLocation references one local, non-bare Git repository with a stable
common-directory identity and a configured `default_base_ref`.

The first Workstream may use an existing checkout with `external` ownership.
Additional V1 Workstreams use `managed` worktrees below the configured
Workstream Navigator root.

Before creating an independent or forked Workstream, the host resolves
`default_base_ref` to one exact locally available commit. It does not fetch.
The operation records that commit before creating the branch or worktree.

Conversation and filesystem lineage are deliberately separate:

```text
provider lineage:   source Codex session -> forked Codex session
filesystem lineage: project default-base commit -> new managed worktree
```

A fork does not copy source-only commits, staged files, unstaged files,
untracked files, ignored files, build output, processes, or credentials. The
navigator must make that distinction visible.

Workstream Navigator never stashes or force-removes. Managed worktree removal
requires exact ownership, matching repository identity, no active runtime, a
clean checkout, and a separately approved retirement rule. External checkouts
are never removed.

The precise branch naming and merged-state retirement rule remain design gates
before worktree implementation.

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
host creates a fresh dedicated tmux session in the recorded checkout
-> launches native codex resume with the exact session ID
-> SessionStart(source=resume) confirms the binding
-> navigator attaches the provider pane
```

### Start an independent Workstream

```text
user selects ProjectLocation and Start Workstream
-> host resolves the exact default-base commit
-> operation reserves Workstream, Checkout, and Runtime IDs
-> host creates the managed branch and worktree
-> host launches a blank native Codex TUI in dedicated tmux
-> SessionStart confirms the native session
-> navigator focuses the new Workstream
-> user enters the first prompt in Codex's native composer
```

No workstream name, model, branch, session ID, or first prompt is required in a
manager-owned creation form. Before binding, the row shows
`starting · <workstream-short-id>`; a bound but unnamed tip shows
`untitled · <workstream-short-id>`. Later native `/rename`, navigator Rename,
or an opt-in Codex naming skill updates the one Codex-owned thread name.

### Fork a running Workstream

The action means “explore another approach from the latest settled conversation
state.” It does not fork partial model output or current filesystem state.

```text
source Codex turn may still be running
-> user explicitly selects Fork Workstream
-> host validates the source binding and last settled provider boundary
-> host resolves the ProjectLocation default base to an exact commit
-> operation creates an independent managed worktree
-> ephemeral App Server forks source through exact lastTurnId with destination cwd
-> if source has a native name, host sets a bounded provisional fork name
-> host launches native codex resume for the returned destination thread ID
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
- `/new` or `/clear` for a fresh chat in the same Workstream;
- `/fork` for a native chat fork that remains in the same Workstream unless the
  user explicitly creates a separate Workstream; and
- native Plan choices, including current-thread implementation or clear-context
  implementation.

Workstream Navigator observes a new session binding when possible. It does not
infer that a native chat transition created a new task or Workstream. A
verified same-Workstream cutover displays the prior effective name
provisionally when the new thread is unnamed, but does not write that fallback
into Codex.

## Navigator experience

The default view is intentionally small:

```text
Host
└── Project
    ├── Tip thread name         working
    ├── Prior name ↻ unnamed    working
    ├── Source · fork · b72c    result ready
    └── untitled · a91f         parked

┌ navigator ┐┌──────────────── native Codex TUI ────────────────┐
│ tree      ││ directly interactive; no manager-owned chrome   │
│ status    ││ inside the provider surface                     │
└───────────┘└──────────────────────────────────────────────────┘
```

Required interactions:

- keyboard and mouse selection in the navigator;
- direct keyboard and mouse interaction in the provider pane;
- one action to focus or reconnect a Workstream;
- Start Workstream with project defaults;
- Fork Workstream from an exact managed source;
- rename the current tip through Codex's canonical thread-name field;
- park/resume without deleting provider history; and
- acknowledge result or recovery attention without injecting provider traffic.

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
| A second client requests attachment | Refuse shared input; offer explicit takeover after revalidating the existing lease and tmux client |
| Navigator crashes during focus switch | Focus is ephemeral; no durable runtime or Workstream mutation is implied |
| Client disconnects during compound action | Reopen the idempotent Operation and reconcile recorded external effects |
| Git worktree creation is partial | Record `recovery_required`; never guess ownership or delete uncertain paths |
| Managed checkout is dirty | Block removal; never stash, reset, or force-remove |
| Host protocol versions differ | Reject mutation and show an actionable compatibility diagnostic |

Result completion and attention creation must commit in one host transaction.
This directly avoids the Python prototype's split result/attention persistence
gap.

## Security and privacy

- State roots are user-private; directories use mode `0700` and files use
  `0600`.
- Every live Runtime owns a private tmux socket and server with exactly one
  session, window, and pane; these sockets never reuse the user's ordinary
  socket.
- Management commands use `env -u TMUX tmux -S <absolute-runtime-socket>` and
  never bare `tmux` or `tmux -L`. A native provider retains the private `TMUX`
  environment by default, so a bare `tmux ls` inside it sees at most that one
  Runtime; removing `TMUX` remains a terminal-acceptance experiment.
- SSH relies on the user's existing host authentication and `known_hosts`;
  Workstream Navigator opens no listener.
- Managed Codex TUIs never use `codex --remote`, and Workstream Navigator never
  starts a persistent Codex App Server transport.
- Ephemeral App Server helpers use private stdio, a distinct process group,
  bounded request and shutdown deadlines, and forced cleanup when graceful
  shutdown fails.
- Remote commands disable forwarding and use bounded fixed protocol entrypoints.
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
- Every destructive Git action revalidates exact recorded ownership immediately
  before the effect.

## Proposed Rust structure

```text
src/
├── main.rs               CLI entrypoint
├── app.rs                top-level command orchestration
├── domain/               pure IDs, entities, statuses, invariants
├── state/                SQLite schema, transactions, revisions, operations
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
│       ├── app_server.rs ephemeral stdio metadata/name/fork client
│       └── hooks.rs      passive lifecycle event handling
├── git/
│   ├── repository.rs     repository identity and base resolution
│   └── worktree.rs       create, verify, and guarded retirement
├── tui/                  minimal navigator state, rendering, input, mouse
└── internal/             hidden remote, hook, and watch entrypoints
```

The provider interface should remain small and capability-based. V1 has one
real implementation. No speculative Claude abstractions or generic
lowest-common-denominator behavior should shape the Codex implementation.

## Validation gates before production implementation

The existing spikes validate transport, native presentation, and the shell-only
per-Runtime tmux topology. The following contracts still need isolated proof:

1. **Terminal acceptance:** real mouse interaction, truecolor/box drawing,
   resizing, focus changes, reconnect through the final two-pane layout, and
   the final native-Codex `TMUX` environment choice.
2. **Runtime isolation and ephemeral metadata:** every managed TUI remains a
   dedicated process-owned runtime with one private tmux server/session/window/
   pane; stdio helpers can read persisted metadata and exit without changing
   it, while persistent App Server transports and `codex --remote` are
   rejected.
3. **Scoped Codex profile:** a managed profile can add passive hooks without
   changing unmanaged Codex sessions, and install/remove/trust behavior is
   deterministic.
4. **Hook robustness:** large payloads, malformed input, missing authority,
   stale generations, event races, and unavailable state never produce
   broken-pipe or provider-facing hook errors.
5. **Hook authority and status transaction:** forged agent-shell events cannot
   rotate a ProviderBinding; legitimate `UserPromptSubmit`, `Stop`, result
   attention, native `/new`, `/clear`, `/rename`, and resume yield conservative
   navigator state without storing prompt or transcript content.
6. **Thread-name lifecycle:** exact managed threads expose nullable names
   through ephemeral `thread/read`; native and navigator rename converge,
   context-specific fallbacks distinguish new, cutover, fork, and unavailable
   states; native cutovers never overwrite a concurrent rename; remote
   filtering removes previews; and failed refresh does not disturb the TUI.
7. **Cold recovery:** loss of an exact private runtime tmux server followed by
   `codex resume <session-id>` restores the same native history in the recorded
   checkout and creates one new runtime generation.
8. **Running-source fork:** ephemeral App Server `thread/fork` with the exact
   accepted `lastTurnId` and destination `cwd` creates one persisted
   destination; native resume opens it while the source's active turn and
   dedicated process remain unchanged.
9. **Worktree ownership:** independent and forked Workstreams resolve one exact
   default-base commit, create collision-free managed worktrees, and refuse
   unsafe retirement.
10. **Multi-host protocol:** local and SSH adapters return the same semantic
   results, reject version mismatch, survive disconnect, enforce one input
   attachment, and never mutate an ordinary tmux server.
11. **Combined acceptance:** start local work, start remote work while it runs,
   switch between both, fork one, observe background completion without focus
   theft, reconnect, resume after runtime loss, and preserve every provider
   result tip.

Passing fixtures contain only provider/version fingerprints, assertion
booleans, event relationships, timings, and cleanup proof. Assisted diagnostics
cannot become passing fixtures.

## Proposed delivery checkpoints

### D0 — Contract kernel

- Domain types, IDs, statuses, invariants, and errors.
- Fresh SQLite state with migrations only from V1 development schemas.
- Operation idempotency and failure-injection tests.
- Versioned host protocol types.

### D1 — Local Codex runtime

- One private tmux server, session, window, and pane per live local Runtime.
- Ephemeral App Server client for exact thread-name reads and writes.
- Scoped Codex profile and observer hook.
- Local project location, external initial checkout, start, attach, status,
  tip naming, attention, park, and exact resume.
- No TUI requirement yet; CLI acceptance first.

### D2 — Minimal navigator

- Dedicated local presentation tmux session.
- Ratatui navigator pane plus directly interactive provider pane.
- Keyboard/mouse selection, focus, switching, and attention.
- Terminal acceptance gate.

### D3 — SSH hosts

- Host registration, handshake, snapshot/watch/apply/attach.
- Remote start, attach, reconnect, status, and cold resume.
- Strict protocol and capability diagnostics.

### D4 — Workstreams and forks

- Managed worktree creation from exact default base.
- Independent Workstream action.
- Exact-turn App Server conversation fork into a separately based worktree,
  followed by native TUI resume.
- Guarded retirement policy.

### D5 — Recovery and V1 acceptance

- Crash/failure reconciliation for every compound action.
- Install, doctor, uninstall, and residue checks.
- Combined local/remote workflow acceptance.
- UX polish after behavior is complete.

Each checkpoint should be reviewable, committed, and accepted separately. No
checkpoint should install hooks, adopt existing sessions, or mutate ordinary
tmux/provider state during automated tests.

## Decisions still requiring review

The first pass deliberately settles these potentially expansive questions:

- project grouping uses explicit opaque IDs and registration, not repository
  heuristics;
- remote hosts require a preinstalled compatible binary at a registered path;
  V1 has no deployment system; and
- simultaneous provider input is unsupported; one live AttachmentLease or an
  explicit takeover is required;
- Workstream display names come from the current Codex tip thread rather than a
  shadow Workstream label, with context-specific computed fallbacks ending in
  the stable Workstream short ID; and
- live TUIs use dedicated process-owned runtimes while App Server access is
  short-lived stdio only; each Runtime has its own bounded private tmux server.

The remaining pre-implementation decisions are:

1. Codex profile naming, ownership marker, trust review, and removal UX.
2. The private tmux terminal configuration and final native-Codex `TMUX`
   environment needed for correct Unicode, truecolor, extended keys, mouse,
   images, and clipboard forwarding on supported hosts.
3. Managed branch naming and the exact clean/merged retirement rule.

These are bounded design questions. They do not reopen the product boundary,
provider-native workflow decision, tmux/SSH substrate, or no-transcript rule.

## Evidence basis

- [Spike 0001: tmux remote-session transport](spikes/0001-tmux-remote-transport.md)
- [Spike 0002: native Codex TUI over remote tmux](spikes/0002-codex-native-tui.md)
- [Spike 0004: per-Workstream tmux runtime isolation](spikes/0004-tmux-runtime-isolation.md)
- [Study 0003: Codex App Server runtime boundary](studies/0003-codex-app-server-runtime-boundary.md)
- [Current Codex CLI commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
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
treats installed behavioral spikes as the final capability authority.
