# D5 Recovery and V1 Closure Acceptance

Date: 2026-07-30

Status: superseded for filesystem behavior — retained lifecycle and recovery
evidence. The 2026-08-01 project-root-only contract retires the separate
managed-checkout assertions below; it does not alter the recorded provider or
private-tmux observations.

Original status: pass — disposable recovery/fresh-install gates and the
bounded operator-run local/remote native-Codex acceptance passed.

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

## Recorded combined native-Codex acceptance

- The operator created fresh local and remote Git test repositories, started
  one native Codex Workstream on each host, and completed harmless turns through
  direct native terminal attachment. Both resulting bindings and result tips
  were observed without injecting navigator traffic into either provider pane.
- A local settled source forked to a distinct native thread. The original
  separate-managed-Checkout observation is retired; the source remained live
  with its own unseen result while the destination completed a divergent native
  turn.
- The destination's exact private tmux server was deliberately stopped. A
  status refresh marked only that Workstream `recovery_required`, retaining its
  prior binding and result attention. `recover` opened the exact native
  conversation rather than a blank session. After the first resumed native
  turn, corroborated lifecycle evidence returned it to ordinary attention with
  recovery attention cleared.
- The remote test Workstream was parked and cold-resumed through separate
  short-lived SSH control connections. The operator reattached natively and
  confirmed its completed history. This proves the normal reconnect path;
  D3's cached-unavailable behavior remains separately covered by automated and
  bounded live evidence.
- All three acceptance Runtimes were parked. The ordinary tmux fingerprints on
  both hosts were unchanged, and an unrelated pre-existing remote Runtime
  remained live and unchanged.
- The regular observer integration was pre-existing and remains in place;
  removal is intentionally unsafe while unrelated Runtimes exist. Exact
  observer removal is instead proven by the isolated D5 fresh-install gate.
  During the run, the remote's stale development binary was found unable to
  read its existing state schema, so a matching current development binary was
  retained there rather than leaving the remote control plane broken. No source
  repository was copied or pushed as part of that operator deployment.

The committed record contains only booleans, version/fingerprint, isolation
comparisons, and cleanup status. It excludes provider identifiers, prompts,
responses, terminal data, paths, process IDs, credentials, and raw provider
payloads.

See the sanitized [D5 recovery fixture](../spikes/fixtures/d5-local-recovery.json)
and [combined native-Codex fixture](../spikes/fixtures/d5-combined-native-codex.json).
