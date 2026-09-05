# D25 Current-Product Stabilization and Closure

Status: the immediate shell-first exit correction is locally and
live-provider accepted. The corrected locked release is installed
byte-identically for operator inspection. Its current remote Rust 1.88 result
remains pending.

## Contract exercised

D25 stabilizes the D24 product without adding a user-facing capability or
changing schema 15. WSNav remains a host-local catalog and private-Runtime
owner around native provider TUIs. Native naming and branching remain provider
owned. No prompt, response, tool output, terminal capture, transcript,
credential, or raw provider payload becomes WSNav state.

The initial Shell-to-provider path now keeps an internal attachment helper in
the outer provider pane. When the native tmux client returns, an exact live
provisional pane is an ordinary detach and causes no registry mutation. An
exact dead candidate may wait for at most one bounded window while the durable
proof and presentation-private marker retirement converge. Reconciliation
still requires the original presentation ID/revision, provisional slot
generation, and candidate Runtime ID to join to exactly one retired
`provider_exec_proven` onboarding operation, its owned Workstream and Runtime
generation, and the canonical private Runtime paths. Missing, stale,
duplicated, malformed, timed-out, or mismatched evidence is refused.

The helper then delegates to the ordinary attachment-end proof. An exact
running PID/birth is a detach. A matching zombie, or absence on the immediate
second identity read, permits a bounded wait for the retained private pane.
Normally, only matching PID, dead topology, process absence, and tmux status
`0` authorize private-server removal and stopped/parked state. On Linux only,
when both tmux exit-status and exit-signal fields remain empty, the proof may
instead use one exact retained zombie whose stable birth and `/proc` field-52
raw wait status decode as normal exit `0`. It revalidates topology, PID, cwd,
and the identical zombie evidence before accepting. Signal evidence,
malformed or conflicting status, a live or reused process, or any changed read
is refused. Ordinary Runtimes additionally require launch cwd equality. A
still-starting shell-promoted Runtime may bridge an earlier absolute pane seed
cwd to its canonical recorded project cwd only through one exact
current-generation `provider_exec_proven` target that independently proved the
native provider cwd. Missing, duplicated, stale, timed-out, or otherwise
ambiguous evidence remains untouched. The same clean-exit proof removes a
retained server left by an older helper before reserving the stopped Runtime's
next generation; it never adopts a live provider into a stopped record.

After exact native clean exit, WSNav waits boundedly and read-only until the
recorded numeric provider group has no visible live members. It then re-reads
the Workstream and Runtime revisions, retained pane PID/cwd/topology and
zero-exit status, and group emptiness immediately before stopping the private
server. It does not signal an absent provider leader's unproven group.
Persistent membership, process-table failure, or changed evidence is refused;
generic Archive/internal-stop process-group semantics are unchanged.

The native attachment helper keeps the private `pane-died` hook as its normal
fast path but no longer assumes every supported tmux release delivers that
hook. Before attachment it records the exact provider PID/birth, then observes
only that process identity while the native client is running. Absence or the
exact same-birth zombie opens one private-tmux topology read. Only the exact
generated session with one dead `provider:0` pane authorizes detaching clients;
live, changed, duplicated, malformed, inaccessible, or reused evidence is
refused. The monitor does not inspect pane content or exit fields and performs
no lifecycle classification or mutation. The separately fenced attachment-end
proof retains that authority after the native client returns.

Linux `ESRCH` maps to a vanished process only at the process-group enumeration
seam after `/proc` already yielded that entry. Direct identity reads remain
strict. The private-client regression waits for the exact client PID/session
and a command-order acknowledgement through that client, the retained-exit
fixture calls the production proof as soon as exact dead topology exists, and
the focus regression polls current-screen terminal
attributes. Account-shell wrapper semantics source the exact generated wrapper
through noninteractive provider-shell command mode; separate tests retain the
exact production interactive argv/bootstrap contract. These are bounded,
deterministic test seams, not new lifecycle authority.

Codex 0.153.2 may persist its model-availability tooltip counter into the
selected observer profile during native review. WSNav accepts only the exact
`[tui.model_availability_nux]` table with a nonempty map of bounded lowercase
model slugs to unsigned 32-bit counters. Codex may also retain an explicit
`enabled = true` beside an exact trust hash after a reviewed hook is disabled
and re-enabled. That active record is accepted; `enabled = false`, another
field, or another type remains `modified`. NUX state without complete hook
hashes remains `trust_pending`; all four generated hook hashes and no disabled
hook are still required for `ready`. Unknown `tui` keys or nested tables,
invalid scalar types, duplicates, malformed model slugs, and out-of-range
counters remain `modified`.

## Source and local validation

The original GitHub MSRV diagnostic was recovered through the operator's
authenticated GitHub CLI on Starship. Historical run `33883382546`, job
`101057180882`, failed
`runtime::tests::provider_exit_hook_detaches_the_client_and_retains_dead_evidence`
after 398 tests passed. That job is evidence for the diagnosed race only; it is
not a result for D25.

GitHub run `33931301731` at
`993dc028c0818d40e861b3c3aae733bfcbae702a` passed its MSRV job but failed
check job `101210258146` in
`provider_exit_hook_detaches_the_client_and_retains_dead_evidence`. The exact
private pane was dead, both tmux exit fields were blank, and the native client
remained attached. A disposable hook marker was absent in a focused tmux 3.4
reproduction, proving this was not another client-readiness race: the
configured `pane-died` hook had not fired. That failed run is diagnosis
evidence, not current-source acceptance.

Local focused validation covered:

- exact private-client attachment before provider-exit hook assertions;
- Linux `ESRCH` scope plus permission, other-I/O, and malformed refusal;
- exact running detach versus zombie/disappearance convergence;
- zero-status cleanup, non-zero refusal, and stopped-record non-adoption;
- delayed tmux exit fields, exact zombie wait-status decoding, and
  signaled/conflicting/malformed evidence refusal;
- transient post-exit process-group drain plus persistent-membership and
  process-table-error refusal without group signalling;
- provisional identity/generation/path mismatch refusal and no-mutation
  unpromoted detach;
- account-shell wrapper semantics both with and without a controlling TTY;
- current-screen focus attributes; and
- terminal clear controls with no WSNav prose.

Before the immediate-exit correction, an Ubuntu 24.04 container with Rust
1.88.0, tmux 3.4, Git 2.43.0, and Zsh 5.9 passed 20 consecutive focused
repetitions of the stopped-retained-zero regression. That run first exposed
that tmux can publish `pane_dead=1` before `pane_dead_status`; after its fixture
was corrected to wait for the exact metadata tuple, the then-current locked
suite passed 407 library tests and 10 presentation integration tests. Those
counts are retained as historical diagnostic evidence, not current-artifact
acceptance.

The corrected complete locked suite also passed on Debian Bookworm with Rust
1.88.0 and tmux 3.3a, the environment that exposed retained dead panes whose
tmux exit fields can remain empty while the pane PID is still a zombie. Sixty
consecutive focused non-zero-exit regressions passed there after the production
proof, rather than the fixture, gained the exact read-only fallback.

## Immediate shell-first exit correction

Operator falsification on the installed prior D25 artifact started a native
provider from a newly onboarded provisional Shell and exited it immediately.
The durable onboarding journal reached `provider_exec_proven`, the retained
private pane exposed the recorded PID with status `0`, and the process was
absent, but the Runtime remained `starting` and could not be reopened. No pane
content was inspected and the specimen state was left untouched.

The cause was a false cwd equivalence in retained-exit proof: tmux permanently
reported the cwd used to seed the provisional account Shell, while onboarding
correctly recorded the canonical project root proven after the shell changed
directory and execed the provider. The ordinary retained-exit path required
those two values to be identical. A second narrow ordering window existed
between the durable `provider_exec_proven` commit and presentation-marker
retirement.

The correction keeps ordinary Runtime launch-cwd equality unchanged. Only a
`starting` Runtime with one exact current-generation onboarding target may use
that target's canonical project root as the promoted-cwd proof. PID, provider,
Workstream, generation, private paths, topology, process absence, and exit
status remain exact. The provisional helper returns immediately for an exact
live detach; an exact dead candidate may poll for at most 500 milliseconds
while durable proof and marker retirement converge. Missing, stale,
duplicated, malformed, non-zero, mismatched, or timed-out evidence grants no
mutation authority.

Focused promoted-cwd, state-before-marker, timeout, and live-detach regressions
passed. The final uninterrupted `scripts/check` run passed formatting, strict
Clippy, all 427 library tests, all 10 presentation tests, package
verification, dependency license/advisory/source policy, documentation links,
source/CLI acceptance, presentation/state acceptance, and diff checks. Cargo
Deny again emitted only the accepted duplicate-version warnings; advisories,
bans, licenses, and sources passed.

## Retained-status compatibility correction

The corrected all-target Rust 1.88 run on tmux 3.3a falsified the assumption
that a retained dead pane always publishes `pane_dead_status`. The pane was
dead and its exact PID was a zombie, but both tmux exit fields remained empty;
the prior production proof therefore refused a legitimate non-zero exit before
it could classify it as unavailable.

The correction reads Linux `/proc/<pid>/stat` field 52 without signaling or
otherwise mutating the provider. Only an exact zombie with stable birth and raw
wait status, normal-exit encoding, unchanged retained topology/PID/cwd, and a
second identical read is eligible. Exit `0` authorizes ordinary clean-exit
reconciliation; non-zero exit remains unavailable. Any tmux signal evidence,
signaled wait encoding, malformed/out-of-range value, live/reused process, or
conflict refuses. Current tmux 3.7c was separately probed on an isolated
private server and its signaled-pane metadata was refused. The exact probe
server and socket were removed.

The complete Rust 1.88 suite passed 425 library tests and 10 presentation
integration tests on both Debian Bookworm with tmux 3.3a and Ubuntu 24.04 with
tmux 3.4. The local full gate passed the same 435 tests.

An Ubuntu 24.04/tmux 3.4 stress run then exposed a second narrow cleanup race.
The retained pane and native zero exit were exact, but generic park attempted
provider-group shutdown after the leader was already absent and transiently
observed a same-numbered member. It correctly refused with
`ProcessGroupIdentityMismatch`; an immediate diagnostic found the group empty,
so there was no durable ownership evidence that could authorize a signal. The
dedicated clean-exit transition now owns the proof, waits read-only for an
empty group, and re-fences all durable and pane evidence before server removal.
The promoted preflight regression passed 60 consecutive repetitions on Rust
1.88.0 with tmux 3.4; deterministic tests cover successful drain and bounded
refusal.

The final Ubuntu full-suite run also exposed an insufficient readiness seam in
the private control-client fixture. tmux could list the exact client before its
initial `attach-session` command had completed, allowing a late attach to win
after the correct `pane-died` hook detached it. The fixture now requires the
exact control-mode `%session-changed` notification, then queues one
session-local option write through that client and observes the option
externally before releasing the provider. This is a metadata-only two-phase
command-order barrier, not a timeout increase or production change. Two sets
of 30 focused repetitions passed on tmux 3.4, the focused regression passed on
tmux 3.3a, and both final all-target container suites passed afterward.

The later remote hook-delivery falsification required a production fallback,
not another fixture delay. The attachment helper now polls the exact provider
PID/birth read-only and makes a single private-tmux query only after exact exit
evidence. Exact unit tests prove one dead generated provider pane detaches and
that live or ambiguous topology does not. The focused attachment regression
passed 500 consecutive repetitions on Ubuntu 24.04 with Rust 1.88.0 and tmux
3.4, then 200 on Debian Bookworm with Rust 1.88.0 and tmux 3.3a. The current
poll cadence was subsequently set to 100 milliseconds to avoid unnecessary
steady-state `/proc` traffic. That current cadence passed 200 consecutive
focused repetitions on local tmux 3.7c, the full current local gate, and
live-provider acceptance; the exact current remote Rust 1.88 result remains
pending.

The final Debian run exposed one additional assertion race after Archive had
completed its exact stop: Linux could still expose the same-birth process as a
zombie briefly. The test postcondition now waits boundedly and accepts only
absence or that exact same-birth zombie. It still refuses a running or reused
process and every observation error. The focused regression passed 100
consecutive repetitions, all 39 action tests passed, and the final Debian and
Ubuntu all-target suites each passed 425 library tests plus 10 presentation
integration tests.

The final local gate also exposed a disposable presentation-fixture leak. Its
Rust struct dropped the temporary root before its tmux cleanup guard, removing
the private socket before `kill-server` could use it. The guard now drops
first. A Linux regression records exact process identities and proves that
both the private tmux server and provider-pane process are absent, replaced,
or same-birth zombies after fixture drop; live or unreadable evidence fails.
It passed 60 consecutive repetitions on Debian/tmux 3.3a, both final
Rust-1.88 all-target suites passed, and no new matching fixture process
remained after validation. Four disposable servers from the falsifying local
run were exact-identified and stopped; no broader historical `/tmp` residue
was mutated.

## Current Codex native-state compatibility

Final-artifact native review on Codex 0.153.2 appended the expected four hook
trust hashes and project trust, then also persisted its provider-owned
`[tui.model_availability_nux]` display counter. A native disable/re-enable
round-trip additionally retained `enabled = true` on the reviewed SessionStart
record. The prior exact suffix parser correctly refused these previously
unknown provider fields as `modified`, which made the current native onboarding
flow unavailable even though the generated WSNav declaration and active hook
trust were exact.

The corrected parser keeps the declaration byte exact and admits only that one
schema-checked TUI table plus optional `enabled = true` on an exact trusted-hook
record. A NUX-only interrupted review stays retryable as `trust_pending`; it
cannot make the integration `ready`. Positive tests cover the observed hook,
project, NUX, and explicitly enabled state plus the unsigned-counter bound.
Negative tests cover disabled, malformed, or extended hook records; unknown or
nested TUI state; invalid types; duplicates; malformed or oversized model
slugs; and out-of-range counters.

## Sanitized live-provider acceptance

Explicit operator-authorized acceptance on corrected artifact SHA-256
`1cb2518100afdb2dd1944674a4e59c690495bb31d90673ae3a89b22c2a738e5d`
used isolated mode-0700 homes, provider configuration, repositories, schema-15
state roots, presentation sockets, and private Runtime servers with Codex
0.153.2 and OpenCode 1.18.27. Credential source files were copied with mode
`0600`, and minimal provider configuration was created with the same mode; no
values were output.

The accepted Codex run used a minimal empty base configuration plus the exact
WSNav-owned profile; OpenCode used an empty isolated configuration. Only each
provider's credential file was copied into its isolated home. Codex's native
directory and hook-review surfaces recorded four exact trusted hooks before
WSNav finalized the integration as ready. A pre-acceptance Codex specimen that
copied unrelated user configuration was rejected, exact-stopped, and deleted
before the accepted run.

Each provider reached an exact `provider_exec_proven` onboarding target with
no work prompt. The final Codex profile contained the exact reviewed hashes
and bounded NUX counter; the earlier native disable/re-enable specimen supplied
the `enabled = true` compatibility falsification. Codex was deliberately
exited before a SessionStart binding existed, covering the immediate
pre-binding edge. Human-paced native `/exit` keystrokes then produced all of
the following without provider-pane capture or content inspection:

- the Workstream became parked and its Runtime stopped;
- the recorded provider process and exact private Runtime server disappeared;
- the two-pane presentation remained live; and
- the right pane remained an inert `wsnav` helper after the stale provider
  surface was cleared without status prose.

The two disposable presentations were closed after verification. Exact socket,
process, provider-handle, and state-directory checks found no retained
acceptance resources; the isolated harness sockets and roots were then
deleted. Ordinary WSNav state remained unchanged. No provider prompt or result
was recorded in this evidence.

## Installed artifact

The corrected locked release was atomically installed to
`~/.local/bin/wsnav`. Source and installed artifacts are mode `0755`, size
7,394,744 bytes, report `wsnav 0.1.0`, and are byte-identical:

```text
1cb2518100afdb2dd1944674a4e59c690495bb31d90673ae3a89b22c2a738e5d  target/release/wsnav
1cb2518100afdb2dd1944674a4e59c690495bb31d90673ae3a89b22c2a738e5d  ~/.local/bin/wsnav
```

## Evidence boundaries

- The exact current-source remote Rust 1.88 result is pending; failing run
  `33931301731` is retained only as the hook-delivery falsification.
- No current UI capture was generated. Capture selection and publication were
  explicitly left to the operator and are not a D25 exit gate.
- No Fork, Rename, Ack, Park/Unpark, provider-thread archive/delete, transcript
  preview, bulk pruning, migration, compatibility path, project cleanup,
  automatic relaunch, or packaging change was added.
- Historical D0-D24 acceptance records remain unchanged and apply only to the
  candidates they identify.
