# D17 shell-first managed-session onboarding acceptance

Date: 2026-08-28

Status: complete. OpenCode and Codex shell promotion, managed-card handoff,
fresh-Shell derivation, Park, observer cleanup, and complete disposable cleanup
passed with explicit operator intent.

## Operator and privacy boundary

The run uses an empty disposable Git repository, schema-14 WSNav state root,
provider homes, and an outer private tmux server. Existing authentication is
copied at mode `0600` only into the disposable Codex home and is never printed,
hashed, parsed, or committed. The outer harness changes its own prefix so
literal `Ctrl-b` reaches the WSNav presentation; neither layer opens or mutates
the default tmux server.

Evidence is limited to versions and executable hash; schema and bounded row
counts; lifecycle/phase equality; boolean identity comparisons; private-socket
device/inode/mode continuity; pane roles; process birth/group/session equality;
and cleanup assertions. No prompt, response, completed output, scrollback,
terminal frame, repository path, UUID, PID, credential, provider payload,
native session identifier, hook body, or transcript is retained in this record.

## Candidate and repository gate

The live-accepted candidate uses Codex CLI `0.150.0`, OpenCode `1.18.23`, tmux
`3.7c`, and the release executable SHA-256
`45e38d2570b219e75ff39800698242a569a9feaf33958b1a68dc28aa8280cf8e`.

The post-review source candidate has release executable SHA-256
`f2c214640847445ca5baafd6457b4605988c5af84e09f5478473310b364db391`.
Its corrected `scripts/check` run passes 501 tests, formatting, Clippy, package
build and verification, Cargo Deny advisory/license/source policy,
shell/Python checks, the disposable D12 presentation harness, the D17
source/CLI cutover check, and staged plus unstaged whitespace checks.

The final review after live acceptance found that both native Codex review
paths still removed their disposable cwd recursively by pathname. The source
candidate now uses one bounded presentation-owned marker containing exact
presentation/revision, owner PID/birth, and parent/directory device/inode
evidence. Cleanup quarantines and revalidates only the empty exact directory
and marker before non-recursive removal; a new review never adopts a dead
owner, and presentation teardown completes interrupted cleanup only after
stopping possible native users. Replacement, non-empty, malformed, foreign,
and ambiguous paths are preserved in disposable tests.

The live provider run was not repeated after this ownership-only correction,
so the two live sections below remain evidence for the first hash, not binary
parity evidence for the post-review hash. The provider command, selected
profile, native trust UI, and no-prompt boundary are unchanged; only ownership
and cleanup of the already-empty review cwd changed.

## Falsifications before acceptance

The first disposable mixed-provider attempt promoted OpenCode successfully,
then exposed two adjacent implementation defects before Codex review:

- the Codex observer profile-mutation fence probed every live Runtime, so a
  live OpenCode Runtime incorrectly blocked an unrelated Codex profile setup;
  and
- the long-lived Navigator dropped its ready OpenCode observer `Child` handle.
  Park stopped the observer but left an unreaped zombie, timed out exact helper
  cleanup, and conservatively recorded `recovery_required/unknown`.

That attempt was not accepted. Its provider, helper, presentation, private
tmux servers, copied authentication, state, and repository were removed. The
correction scopes the profile fence to Codex Runtimes while preserving exact
absence proof and provider-mismatch refusal. It also retains a detached waiter
for every ready observer and treats only an exact birth-matched Linux zombie
helper as stopped; native provider process-group authority is unchanged.

## OpenCode live result

The corrected fresh run passed:

- startup created schema 14 directly and materialized one marker-owned account
  shell without a durable Workstream or Runtime row;
- brokered `opencode` promotion retained the candidate private socket
  device/inode, shell PID, birth, process group, session, Runtime identity, and
  tmux session while the executable became the native OpenCode TUI;
- exactly one OpenCode Workstream, Runtime, ready observer handle, non-empty
  native binding, and terminal `provider_exec_proven` onboarding operation were
  recorded;
- selection moved from the promoted Shell card to its managed Workstream. The
  fresh Shell card then reopened in the right pane while durable Workstream,
  Runtime, and handle counts remained one;
- one Navigator Park converged directly to `parked/stopped`, deleted the
  OpenCode handle, stopped both provider and observer, and removed the managed
  private Runtime without a zombie or recovery detour; and
- normal `q` removed the exact provisional shell and presentation. The outer
  private tmux server and complete disposable root were then removed.

No provider prompt or turn was submitted. The provider pane remained native
and no management traffic entered it.

## Codex live result

The Codex run passed the native operator boundary and managed launch:

- before consent, schema 14 contained no Workstream, Runtime, onboarding
  operation, or integration row. Explicit shell consent created one exact
  owned profile at `trust_pending` and opened Codex's native review without
  reserving broker state;
- the operator reviewed and trusted only the four generated hooks. WSNav did
  not synthesize trust, submit a prompt, or send provider input;
- after native review exited, the exact in-memory `codex` argv retried and
  promotion retained the candidate Runtime identity, private socket
  device/inode, shell PID and birth, process group and session, cwd, and tmux
  session. The shell executable became the native Codex TUI, the integration
  became `ready`, and onboarding reached `provider_exec_proven`;
- the blank Codex landing screen intentionally remained `starting` and
  unbound: Codex emits no `SessionStart` until a first prompt creates a native
  session. WSNav neither fabricated a binding nor required provider content to
  prove the live TUI;
- one public schema-14 Park stopped that exact unbound Codex process and
  private Runtime, converging directly to `parked/stopped`. Selecting the
  permanent Shell card then materialized a fresh provisional shell while the
  durable Workstream and Runtime counts remained one; and
- exact observer removal succeeded only after the managed Runtime stopped. It
  removed the unchanged WSNav profile and integration row while preserving
  provider-owned settings.

An earlier interrupted trust handoff also exercised the failure
boundary: the provider exited after `provider_exec_proven` but before a native
binding existed. Reopening did not guess or adopt a session; it recorded
`recovery_required/unknown`. A fresh Shell still materialized without changing
durable counts. Reusing the profile after discarding its ownership database was
also refused as ambiguous. Those attempts were preserved only long enough to
verify the fail-closed result, then their exact private servers and state were
removed before the final clean run.

The final run removed the observer, provider, provisional shell, presentation,
outer private tmux server, copied authentication, provider homes, state roots,
repository, and review directories. No test process or socket remained. No
provider prompt or turn was submitted, no pane was captured, and the default
tmux server was never opened or mutated.
