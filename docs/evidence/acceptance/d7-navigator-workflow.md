# D7 Navigator Workflow Acceptance

Date: 2026-08-01

Status: pass — native observer review occurred once per host; remaining
navigator and host lifecycle evidence ran through isolated or reversible
automation.

Historical evidence note: this file records the D7.5 candidate. The current
contract, including D7.6's host-private Project browser and later navigator
presentation refinements, is in the [design](../../design.md),
[roadmap](../../roadmap.md), and [operator guide](../../../README.md).

## Native evidence

- A bare local `wsnav` launch installed the exact observer profile only after
  every managed Runtime was stopped, opened Codex's native `/hooks` review in
  the right pane, and became `Ready` only after the operator approved it and
  exited Codex.
- The same exact-remove, Host-page activation, native-review, and ready
  reconciliation completed through the registered SSH host. The remote review
  returned from the native TUI before its SSH connection closed; it did not
  become a second persistence layer.
- The live test exposed one presentation issue: observer removal correctly
  refused while a Runtime was live, but originally rendered a generic action
  error. The navigator now names the required number of live Workstreams to
  park before offering confirmation. A second live check exposed a stale
  acknowledgement projection; it now submits the attention record's current
  revision, rather than the first unseen-result revision. Both paths are
  regression-tested.

## Automated and reversible evidence

- The full repository gate passed with 209 library tests, four transport
  tests, formatting, Clippy, package verification, Cargo Deny, fixture and
  script linting, and diff checks. Its disposable D4 and D5 harnesses covered
  external Project registration, settled-turn fork, private Runtime isolation,
  deliberate runtime loss, exact native resume, and cleanup.
- A reversible client-only host forget/re-register removed the SSH registration,
  rejected a subsequent control request as unknown, re-established compatibility,
  and compared the remote Workstream, ProjectLocation, and observer counts
  before and after. They were unchanged.
- Local and SSH archive/restore each hid then restored one stopped test
  Workstream with revision guards. Neither action launched Codex, removed Git
  state, or altered the provider history.
- A separate disposable SSH endpoint registered a new Git checkout through the
  bounded host protocol. Its snapshot returned no repository path; the
  temporary client state, remote state, observer home, and checkout were then
  removed.

## Scope and cleanup

The production candidate on both hosts reports package `0.1.0`, control ABI
`1`, protocol `13`, and host schema `6`. The ordinary tmux server was not used
by any disposable Runtime; D4 and D5 compare its fingerprint before and after.
Existing operator test Workstreams were retained and stopped. No prompt,
response, title, Workstream/provider identifier, terminal capture, filesystem
path, process identifier, credential, or raw lifecycle/App Server payload is
recorded here or in the fixture.

See the sanitized [D7 fixture](../../../spikes/fixtures/d7-navigator-workflow.json).
