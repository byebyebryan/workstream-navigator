# D25 Current-Product Stabilization and Closure

Status: locally accepted, sanitized live-provider accepted, and installed
byte-identically for operator inspection from the D25 candidate. This record
includes no passing current-source remote-CI result.

## Contract exercised

D25 stabilizes the D24 product without adding a user-facing capability or
changing schema 15. WSNav remains a host-local catalog and private-Runtime
owner around native provider TUIs. Native naming and branching remain provider
owned. No prompt, response, tool output, terminal capture, transcript,
credential, or raw provider payload becomes WSNav state.

The initial Shell-to-provider path now keeps an internal attachment helper in
the outer provider pane. When the native tmux client returns, the helper may
reconcile only after the original presentation ID/revision, provisional slot
generation, and candidate Runtime ID join to exactly one retired
`provider_exec_proven` onboarding operation, its owned Workstream and Runtime
generation, and the canonical private Runtime paths. A still-present matching
marker is an unpromoted shell detach and causes no registry mutation. Missing,
stale, duplicated, malformed, or mismatched evidence is refused.

The helper then delegates to the ordinary attachment-end proof. An exact
running PID/birth is a detach. A matching zombie, or absence on the immediate
second identity read, permits a bounded wait for the retained private pane.
Only matching PID, launch cwd, dead topology, process absence, and status `0`
authorize private-server removal and stopped/parked state. Non-zero, reused,
inaccessible, malformed, timed-out, or otherwise ambiguous evidence remains
untouched. The same proof removes a clean retained server left by an older
helper before reserving the stopped Runtime's next generation; it never adopts
a live provider into a stopped record.

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

## Sanitized live-provider acceptance

Explicit operator-authorized acceptance used isolated mode-0700 homes, provider
configuration, repositories, schema-15 state roots, presentation sockets, and
private Runtime servers. It used the exact locked artifact recorded below with
Codex 0.153.2 and OpenCode 1.18.27.

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

The locked release was atomically installed to `~/.local/bin/wsnav`. Source and
installed artifacts are mode `0755`, size 7,370,160 bytes, report
`wsnav 0.1.0`, and are byte-identical:

```text
7d8c4704a79b7c1e48c1bbff0034aaf5faf9b5905744e4520838de8eeca5bdc0  target/release/wsnav
7d8c4704a79b7c1e48c1bbff0034aaf5faf9b5905744e4520838de8eeca5bdc0  ~/.local/bin/wsnav
```

## Evidence boundaries

- No passing current-source remote CI result is included in this record.
- No current UI capture was generated. Capture selection and publication were
  explicitly left to the operator and are not a D25 exit gate.
- No Fork, Rename, Ack, Park/Unpark, provider-thread archive/delete, transcript
  preview, bulk pruning, migration, compatibility path, project cleanup,
  automatic relaunch, or packaging change was added.
- Historical D0-D24 acceptance records remain unchanged and apply only to the
  candidates they identify.
