# Study 0003: Codex App Server Runtime Boundary

Date: 2026-07-28

Status: validated by isolated live Spikes 0007 and 0008

## Question

Can Workstream Navigator use Codex App Server metadata and thread operations
without converting dedicated native TUIs into clients of a shared runtime?

## Answer

Yes, if App Server is a short-lived stdio helper that no TUI connects to.

A persistent Unix or WebSocket App Server plus `codex --remote` is a different
runtime topology: multiple TUI clients may depend on one shared server-owned
runtime. Workstream Navigator rejects that topology. Every managed Workstream
must retain one dedicated native Codex process in its host tmux session.

The permitted auxiliary topology is:

```text
dedicated live runtime
tmux -> native codex TUI process

bounded metadata operation
wsnav host command -> codex app-server --listen stdio:// -> request -> exit
```

The helper reads or mutates the persisted Codex thread store by exact ID. It
does not attach to, host, redirect, resume, or report live status for the
separate TUI process.

## Existing precedent

`dms-agent-picker` at commit `c999d16` and version `0.5.0` already implements
this split:

- `AppServerClient` starts a distinct process with private stdin, stdout, and
  stderr, performs the initialize handshake, and enforces bounded shutdown.
- Local queries start `codex app-server --stdio`; remote queries start the same
  helper over SSH. Workstream Navigator should use the current explicit spelling
  `--listen stdio://`.
- saved Codex sessions come from `thread/list` with
  `sourceKinds: ["cli"]`, `useStateDbOnly: true`, and no archived sessions;
- exact session reads use `thread/read` with `includeTurns: false`;
- persistent socket or WebSocket App Servers are classified as shared and
  omitted from dedicated-TUI activity discovery; and
- active status comes independently from process ancestry, open rollout file
  descriptors, tmux panes, and tmux metadata.

The picker test suite passed all 36 tests during this study, including the
shared-versus-stdio classification and shared-runtime exclusion tests.

Workstream Navigator should reuse the lifecycle pattern, not the picker's
broad discovery scope. V1 knows which Workstreams it started and should query
only their exact thread IDs during normal operation.

## Thread names

The installed Codex CLI `0.145.0` schema exposes:

- nullable `thread.name` as the optional user-facing title on `thread/read` and
  `thread/list`;
- `thread/name/set` with exact `threadId` and `name`; and
- `thread/name/updated` notifications within an App Server connection.

The existing local `rename-thread` skill uses the same short-lived stdio
pattern: it obtains the current `CODEX_THREAD_ID`, calls `thread/name/set`, and
verifies the persisted value with `thread/read`.

A read-only live check of the current thread returned a null `name` and a
non-empty, prompt-derived `preview`. No raw identifier, preview, prompt, or
response is recorded here. This establishes two design rules:

> **Superseded display examples.** This study's provenance and App Server
> findings remain valid, but the current V1 design no longer exposes stable
> Workstream short IDs in user-facing fallback names. See
> [`docs/design.md`](../design.md#workstream-display-names) for the accepted
> presentation contract.

1. the current tip's non-empty `thread.name` is the canonical Workstream display
   name; and
2. `thread.preview` is never transported, persisted, logged, or used as a
   fallback.

Missing and unavailable names require different treatment:

```text
NameState
  named | known_empty | unavailable
```

- A new bound Workstream with a known-empty name displays
  `untitled · <workstream-short-id>`.
- A same-Workstream cutover displays `<previous name> ↻ unnamed`, or retains
  the stable unnamed Workstream fallback when the previous tip was unnamed.
- A new fork displays
  `<source name> · fork · <destination-short-id>`, or
  `fork of <source-short-id> · <destination-short-id>` when the source was
  unnamed.
- An unavailable refresh retains the last cached native name with stale or
  unreachable provenance; it does not turn into `known_empty`.
- If an unavailable current tip has no cached name, its transition context is
  retained with `name unavailable`, or it displays
  `name unavailable · <workstream-short-id>`; it never displays `untitled`.
- The ultimate fallback uses the stable Workstream short ID, not the moving
  thread UUID. Branch, worktree, host, and cwd remain secondary context.

Native `/rename` and navigator Rename both update the same Codex-owned field.
A bounded refresh observes native changes. Workstream Navigator may cache the
last approved `{thread_id, name}` pair, but that cache is not naming authority.

App Server `thread/name/set` has no compare-and-set field. Workstream Navigator
therefore must not copy A's name into B after a native cutover: a read-then-set
sequence could overwrite a faster user or skill rename. The previous name is a
computed, visibly provisional fallback only.

After a Workstream Navigator-controlled `thread/fork`, setting a provisional
name derived from a non-empty source name is safe before the destination TUI
starts. If the source has no native name, the destination remains unnamed and
uses a computed fallback.

## Exact settled-tip forks

The installed App Server schema exposes `thread/fork` with:

- `threadId` for the source;
- optional `lastTurnId`, copied inclusively while omitting later turns; and
- optional `cwd` for the destination.

The current documented contract rejects an in-progress `lastTurnId`. Omitting
`lastTurnId` while the source is mid-turn can add an interruption marker, so
Workstream Navigator must always pass its accepted last-settled turn ID.

The proposed Fork Workstream operation is:

```text
record exact source thread and accepted last settled turn
-> create destination worktree from configured default base
-> ephemeral thread/fork(source, lastTurnId, destination cwd)
-> optionally set a bounded provisional destination name
-> close the helper
-> start native codex -C <destination-worktree> resume <destination-id> in dedicated tmux
```

Spike 0008 proved the destination contains exactly the settled prefix, the
source's active turn and TUI process remain unchanged, the destination survives
helper exit, and native resume executes commands in the destination worktree.

One installed behavior narrows the request contract: Codex 0.145.0 did not
persist the requested fork cwd before native resume. Workstream Navigator still
passes `cwd`, but native `-C` is authoritative for destination execution and
pre-resume recovery cannot require a matching stored cwd.

The generated schema does not expose an idempotency key for `thread/fork`.
Losing the response after Codex persisted a destination is therefore an
ambiguous external effect. Spike 0008 left a successful response unread,
submitted no retry, and recovered one destination from exact source lineage,
settled boundary, and operation timing. The fork was absent from a CLI-only
source-kind query, so this one unresolved operation queries all documented
source kinds before applying those exact filters. Zero or multiple candidates
remain `recovery_required`. This bounded `thread/list` use is not a normal
discovery or onboarding path.

## Workstream Navigator adapter contract

The Rust adapter must:

- spawn on the host that owns the Codex home;
- use private stdio and no listener;
- identify itself during `initialize`;
- allowlist exact methods and parameter shapes;
- batch bounded reads inside one helper when useful;
- filter returned fields before remote protocol output;
- discard `preview`, turns, items, paths, raw responses, and App Server runtime
  status;
- close stdin after the final response;
- wait for a short grace period, then terminate and kill the process group if
  necessary; and
- report metadata failure without changing or stopping any dedicated TUI.

Allowed V1 methods:

```text
thread/read
thread/list       recovery and doctor only
thread/name/set   explicit rename or pre-TUI provisional fork name
thread/fork       explicit Workstream fork only
```

App Server turn, input, interrupt, shell, approval, item-injection, and runtime
configuration methods are outside V1.

## Validation result

Spike 0007 proved short-lived read and name-set stability beside a native TUI,
native `/rename` visibility, response filtering, the full fallback matrix, the
absence of compare-and-set, and rejection of persistent App Server and managed
`codex --remote` topologies.

Spike 0008 proved the exact settled-prefix fork beside a running source,
single-submission lost-response reconciliation, native destination resume,
default-base worktree execution, and source/destination divergence. It also
confirmed that App Server's persisted view of the active source turn is not
live status authority.

## Privacy and cleanup

The studies committed no thread UUID, prompt, preview, transcript, path, PID,
credential, environment, or raw App Server response. Generated schema
directories, disposable threads/worktrees, short-lived App Server processes,
and private tmux servers were removed. No ordinary Codex thread, repository,
TUI, or tmux server was modified.

## Sources

- [Codex App Server](https://learn.chatgpt.com/docs/app-server)
- [Codex CLI commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- [DMS Agent Picker](https://github.com/byebyebryan/dms-agent-picker)
