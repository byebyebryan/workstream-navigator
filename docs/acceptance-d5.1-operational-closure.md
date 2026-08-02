# D5.1 Operational Closure Acceptance

Date: 2026-07-30

Status: pass — automated and disposable operational gates passed without
touching ordinary Codex state or the user's tmux server.

## Evidence

- A client loss during independent Start leaves its durable Workstream row for
  normal open/recovery. An unresolved Fork is available through the local, SSH
  protocol, and navigator projections as an opaque operation ID. Exact Fork
  reconciliation reads the durable plan without its original request key. State
  and action tests prove
  that a recorded fork-attempt marker permits reconciliation only, never a
  second `thread/fork` request.
- The hidden `_probe` endpoint writes only release version, control ABI,
  protocol version, and host-schema version. The disposable script passes a
  temporary state-root argument and confirms that the probe creates no state.
  Local-subprocess protocol tests require that probe before a normal handshake.
- The shared bounded child-process runner drains stdout and stderr while
  retaining only caller caps. Runtime tmux, presentation tmux, Git, navigator
  child actions, and checkout inspection use it; the acceptance script rejects
  any remaining direct `Command::output` call under `src/`.
- New Runtime records use their complete UUID in both the private tmux path and
  session name. Existing persisted short-session records resolve only to their
  exact former path; any other session value is rejected before a tmux action.
- The disposable native-recovery harness verifies a full UUID runtime
  directory/session pair before stopping only that server. The ordinary tmux
  fingerprint remains unchanged before and after cleanup.
- The empty navigator and README require `wsnav register
  /path/to/git-checkout`; setup remains a separate host-level native trust
  action and no current-working-directory heuristic was added.
- D5.1 initially declared Rust 1.85 and configured a pinned CI job, but the
  later post-checkpoint review found that the locked Ratatui graph required
  Rust 1.88. That contradiction reopened V1 closure and is superseded by the
  passing Rust 1.88 evidence in
  [D5.2](acceptance-d5.2-correctness-closure.md).

`scripts/check` runs the new disposable probe/output gate, existing fake-Codex
runtime recovery, fresh package install, all tests, Cargo Deny, shell lint,
fixture validation, package verification, and diff checks.

The [sanitized D5.1 fixture](../spikes/fixtures/d5.1-operational-closure.json)
contains only fixed capability assertions and privacy/isolation results. It
contains no provider identifiers, user paths, prompts, results, terminal data,
process IDs, credentials, or raw payloads.
