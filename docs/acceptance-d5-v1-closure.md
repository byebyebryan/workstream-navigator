# D5 Recovery and V1 Closure Acceptance

Date: 2026-07-30

Status: in progress — disposable recovery and fresh-install gates pass; the
combined operator-run local/remote acceptance remains the final gate.

## Automated recovery and installation evidence

- A disposable D5 harness creates a private state root, Codex home, Git
  checkout, fake provider, and tmux runtime. It never installs a hook in the
  normal Codex home or contacts the ordinary tmux server.
- After a verified native result, the harness deliberately stops only the
  recorded private tmux server. A status refresh makes the Workstream
  `recovery_required`, keeps the original binding and unseen result, and does
  not launch a blank thread.
- `recover` removes the exact now-missing private runtime directory, launches
  native `codex resume <bound-session>`, and accepts only corroborated
  `SessionStart(source=resume)`. A `startup` claim cannot reopen the
  Workstream. The state suite separately proves the unbound case permits only
  a native resume-picker selection.
- The recovered fake provider returns to the ordinary native result state. The
  former result remains unseen and recovery attention clears only after the
  verified resume binding.
- Cleanup parks the private Runtime, removes the exact owned observer profile,
  removes every disposable artifact, and compares ordinary tmux fingerprints
  before and after.
- A second disposable gate packages the crate, installs that package into a
  temporary user prefix, runs `--help`, setup, doctor, and exact observer
  removal against a temporary Codex home. It leaves no observer profile there.

`scripts/check` runs both gates, all unit/transport tests, package verification,
dependency policy checks, script/fixture linting, and diff checks.

## Remaining bounded operator acceptance

The final D5 operator run must use explicit disposable local and remote test
roots with the installed current Codex version. It will cover a local start and
settled-prefix fork, remote start and direct native attachment, an intentional
bounded control disconnect/reconnect, private-runtime loss followed by native
resume, and observer cleanup. The record will contain only boolean assertions,
version/fingerprint, isolation comparisons, and cleanup status.

The current automated evidence is intentionally not a claim that this final
real-Codex combined run has already occurred.

See the sanitized [D5 recovery fixture](../spikes/fixtures/d5-local-recovery.json).
