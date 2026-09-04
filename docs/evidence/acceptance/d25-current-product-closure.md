# D25 Current-Product Stabilization and Closure

Status: the immediate shell-first exit correction is locally accepted and
installed byte-identically for operator inspection. Sanitized live-provider
and declared-Rust-1.88 acceptance remains bound to the prior D25 artifact, not
the corrected hash. This record includes no passing current-source remote-CI
result.

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
Only matching PID, dead topology, process absence, and status `0` authorize
private-server removal and stopped/parked state. Ordinary Runtimes additionally
require launch cwd equality. A still-starting shell-promoted Runtime may bridge
an earlier absolute pane seed cwd to its canonical recorded project cwd only
through one exact current-generation `provider_exec_proven` target that
independently proved the native provider cwd. Non-zero, reused, inaccessible,
malformed, missing, duplicated, stale, timed-out, or otherwise ambiguous
evidence remains untouched. The same proof removes a clean retained server
left by an older helper before reserving the stopped Runtime's next generation;
it never adopts a live provider into a stopped record.

Linux `ESRCH` maps to a vanished process only at the process-group enumeration
seam after `/proc` already yielded that entry. Direct identity reads remain
strict. The private-client regression waits for the exact client PID/session,
the retained-exit fixture waits for tmux to publish the exact dead status before
calling the full proof, and the focus regression polls current-screen terminal
attributes. These are bounded observation seams, not new lifecycle authority.

## Source and local validation

The original GitHub MSRV diagnostic was recovered through the operator's
authenticated GitHub CLI on Starship. Historical run `33883382546`, job
`101057180882`, failed
`runtime::tests::provider_exit_hook_detaches_the_client_and_retains_dead_evidence`
after 398 tests passed. That job is evidence for the diagnosed race only; it is
not a result for D25.

Local focused validation covered:

- exact private-client attachment before provider-exit hook assertions;
- Linux `ESRCH` scope plus permission, other-I/O, and malformed refusal;
- exact running detach versus zombie/disappearance convergence;
- zero-status cleanup, non-zero refusal, and stopped-record non-adoption;
- provisional identity/generation/path mismatch refusal and no-mutation
  unpromoted detach;
- current-screen focus attributes; and
- terminal clear controls with no WSNav prose.

An Ubuntu 24.04 container with Rust 1.88.0, tmux 3.4, Git 2.43.0, and Zsh 5.9
passed 20 consecutive focused repetitions of the stopped-retained-zero
regression. The first complete rerun exposed that tmux can publish
`pane_dead=1` before `pane_dead_status`; the fixture was corrected to wait for
that exact metadata tuple while retaining the full production proof. The final
locked all-target/all-feature MSRV run passed 407 library tests and 10
presentation integration tests.

The final `scripts/check` run passed formatting, strict Clippy, all 407 library
tests, all 10 presentation tests, package verification, dependency
license/advisory/source policy, documentation links, source acceptance, and
staged/unstaged diff checks. Cargo Deny emitted only the already accepted
duplicate-version warnings; advisories, bans, licenses, and sources passed.

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
passed. The corrected uninterrupted `scripts/check` run passed formatting,
strict Clippy, all 412 library tests, all 10 presentation tests, package
verification, dependency license/advisory/source policy, documentation links,
source/CLI acceptance, presentation/state acceptance, and diff checks. Cargo
Deny again emitted only the accepted duplicate-version warnings; advisories,
bans, licenses, and sources passed. No live-provider, declared-Rust-1.88, or
remote-CI acceptance was run on the corrected artifact.

## Sanitized live-provider acceptance

This historical acceptance belongs to the prior D25 artifact with SHA-256
`7d8c4704a79b7c1e48c1bbff0034aaf5faf9b5905744e4520838de8eeca5bdc0`.
It is not current-artifact evidence. Explicit operator-authorized acceptance
used isolated mode-0700 homes, provider configuration, repositories, schema-15
state roots, presentation sockets, and private Runtime servers with Codex
0.153.2 and OpenCode 1.18.27.

For each provider, one harmless native interaction established durable
`attention`. Native `/exit` then produced all of the following without pane
content inspection:

- the Workstream became parked and its Runtime stopped;
- the recorded provider process and exact private Runtime server disappeared;
- the two-pane presentation remained live; and
- the right pane remained an inert `wsnav` helper after the stale provider
  surface was cleared without status prose.

The two disposable presentations were closed after verification. Exact socket,
process, provider-handle, and state-directory checks found no retained
acceptance resources, and both isolated roots were then deleted. No provider
prompt or result was recorded in this evidence.

## Installed artifact

The corrected locked release was atomically installed to
`~/.local/bin/wsnav`. Source and installed artifacts are mode `0755`, size
7,398,872 bytes, report `wsnav 0.1.0`, and are byte-identical:

```text
2ab6c705291d6140826e0a72c00c1668f03b7bf463b7d104c38e0dad5dfa1057  target/release/wsnav
2ab6c705291d6140826e0a72c00c1668f03b7bf463b7d104c38e0dad5dfa1057  ~/.local/bin/wsnav
```

## Evidence boundaries

- No passing current-source remote CI result is included in this record.
- No live-provider or declared-Rust-1.88 acceptance was run on the corrected
  installed hash; those results remain bound to the prior D25 artifact.
- No current UI capture was generated. Capture selection and publication were
  explicitly left to the operator and are not a D25 exit gate.
- No Fork, Rename, Ack, Park/Unpark, provider-thread archive/delete, transcript
  preview, bulk pruning, migration, compatibility path, project cleanup,
  automatic relaunch, or packaging change was added.
- Historical D0-D24 acceptance records remain unchanged and apply only to the
  candidates they identify.
